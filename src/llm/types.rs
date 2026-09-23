//! 大模型 API 的请求与响应 DTO。
//!
//! 同时覆盖两种协议：
//! - **OpenAI 兼容**（`/chat/completions`）：适用于 OpenAI、DeepSeek、
//!   Moonshot、智谱、通义、Groq、本地 Ollama/LM Studio 等绝大多数厂商。
//! - **Anthropic**（`/messages`）：结构与 OpenAI 有差异（`system` 是顶层
//!   字段而非消息、`max_tokens` 必填、响应在 `content` 数组里）。
//!
//! ## 宽容解析约定
//!
//! 各家厂商对 OpenAI 协议的实现存在细微差异（有的返回 `content` 为 `null`，
//! 有的省掉 `usage`）。因此响应结构全部使用 `Option` + `serde(default)`，
//! 且提供多个候选字段以便兼容。
//!
//! ## 关于未读字段
//!
//! 响应 DTO 中有若干字段（`usage`、`role`、`stop_reason` 等）当前仅用于
//! 完整性映射，尚未在 UI 展示。保留它们是有意为之：这些是 API 契约的
//! 组成部分，删掉会让后续需要时又要重新查文档。允许 dead_code 而非删除。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// 请求 Anthropic 时的 `max_tokens` 默认值。
///
/// Anthropic 要求该字段必填。2048 足以容纳分析建议类回复，
/// 且不会因过大而浪费额度。
pub const ANTHROPIC_DEFAULT_MAX_TOKENS: u32 = 2048;

// ---------------------------------------------------------------------------
// 请求
// ---------------------------------------------------------------------------

/// OpenAI 兼容协议的请求体。
#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<WireMessage>,
    pub temperature: f32,
    /// 是否流式。本应用使用非流式以简化实现（桌面端对首字延迟不敏感，
    /// 且非流式更容易做错误处理与重试）。
    pub stream: bool,
}

/// 协议中的消息结构。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WireMessage {
    pub role: String,
    pub content: String,
}

