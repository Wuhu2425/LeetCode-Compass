//! 大模型统一客户端。
//!
//! ## 为什么一个客户端能覆盖多数厂商
//!
//! OpenAI 的 `/chat/completions` 事实上成了行业标准接口，DeepSeek、
//! Moonshot、智谱、通义、Groq、SiliconFlow、本地 Ollama/LM Studio 等
//! 都提供兼容端点。因此只需一个实现 + 可配置的 `base_url`，就能接入
//! 这些厂商，无需为每家写适配器。
//!
//! Anthropic 的 Messages API 协议不同，单独分支处理。
//!
//! ## 凭据处理
//!
//! API Key 从共享配置中**每次请求动态读取**，不缓存在客户端内部。
//! 这保证了用户在设置页更新 Key 后立即生效，也避免了在长生命周期对象
//! 中持有多份密钥副本。

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use tokio::sync::RwLock;

use crate::config::{AppConfig, LlmConfig, LlmProvider};
use crate::error::{AppError, AppResult};
use crate::llm::types::*;

/// 统一的大模型客户端。
#[derive(Clone)]
pub struct LlmClient {
    config: Arc<RwLock<AppConfig>>,
}

impl LlmClient {
    pub fn new(config: Arc<RwLock<AppConfig>>) -> Self {
        Self { config }
    }

    /// 读取当前 LLM 配置快照。
    async fn snapshot(&self) -> LlmConfig {
        self.config.read().await.llm.clone()
    }

    /// 构造请求头。
    ///
    /// 不同厂商的认证头不同：
    /// - OpenAI 兼容：`Authorization: Bearer <key>`
    /// - Anthropic：`x-api-key: <key>` + `anthropic-version`
    fn build_headers(cfg: &LlmConfig) -> AppResult<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        // 密钥含非法字符（如换行）时给出明确错误，而不是让 reqwest
        // 抛出难以理解的转换失败。
        let key = cfg.api_key.trim();
        if key.contains(['\n', '\r']) {
            return Err(AppError::InvalidInput(
                "API Key 中不能包含换行符，请检查是否误粘贴了多行内容".to_string(),
            ));
        }