impl WireMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// Anthropic Messages API 的请求体。
///
/// 与 OpenAI 的关键差异：
/// - `system` 是顶层字段，不放在 `messages` 数组里
/// - `max_tokens` 是必填项
#[derive(Debug, Serialize)]
pub struct AnthropicRequest {
    pub model: String,
    /// 必填。未提供默认值时 API 会报错。
    pub max_tokens: u32,
    pub messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

// ---------------------------------------------------------------------------
// 响应
// ---------------------------------------------------------------------------

/// OpenAI 兼容协议的响应体。
#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
    /// 部分厂商在成功响应中也带 error 字段。
    #[serde(default)]
    pub error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    #[serde(default)]
    pub message: Option<ResponseMessage>,
    /// 某些实现（如旧版）直接用 `text` 字段。
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResponseMessage {
    #[serde(default)]
    pub role: Option<String>,
    /// 刻意用 `Option<String>`：部分厂商在仅返回工具调用时 content 为 null，
    /// 若用 `String` 会导致整个响应解析失败。
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

impl ChatResponse {
    /// 提取回复文本。
    ///
    /// 按优先级尝试三种可能的位置，兼容各家实现差异：
    /// 1. `choices[0].message.content`
    /// 2. `choices[0].text`
    ///
    /// 返回 `None` 表示响应成功但无文本内容（内容过滤、空回复等），
    /// 调用方应给出友好提示而非当作错误。
    pub fn extract_text(&self) -> Option<String> {
        let choice = self.choices.first()?;
        if let Some(msg) = &choice.message {
            if let Some(c) = &msg.content {
                if !c.trim().is_empty() {
                    return Some(c.clone());
                }
            }
        }
        if let Some(t) = &choice.text {
            if !t.trim().is_empty() {
                return Some(t.clone());
            }
        }
        None
    }

    /// 检查是否因长度限制被截断。
    pub fn was_truncated(&self) -> bool {
        self.choices
            .first()
            .and_then(|c| c.finish_reason.as_deref())
            == Some("length")
    }
}

/// Anthropic 响应体。
#[derive(Debug, Deserialize)]
pub struct AnthropicResponse {
    #[serde(default)]
    pub content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicContentBlock {
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

impl AnthropicResponse {
    /// 拼接所有 text 类型的内容块。
    ///
    /// Anthropic 可能返回多个内容块（例如文本 + 工具调用），只取文本部分。
    pub fn extract_text(&self) -> Option<String> {
        let joined: String = self
            .content
            .iter()
            .filter(|b| b.r#type.as_deref() == Some("text") || b.r#type.is_none())
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("");

        if joined.trim().is_empty() {
            None
        } else {
            Some(joined)
        }
    }
}

/// API 返回的错误体。
#[derive(Debug, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

impl ApiErrorBody {
    /// 汇总为单行描述。
    pub fn describe(&self) -> String {
        match (&self.message, &self.r#type, &self.code) {
            (Some(m), Some(t), _) => format!("{m} (类型: {t})"),
            (Some(m), _, Some(c)) => format!("{m} (代码: {c})"),
            (Some(m), _, _) => m.clone(),
            (None, Some(t), _) => format!("未知错误 (类型: {t})"),
            _ => "未知错误".to_string(),
        }
    }
}

/// 把 HTTP 错误响应体转换为人类可读的提示。
///
/// 不同厂商的错误结构不同，这里做统一的容错解析。
/// 关键要求：**绝不能把 API Key 回显到错误信息中**——部分厂商的
/// 错误响应会包含请求头片段。
pub fn describe_error_body(status: u16, body: &str) -> String {
    // 安全要点：**每一条返回路径都必须经过 `sanitize_body`**。
    //
    // 早期实现直接从 JSON 里取出 message 就返回，绕过了清理逻辑。
    // 现实中不少网关（以及部分厂商的错误体）会把请求头或密钥回显在
    // 错误消息里，例如 `{"error":{"message":"invalid key sk-proj-... used"}}`。
    // 结果就是这个密钥会被原样渲染到界面上、被用户截图、被贴到 issue 里。
    // 因此下面统一走 `sanitize_body`，不留旁路。
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) {
        // 尝试 error.message
        if let Some(msg) = parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            return format!("HTTP {status}: {}", truncate(&sanitize_body(msg), 300));
        }
        // 尝试顶层 message
        if let Some(msg) = parsed.get("message").and_then(|m| m.as_str()) {
            return format!("HTTP {status}: {}", truncate(&sanitize_body(msg), 300));
        }
        // 尝试 Anthropic 风格
        if let Some(err) = parsed.get("error") {
            return format!("HTTP {status}: {}", truncate(&sanitize_body(&err.to_string()), 300));
        }
    }

    // 无法解析为 JSON，返回截断后的原文（可能是 HTML 错误页）。
    let cleaned = sanitize_body(body);
    format!("HTTP {status}: {}", truncate(&cleaned, 300))
}

/// 清理响应体，移除可能包含敏感信息的内容。
///
/// 防御 `sk-` 开头的密钥片段泄露到 UI。虽然后端一般不会回显密钥，
/// 但部分网关的错误页可能包含请求头。
fn sanitize_body(body: &str) -> String {
    let mut out = body.to_string();

    // 移除常见的密钥模式。
    //
    // 关键细节（三个都不能少）：
    //   1. 必须**替换全部出现**——早期用 `if let Some(pos)` 只换第一处，
    //      第二个密钥会漏出去。
    //   2. 结束位置要从标记**之后**开始找，否则 `"Bearer "` 这类带尾随
    //      空格的标记会立刻命中自己的空格，只打码 "Bearer" 而把 token
    //      留在原地。
    //   3. 标记之后要先**跳过空白**再取密钥值。否则 `"Bearer token"` 这种
    //      写法（标记后紧跟空格）会得到空值——`end` 落在空格上，
    //      等价于什么都没打码。
    for marker in ["sk-", "Bearer", "api-key", "x-api-key", "Authorization"] {
        let mut search_from = 0usize;
        while let Some(rel) = out[search_from..].find(marker) {
            let pos = search_from + rel;
            let after_marker = pos + marker.len();

            // 跳过标记与值之间的空白（可能不止一个空格）。
            let value_start = out[after_marker..]
                .find(|c: char| !c.is_whitespace())
                .map(|i| after_marker + i)
                .unwrap_or(out.len());

            if value_start >= out.len() {
                // 标记位于末尾，其后没有值可打码。
                break;
            }

            // 从值的起始处寻找终止分隔符。
            let end = out[value_start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == '}')
                .map(|i| value_start + i)
                .unwrap_or(out.len());

            // 保护性检查：`end` 必然 >= `value_start`，否则 `replace_range` 会 panic。
            if end < value_start {
                break;
            }

            // 连同标记一起替换，避免 "Bearer [已隐藏]" 这类残留提示
            // 让人误以为后面还有内容。
            out.replace_range(pos..end, "[已隐藏]");
            search_from = pos + "[已隐藏]".len();

            if search_from >= out.len() {
                break;
            }
        }
    }

    // 移除 HTML 标签，避免 Markdown 渲染错乱。
    if out.contains('<') && out.contains('>') {
        let mut cleaned = String::with_capacity(out.len());
        let mut in_tag = false;
        for c in out.chars() {
            match c {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => cleaned.push(c),
                _ => {}
            }
        }
        out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    }

    out
}

/// 按字符（而非字节）截断字符串，避免切断多字节字符导致 panic。
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}…")
}

/// 根据 HTTP 状态码给出可操作的建议。
pub fn hint_for_status(status: u16) -> &'static str {
    match status {
        401 => "API Key 无效或已过期，请在设置页重新填写。",
        403 => "该 Key 无权访问此模型，请检查模型名称或账号权限。",
        404 => "接口路径不存在，请检查 Base URL 是否正确（通常需要包含 /v1）。",
        422 => "请求参数不被接受，请检查模型名称是否正确。",
        429 => "请求过于频繁或额度已用尽，请稍后重试或检查账户余额。",
        s if s >= 500 => "服务端暂时不可用，请稍后重试。",
        _ => "请检查配置是否正确。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_response_extracts_message_content() {
        let raw = r#"{"choices":[{"message":{"role":"assistant","content":"你好"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;
        let r: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.extract_text().as_deref(), Some("你好"));
        assert!(!r.was_truncated());
        assert_eq!(r.usage.unwrap().total_tokens, Some(15));
    }

    #[test]
    fn openai_response_handles_null_content() {
        // 部分厂商在内容被过滤时返回 content: null。
        let raw = r#"{"choices":[{"message":{"role":"assistant","content":null},"finish_reason":"content_filter"}]}"#;
        let r: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.extract_text(), None);
    }

    #[test]
    fn openai_response_falls_back_to_text_field() {
        let raw = r#"{"choices":[{"text":"传统格式回复"}]}"#;
        let r: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.extract_text().as_deref(), Some("传统格式回复"));
    }

    #[test]
    fn openai_response_handles_empty_choices() {
        let raw = r#"{"choices":[]}"#;
        let r: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.extract_text(), None);
    }

    #[test]
    fn openai_response_handles_whitespace_only_content() {
        let raw = r#"{"choices":[{"message":{"content":"   \n  "}}]}"#;
        let r: ChatResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.extract_text(), None, "纯空白应视为无内容");
    }

    #[test]
    fn truncated_response_detected() {
        let raw = r#"{"choices":[{"message":{"content":"被截断的内容"},"finish_reason":"length"}]}"#;
        let r: ChatResponse = serde_json::from_str(raw).unwrap();
        assert!(r.was_truncated());
    }

    #[test]
    fn anthropic_response_joins_text_blocks() {
        let raw = r#"{"content":[{"type":"text","text":"第一段"},
            {"type":"text","text":"第二段"}],"stop_reason":"end_turn"}"#;
        let r: AnthropicResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.extract_text().as_deref(), Some("第一段第二段"));
    }

    #[test]
    fn anthropic_response_skips_non_text_blocks() {
        let raw = r#"{"content":[{"type":"tool_use","text":null},{"type":"text","text":"只有这段"}]}"#;
        let r: AnthropicResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.extract_text().as_deref(), Some("只有这段"));
    }

    #[test]
    fn anthropic_response_with_no_text_returns_none() {
        let raw = r#"{"content":[{"type":"tool_use"}]}"#;
        let r: AnthropicResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.extract_text(), None);
    }

    #[test]
    fn request_serializes_with_expected_shape() {
        let req = ChatRequest {
            model: "gpt-4o-mini".into(),
            messages: vec![WireMessage::system("sys"), WireMessage::user("hi")],
            temperature: 0.3,
            stream: false,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], "gpt-4o-mini");
        assert_eq!(v["messages"][0]["role"], "system");
        assert_eq!(v["messages"][0]["content"], "sys");
        assert_eq!(v["stream"], false);
    }

    #[test]
    fn anthropic_request_omits_optional_fields_when_none() {
        let req = AnthropicRequest {
            model: "claude-3-5-sonnet".into(),
            max_tokens: 2048,
            messages: vec![WireMessage::user("hi")],
            system: None,
            temperature: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("system").is_none(), "None 字段应被跳过");
        assert!(v.get("temperature").is_none());
        assert_eq!(v["max_tokens"], 2048);
    }

    #[test]
    fn error_body_describes_openai_style() {
        let body = r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error","code":"invalid_api_key"}}"#;
        let d = describe_error_body(401, body);
        assert!(d.contains("Incorrect API key"));
        assert!(d.contains("401"));
    }

    #[test]
    fn error_body_describes_top_level_message() {
        let body = r#"{"message":"模型不存在"}"#;
        let d = describe_error_body(404, body);
        assert!(d.contains("模型不存在"));
    }

    #[test]
    fn error_body_handles_html_page() {
        let body = "<html><head><title>502 Bad Gateway</title></head><body>Nginx error</body></html>";
        let d = describe_error_body(502, body);
        assert!(!d.contains('<'), "HTML 标签应被剥离: {d}");
        assert!(d.contains("502"));
    }

    #[test]
    fn error_body_never_reveals_api_key() {
        // 某些网关的错误页会回显请求头，必须打码。
        let body = r#"{"error":{"message":"invalid key sk-proj-abcdef1234567890xyz used"}}"#;
        let d = describe_error_body(401, body);
        assert!(
            !d.contains("sk-proj-abcdef1234567890xyz"),
            "错误信息泄露了 API Key: {d}"
        );
        assert!(d.contains("[已隐藏]"));
    }

    #[test]
    fn error_body_handles_non_json_non_html() {
        let d = describe_error_body(500, "plain text failure");
        assert!(d.contains("plain text failure"));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // 中文是多字节字符，按字节截断会 panic。
        let s = "这是一段很长的中文文本用于测试截断行为";
        let t = truncate(s, 5);
        assert_eq!(t.chars().count(), 6, "5 个字符 + 省略号");
        assert!(t.ends_with('…'));
        // 边界：不截断时原样返回。
        assert_eq!(truncate("短", 10), "短");
    }

    #[test]
    fn truncate_handles_exact_boundary() {
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn hints_cover_common_status_codes() {
        assert!(hint_for_status(401).contains("API Key"));
        assert!(hint_for_status(403).contains("权限"));
        assert!(hint_for_status(404).contains("/v1"));
        assert!(hint_for_status(429).contains("频繁"));
        assert!(hint_for_status(500).contains("稍后"));
        assert!(!hint_for_status(418).is_empty());
    }

    #[test]
    fn sanitize_removes_bearer_tokens() {
        let s = sanitize_body("request failed with Bearer abcdefghijk token");
        assert!(!s.contains("abcdefghijk"), "实得: {s}");
    }
}