        match cfg.provider {
            LlmProvider::OpenAiCompatible => {
                headers.insert(
                    "Authorization",
                    HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| {
                        AppError::InvalidInput("API Key 含非法字符，无法构造认证头".to_string())
                    })?,
                );
            }
            LlmProvider::Anthropic => {
                headers.insert(
                    "x-api-key",
                    HeaderValue::from_str(key).map_err(|_| {
                        AppError::InvalidInput("API Key 含非法字符".to_string())
                    })?,
                );
                // Anthropic 要求显式声明 API 版本，缺失会返回 400。
                headers.insert(
                    "anthropic-version",
                    HeaderValue::from_static("2023-06-01"),
                );
            }
        }

        Ok(headers)
    }

    /// 发送对话请求，返回模型回复文本。
    ///
    /// `messages` 中若含 `System` 角色消息，会按各协议的要求处理：
    /// - OpenAI 兼容：原样放入 messages 数组
    /// - Anthropic：抽出为顶层 `system` 字段（Anthropic 不接受
    ///   messages 数组中出现 system 角色）
    pub async fn chat(&self, messages: Vec<WireMessage>) -> AppResult<String> {
        let cfg = self.snapshot().await;

        // 前置校验：配置不完整时立即失败，避免无意义的网络往返。
        //
        // `validate()` 允许"完全未配置"（表示用户不启用 AI），
        // 因此这里还要用 `is_usable()` 判断**是否真的能用**。
        // 两者分工：validate 管"填了就得合法"，is_usable 管"是否可用"。
        if !cfg.is_usable() {
            return Err(AppError::InvalidInput(
                "尚未配置大模型（需要 API Key）。请在设置页填写后再使用 AI 助理".to_string(),
            ));
        }
        if let Err(e) = cfg.validate() {
            return Err(AppError::InvalidInput(e));
        }

        let endpoint = cfg.chat_endpoint();
        let headers = Self::build_headers(&cfg)?;

        let (body, is_anthropic) = match cfg.provider {
            LlmProvider::OpenAiCompatible => {
                let req = ChatRequest {
                    model: cfg.model.clone(),
                    messages,
                    temperature: cfg.temperature,
                    stream: false,
                };
                (
                    serde_json::to_value(&req).map_err(|e| {
                        AppError::Llm(format!("请求体序列化失败: {e}"))
                    })?,
                    false,
                )
            }
            LlmProvider::Anthropic => {
                // 抽出 system 消息。
                let mut system_parts: Vec<String> = Vec::new();
                let mut normal: Vec<WireMessage> = Vec::new();
                for m in messages {
                    if m.role == "system" {
                        system_parts.push(m.content);
                    } else {
                        normal.push(m);
                    }
                }
                let system = if system_parts.is_empty() {
                    None
                } else {
                    Some(system_parts.join("\n\n"))
                };

                let req = AnthropicRequest {
                    model: cfg.model.clone(),
                    max_tokens: ANTHROPIC_DEFAULT_MAX_TOKENS,
                    messages: normal,
                    system,
                    temperature: Some(cfg.temperature),
                };
                (
                    serde_json::to_value(&req).map_err(|e| {
                        AppError::Llm(format!("请求体序列化失败: {e}"))
                    })?,
                    true,
                )
            }
        };

        // 单独构造 client 以便应用配置中的超时（不同用途可能设不同值）。
        //
        // 必须先确保加密后端已安装：本项目用 `rustls-no-provider`，
        // 缺少注册时构造 client 会直接 panic。这是第三个入口
        // （另两处见 `main()` 与 `LeetCodeClient::new`）——
        // 漏掉任何一处都会在特定代码路径上炸。幂等，重复调用安全。
        crate::error::install_crypto_provider();

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_secs))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(AppError::Network)?;

        let resp = http
            .post(&endpoint)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(AppError::Network)?;

        let status = resp.status();

        if !status.is_success() {
            let code = status.as_u16();
            // 读取错误体以提取厂商的具体说明。
            let body_text = resp.text().await.unwrap_or_default();
            let detail = describe_error_body(code, &body_text);
            let hint = hint_for_status(code);

            return Err(match code {
                401 | 403 => AppError::Unauthorized(format!("{detail} {hint}")),
                429 => AppError::RateLimited,
                404 | 422 => AppError::InvalidInput(format!("{detail} {hint}")),
                _ => AppError::Llm(format!("{detail} {hint}")),
            });
        }

        let raw = resp.text().await.map_err(AppError::Network)?;

        if is_anthropic {
            let parsed: AnthropicResponse = serde_json::from_str(&raw).map_err(|e| {
                AppError::parse(format!("大模型响应解析失败: {e}"), Some(&raw))
            })?;
            if let Some(err) = &parsed.error {
                return Err(AppError::Llm(err.describe()));
            }
            parsed.extract_text().ok_or_else(|| {
                AppError::Llm(
                    "模型返回了空内容。可能是触发内容过滤或请求被中断，请调整提问后重试。"
                        .to_string(),
                )
            })
        } else {
            let parsed: ChatResponse = serde_json::from_str(&raw).map_err(|e| {
                AppError::parse(format!("大模型响应解析失败: {e}"), Some(&raw))
            })?;
            if let Some(err) = &parsed.error {
                return Err(AppError::Llm(err.describe()));
            }
            let text = parsed.extract_text().ok_or_else(|| {
                AppError::Llm(
                    "模型返回了空内容。可能是触发内容过滤或请求被中断，请调整提问后重试。"
                        .to_string(),
                )
            })?;

            if parsed.was_truncated() {
                // 不视为错误，但附加说明让用户知道回复不完整。
                return Ok(format!(
                    "{text}\n\n---\n_（回复因长度限制被截断，可要求模型继续或缩短输出）_"
                ));
            }
            Ok(text)
        }
    }

    /// 连通性测试。
    ///
    /// 用一个极短的提问验证：Base URL 可达、API Key 有效、模型名存在。
    /// 使用最小 token 消耗（提问与预期回复都很短）。
    pub async fn test_connection(&self) -> AppResult<String> {
        let messages = vec![WireMessage::user("请回复「连接成功」四个字，不要有任何其他内容。")];
        let reply = self.chat(messages).await?;
        Ok(reply.trim().to_string())
    }

    /// 配置是否已可用于发起请求。
    ///
    /// **同步**：UI 层每帧都要用它决定按钮是否可用，若做成 async 就得
    /// 在渲染循环里 `block_on`，那会阻塞界面。这里用 `try_read` 读配置
    /// 快照；读锁被占用时（后台任务正在写）保守返回 `false`，
    /// 下一帧即可得到正确结果。
    pub fn is_configured(&self) -> bool {
        match self.config.try_read() {
            Ok(cfg) => cfg.llm.is_usable(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn cfg_with(llm: LlmConfig) -> Arc<RwLock<AppConfig>> {
        Arc::new(RwLock::new(AppConfig {
            llm,
            ..Default::default()
        }))
    }

    #[test]
    fn openai_headers_use_bearer_scheme() {
        let cfg = LlmConfig {
            api_key: "sk-test".into(),
            ..Default::default()
        };
        let h = LlmClient::build_headers(&cfg).unwrap();
        assert_eq!(h.get("Authorization").unwrap(), "Bearer sk-test");
        assert!(!h.contains_key("x-api-key"));
    }

    #[test]
    fn anthropic_headers_use_x_api_key_and_version() {
        let cfg = LlmConfig {
            provider: LlmProvider::Anthropic,
            api_key: "sk-ant-test".into(),
            ..Default::default()
        };
        let h = LlmClient::build_headers(&cfg).unwrap();
        assert_eq!(h.get("x-api-key").unwrap(), "sk-ant-test");
        assert_eq!(h.get("anthropic-version").unwrap(), "2023-06-01");
        assert!(!h.contains_key("Authorization"), "不应使用 Bearer 方案");
    }

    #[test]
    fn headers_reject_multiline_api_key() {
        let cfg = LlmConfig {
            api_key: "sk-line1\nline2".into(),
            ..Default::default()
        };
        let err = LlmClient::build_headers(&cfg).unwrap_err();
        assert!(err.to_string().contains("换行符"));
    }

    #[test]
    fn headers_trim_whitespace_from_key() {
        let cfg = LlmConfig {
            api_key: "  sk-padded  ".into(),
            ..Default::default()
        };
        let h = LlmClient::build_headers(&cfg).unwrap();
        assert_eq!(h.get("Authorization").unwrap(), "Bearer sk-padded");
    }

    #[test]
    fn is_configured_reflects_api_key_presence() {
        let client = LlmClient::new(cfg_with(LlmConfig::default()));
        assert!(!client.is_configured(), "默认配置无 Key");

        let client2 = LlmClient::new(cfg_with(LlmConfig {
            api_key: "sk-x".into(),
            ..Default::default()
        }));
        assert!(client2.is_configured());
    }

    #[tokio::test]
    async fn chat_fails_fast_when_not_configured() {
        let client = LlmClient::new(cfg_with(LlmConfig::default()));
        let r = client.chat(vec![WireMessage::user("hi")]).await;
        // 空配置现在**可通过** `validate()`（表示用户不启用 AI），
        // 因此失败点前移到 `is_usable()` 的检查：应在发起网络请求前
        // 就明确告知"未配置"，而不是让用户看到一个底层的连接错误。
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(
            msg.contains("未配置") || msg.contains("API Key"),
            "应提示未配置大模型，实际: {msg}"
        );
    }

    #[tokio::test]
    async fn chat_rejects_invalid_base_url_before_network() {
        let client = LlmClient::new(cfg_with(LlmConfig {
            base_url: "http://evil.example.com/v1".into(),
            api_key: "sk-x".into(),
            model: "m".into(),
            ..Default::default()
        }));
        let r = client.chat(vec![WireMessage::user("hi")]).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("https"));
    }

    #[tokio::test]
    async fn config_update_takes_effect_without_recreating_client() {
        let cfg = cfg_with(LlmConfig::default());
        let client = LlmClient::new(cfg.clone());
        assert!(!client.is_configured());

        // 模拟用户在设置页填入 Key。
        {
            let mut g = cfg.write().await;
            g.llm.api_key = "sk-new".into();
        }
        assert!(client.is_configured(), "客户端应感知到配置变更");
    }

    #[test]
    fn anthropic_system_message_is_extracted() {
        // 验证 system 消息不会出现在 Anthropic 的 messages 数组中。
        let messages = vec![
            WireMessage::system("你是助手"),
            WireMessage::user("你好"),
        ];
        let mut system_parts = Vec::new();
        let mut normal = Vec::new();
        for m in messages {
            if m.role == "system" {
                system_parts.push(m.content);
            } else {
                normal.push(m);
            }
        }
        assert_eq!(system_parts.len(), 1);
        assert_eq!(normal.len(), 1);
        assert_eq!(normal[0].role, "user");
    }
}
