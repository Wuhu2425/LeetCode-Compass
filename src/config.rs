//! 应用配置。
//!
//! 分两类内容：
//! 1. **非敏感偏好**：用户名、主题、每页条数等，可明文持久化。
//! 2. **敏感凭据**：LeetCode session cookie、大模型 API Key。
//!
//! 敏感凭据的处理约定（贯穿全项目）：
//! - 只写入本地数据库，绝不上传；
//! - `Debug` 实现必须打码，防止随日志泄露；
//! - UI 默认以密码态渲染，需显式点击才明文显示。
//!
//! 注意：本模块刻意不派生 `Debug`，而是手写实现以打码敏感字段。

use serde::{Deserialize, Serialize};

/// 大模型提供方。
///
/// 绝大多数厂商（OpenAI / DeepSeek / Moonshot / 智谱 / 通义 / 本地 Ollama
/// 的兼容模式）都实现了 OpenAI 的 `/chat/completions` 协议，因此只需一个
/// 客户端实现 + 不同的 Base URL 即可覆盖，无需为每家写适配器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmProvider {
    /// OpenAI 兼容协议（默认）。适用于绝大多数厂商。
    OpenAiCompatible,
    /// Anthropic Messages API（协议不同，单独适配）。
    Anthropic,
}

impl LlmProvider {
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "OpenAI 兼容协议",
            Self::Anthropic => "Anthropic",
        }
    }

    pub fn all() -> [LlmProvider; 2] {
        [Self::OpenAiCompatible, Self::Anthropic]
    }

    /// 默认 Base URL，便于用户少填一项。
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
        }
    }
}

/// 大模型连接配置。
///
/// `api_key` 属于敏感字段，见模块级文档说明。
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    /// API 根地址，不含 `/chat/completions` 路径。
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// 采样温度，0.0..=2.0。
    pub temperature: f32,
    /// 单次请求超时（秒）。
    pub timeout_secs: u64,
    /// 保留的最大对话轮数（用于裁剪上下文，控制 token 消耗）。
    pub max_history_turns: usize,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: LlmProvider::OpenAiCompatible,
            base_url: LlmProvider::OpenAiCompatible.default_base_url().to_string(),
            model: "gpt-4o-mini".to_string(),
            api_key: String::new(),
            temperature: 0.3,
            timeout_secs: 60,
            max_history_turns: 10,
        }
    }
}

impl std::fmt::Debug for LlmConfig {
    /// 手写实现以打码 API Key，避免密钥通过 `{:?}` 泄露到日志或错误信息中。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &mask_secret(&self.api_key))
            .field("temperature", &self.temperature)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_history_turns", &self.max_history_turns)
            .finish()
    }
}

impl LlmConfig {
    /// 配置是否完整到可以发起请求。
    pub fn is_usable(&self) -> bool {
        !self.api_key.trim().is_empty()
            && !self.base_url.trim().is_empty()
            && !self.model.trim().is_empty()
    }

    /// 校验并规范化。返回人类可读的错误说明。
    ///
    /// **契约：空配置合法。** AI 功能是可选增强，未填写 API Key 表示
    /// 用户不启用它，此时不应报错。
    ///
    /// 因此校验只在用户**确实想启用**（即填了 API Key）时才要求配置完整；
    /// 但无论是否启用，只要填了字段就要合法（如 URL 必须 https）——
    /// 免得留下一个"看起来能用、实际一调用就失败"的半成品状态。
    pub fn validate(&self) -> Result<(), String> {
        let has_key = !self.api_key.trim().is_empty();

        if !has_key {
            // 未填 Key：视为不启用 AI。但仍检查已填字段的合法性。
            // Base URL 与 Model 有默认值，通常非空，做一次轻量校验即可。
            let url = self.base_url.trim();
            if !url.is_empty() && !url.starts_with("https://") && !is_local_endpoint(url) {
                return Err("Base URL 必须使用 https（本地服务可用 http://127.0.0.1）".into());
            }
            return Ok(());
        }

        // 已填 Key：要求配置完整且合法，否则请求必然失败。
        let url = self.base_url.trim();
        if url.is_empty() {
            return Err("Base URL 不能为空".into());
        }
        // 只接受 https，避免密钥经明文 HTTP 传输被中间人截获。
        // 例外：本地回环地址允许 http（Ollama、LM Studio 等本地推理服务）。
        if !url.starts_with("https://") && !is_local_endpoint(url) {
            return Err("Base URL 必须使用 https（本地服务可用 http://127.0.0.1）".into());
        }
        if self.model.trim().is_empty() {
            return Err("模型名称不能为空".into());
        }
        if !(0.0..=2.0).contains(&self.temperature) {
            return Err("温度需在 0.0 到 2.0 之间".into());
        }
        if self.timeout_secs == 0 || self.timeout_secs > 600 {
            return Err("超时时间需在 1 到 600 秒之间".into());
        }
        if self.max_history_turns == 0 {
            return Err("对话历史轮数至少为 1".into());
        }
        Ok(())
    }

    /// 拼出完整的 chat completions 端点。
    ///
    /// 兼容用户把 `/v1` 写成 `/v1/chat/completions` 的常见误填：若已包含
    /// 该路径则不再追加。
    pub fn chat_endpoint(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        match self.provider {
            LlmProvider::OpenAiCompatible => {
                if base.ends_with("/chat/completions") {
                    base.to_string()
                } else {
                    format!("{base}/chat/completions")
                }
            }
            LlmProvider::Anthropic => {
                if base.ends_with("/messages") {
                    base.to_string()
                } else {
                    format!("{base}/messages")
                }
            }
        }
    }
}

/// 判断是否为本地/回环地址。
fn is_local_endpoint(url: &str) -> bool {
    const LOCAL_PREFIXES: [&str; 4] = [
        "http://127.0.0.1",
        "http://localhost",
        "http://[::1]",
        "http://0.0.0.0",
    ];
    LOCAL_PREFIXES.iter().any(|p| url.starts_with(p))
}

/// LeetCode 站点。
///
/// **这是会话凭据能否生效的关键。** 国际站（leetcode.com）与中国站
/// （leetcode.cn）使用**完全独立**的账号体系与 Cookie 域。把 leetcode.cn
/// 的 Cookie 发往 leetcode.com，服务端视作未登录，表现就是"Cookie 总是过期"。
///
/// 用户的 Cookie 从哪个站复制的，就必须选哪个站。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LeetCodeSite {
    /// 国际站 leetcode.com
    #[default]
    Com,
    /// 中国站 leetcode.cn（力扣）
    Cn,
}

impl LeetCodeSite {
    /// 站点主域名。
    pub fn host(self) -> &'static str {
        match self {
            Self::Com => "leetcode.com",
            Self::Cn => "leetcode.cn",
        }
    }

    /// GraphQL 端点完整地址。
    pub fn graphql_endpoint(self) -> String {
        format!("https://{}/graphql", self.host())
    }

    /// 用于界面的中文标签。
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Com => "国际站 leetcode.com",
            Self::Cn => "中国站 leetcode.cn（力扣）",
        }
    }

    pub fn all() -> [LeetCodeSite; 2] {
        [Self::Com, Self::Cn]
    }

    /// 该站点的 GraphQL schema 是否支持"用户技能标签统计"。
    ///
    /// 国际站的 `matchedUser.tagProblemCounts` 在中国站**不存在**
    /// （无对应根字段）。这是推荐算法的核心输入之一，缺失会导致推荐
    /// 降级为"仅基于题库元数据"。
    pub fn supports_tag_stats(self) -> bool {
        matches!(self, Self::Com)
    }

    /// 该站点是否支持"提交日历"。
    ///
    /// 中国站无 `userProfileCalendar` / `userCalendar` 根字段。
    pub fn supports_calendar(self) -> bool {
        matches!(self, Self::Com)
    }

    /// 该站点是否支持"竞赛复盘"。
    ///
    /// 中国站的 `userContestRanking` 存在但参数名与字段名都与国际站不同，
    /// 目前未做适配，因此视为不支持（而非返回错误数据）。
    pub fn supports_contest(self) -> bool {
        matches!(self, Self::Com)
    }

    /// 该站点是否支持"最近提交记录"。
    ///
    /// 中国站的 `submissionList` 节点类型是 `SubmissionDumpNode`，
    /// 没有国际站的 `titleSlug` 字段，无法映射到题目。
    pub fn supports_recent_submissions(self) -> bool {
        matches!(self, Self::Com)
    }

    /// 该站点是否提供"全站题目难度统计"。
    ///
    /// 国际站的 `allQuestionsCount` 在中国站不存在。中国站只能拿到
    /// **当前用户自己的**通过/未通过数，拿不到全站题量。
    /// 缺失时 UI 应改用题库列表返回的 `total` 作为总量。
    pub fn supports_global_counts(self) -> bool {
        matches!(self, Self::Com)
    }

    /// 该站点是否提供题目的中文标题。
    ///
    /// 中国站的题目节点带 `titleCn`，可优先展示中文。
    pub fn prefers_chinese_titles(self) -> bool {
        matches!(self, Self::Cn)
    }

    /// 站点能力的单行说明，用于设置页提示用户。
    pub fn capability_note(self) -> &'static str {
        match self {
            Self::Com => "支持全部功能：题库、画像、标签统计、提交日历、竞赛复盘。",
            Self::Cn => {
                "支持题库与账号画像。因力扣中国站接口限制，\
                 标签统计、提交日历、竞赛复盘、最近提交不可用。"
            }
        }
    }
}

/// 从一个完整的 Cookie 字符串中解析出 `LEETCODE_SESSION` 与 `csrftoken`。
///
/// 设计动机：用户在浏览器开发者工具里复制的是**整段 Cookie**
/// （`LEETCODE_SESSION=xxx; csrftoken=yyy; _gid=zzz; ...`），
/// 而不是单个值。要求用户手动抠出其中两项既繁琐又极易出错——
/// 多复制一个分号、少复制一段都会导致"Cookie 无效"。
///
/// 因此这里做**尽可能宽容**的解析：
/// - 支持 `;` 或换行分隔的多项；
/// - 忽略 `key=` 两侧空白；
/// - 只挑出需要的两项，其余（`_gid`、`_ga` 等）丢弃；
/// - 若输入本身就是裸值（不含 `=`），则按调用方指定的字段直接采用。
///
/// 返回 `(session_cookie, csrf_token)`，未能解析出的为 `None`。
pub fn parse_cookie_blob(blob: &str) -> (Option<String>, Option<String>) {
    let trimmed = blob.trim();
    if trimmed.is_empty() {
        return (None, None);
    }

    let mut session: Option<String> = None;
    let mut csrf: Option<String> = None;

    // 先按分号切；若整段不含 '='，说明是裸值，交由下方兜底处理。
    if trimmed.contains('=') {
        for part in trimmed.split([';', '\n']) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let key = key.trim();
            // 同时去掉单双引号：用户从浏览器控制台复制时可能带引号。
            let value = value.trim().trim_matches(['"', '\'']);
            if value.is_empty() {
                continue;
            }
            if key.eq_ignore_ascii_case("LEETCODE_SESSION") {
                session = Some(value.to_owned());
            } else if key.eq_ignore_ascii_case("csrftoken") {
                csrf = Some(value.to_owned());
            }
        }
    } else {
        // 裸值：无法判断属于哪一项。但 `csrftoken` 有固定长度特征（32 位），
        // 而 `LEETCODE_SESSION` 明显更长，可据此区分。
        //
        // 先滤掉不含任何字母数字的输入（如 ";;;"、"--"），它们显然是分隔符
        // 或乱码而非凭据值。
        let trimmed_quotes = trimmed.trim_matches(['"', '\'']);
        if trimmed_quotes.chars().any(|c| c.is_ascii_alphanumeric()) {
            if trimmed_quotes.len() >= 32 {
                session = Some(trimmed_quotes.to_owned());
            } else {
                csrf = Some(trimmed_quotes.to_owned());
            }
        }
    }

    (session, csrf)
}

/// LeetCode 站点与会话凭据。
///
/// 仅在用户主动配置后存在。缺失时应用进入降级模式（功能仍可用，
/// 但无法获知逐题完成状态）。
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LeetCodeSession {
    /// `LEETCODE_SESSION` cookie 的值。
    pub session_cookie: String,
    /// `csrftoken` cookie 的值。必须与 `x-csrftoken` 请求头一致。
    pub csrf_token: String,
    /// 凭据所属站点。默认国际站。
    #[serde(default)]
    pub site: LeetCodeSite,
}

impl std::fmt::Debug for LeetCodeSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 同样打码：session cookie 等价于账号登录态，泄露即可被完全冒充。
        f.debug_struct("LeetCodeSession")
            .field("session_cookie", &mask_secret(&self.session_cookie))
            .field("csrf_token", &mask_secret(&self.csrf_token))
            .finish()
    }
}

impl LeetCodeSession {
    /// 两项凭据是否都已提供。
    pub fn is_complete(&self) -> bool {
        !self.session_cookie.trim().is_empty() && !self.csrf_token.trim().is_empty()
    }

    /// 是否完全未配置。
    pub fn is_empty(&self) -> bool {
        self.session_cookie.trim().is_empty() && self.csrf_token.trim().is_empty()
    }

    /// 基本形态校验。这里只做长度与空白检查——真正的有效性只能由服务端判定。
    pub fn validate(&self) -> Result<(), String> {
        if self.is_empty() {
            return Err("未配置会话凭据".into());
        }
        if self.session_cookie.trim().is_empty() {
            return Err("缺少 LEETCODE_SESSION".into());
        }
        if self.csrf_token.trim().is_empty() {
            return Err("缺少 csrftoken".into());
        }
        if self.session_cookie.contains(['\n', '\r']) || self.csrf_token.contains(['\n', '\r']) {
            return Err("凭据中不能包含换行符（请检查是否误粘贴了多行内容）".into());
        }
        // 值里混入分号通常意味着用户把整段 Cookie 原样粘进了单值输入框。
        // 后续构造 Cookie 头时会产生 `LEETCODE_SESSION=a; b; c` 这类畸形结构，
        // 服务端只取到前半段，表现为"Cookie 无效"。此处提前拦截并给出明确指引。
        //
        // 注意检查顺序：分号检查须在 csrftoken 格式检查**之前**，
        // 因为整段粘贴时 csrftoken 字段往往已包含分号与其它键值，
        // 先报"分号"比先报"长度不符"更贴近用户的实际操作。
        if self.session_cookie.contains(';') || self.csrf_token.contains(';') {
            return Err(
                "凭据值中出现了分号，说明粘贴的是整段 Cookie。\
                 请使用「自动提取」按钮，或只填写等号后的那一串值"
                    .into(),
            );
        }
        // LeetCode 的 csrftoken 固定为 32 位字母数字。长度明显不符多半是复制错了
        // （如复制成了 CSRF-TOKEN 头的值，或漏了一段）。这是本地可判定的常见错误，
        // 提前拦截能避免用户把"格式错"误认为"Cookie 过期"。
        let csrf = self.csrf_token.trim();
        if csrf.len() != 32 || !csrf.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(format!(
                "csrftoken 格式不符（期望 32 位字母数字，实际 {} 位）。\
                 请确认复制的是 csrftoken cookie 的值，而非 CSRF-TOKEN 请求头",
                csrf.chars().count()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod cookie_parse_tests {
    use super::*;

    #[test]
    fn parses_full_browser_cookie_string() {
        // 这是用户实际会粘贴的形态：从开发者工具复制的整段 Cookie。
        let blob = "LEETCODE_SESSION=eyJhbGciOiJIUzI1NiJ9.abcdefg; \
                    csrftoken=AbCdEfGhIjKlMnOpQrStUvWxYz012345; _gid=GA1.2.999; \
                    _ga=GA1.1.888";
        let (session, csrf) = parse_cookie_blob(blob);
        assert_eq!(session.as_deref(), Some("eyJhbGciOiJIUzI1NiJ9.abcdefg"));
        assert_eq!(csrf.as_deref(), Some("AbCdEfGhIjKlMnOpQrStUvWxYz012345"));
    }

    #[test]
    fn parsing_is_order_independent_and_case_insensitive() {
        let blob = "csrftoken=AbCdEfGhIjKlMnOpQrStUvWxYz012345; leetcode_session=token123";
        let (session, csrf) = parse_cookie_blob(blob);
        assert_eq!(session.as_deref(), Some("token123"));
        assert_eq!(csrf.as_deref(), Some("AbCdEfGhIjKlMnOpQrStUvWxYz012345"));
    }

    #[test]
    fn tolerates_whitespace_and_quotes() {
        let blob = "  LEETCODE_SESSION = \"abc123\" ;  csrftoken = 'xyz'  ";
        let (session, csrf) = parse_cookie_blob(blob);
        assert_eq!(session.as_deref(), Some("abc123"));
        assert_eq!(csrf.as_deref(), Some("xyz"));
    }

    #[test]
    fn ignores_unrelated_cookies() {
        let blob = "_gid=1; _gat=2; intercom=abc";
        let (session, csrf) = parse_cookie_blob(blob);
        assert!(session.is_none(), "无关 cookie 不应被当作 session");
        assert!(csrf.is_none(), "无关 cookie 不应被当作 csrftoken");
    }

    #[test]
    fn handles_empty_and_garbage_input() {
        assert_eq!(parse_cookie_blob(""), (None, None));
        assert_eq!(parse_cookie_blob("   "), (None, None));
        assert_eq!(parse_cookie_blob(";;;"), (None, None));
        // 裸值兜底：长串判定为 session，短串判定为 csrftoken。
        let (s, c) = parse_cookie_blob("short");
        assert!(s.is_none());
        assert_eq!(c.as_deref(), Some("short"));
    }

    #[test]
    fn validates_realistic_session() {
        let s = LeetCodeSession {
            session_cookie: "eyJhbGciOiJIUzI1NiJ9.long_token_value".into(),
            csrf_token: "AbCdEfGhIjKlMnOpQrStUvWxYz012345".into(),
            site: LeetCodeSite::Cn,
        };
        assert!(s.validate().is_ok(), "正常的 32 位 csrftoken 应通过");
        assert_eq!(s.site.host(), "leetcode.cn");
    }

    #[test]
    fn rejects_pasted_whole_cookie_in_single_field() {
        // 用户在单个输入框里粘了整段 Cookie —— 这是"无效"的常见真实成因。
        let s = LeetCodeSession {
            session_cookie: "LEETCODE_SESSION=abc; csrftoken=def".into(),
            csrf_token: "AbCdEfGhIjKlMnOpQrStUvWxYz012345".into(),
            site: LeetCodeSite::Com,
        };
        let err = s.validate().unwrap_err();
        assert!(err.contains("分号"), "应提示含分号并引导自动提取，实际: {err}");
    }

    #[test]
    fn rejects_malformed_csrf_token_with_actionable_message() {
        // 复制成 CSRF-TOKEN 头（大写短横线形式）会得到明显不同的值。
        let s = LeetCodeSession {
            session_cookie: "token".into(),
            csrf_token: "not-32-chars".into(),
            site: LeetCodeSite::Com,
        };
        let err = s.validate().unwrap_err();
        assert!(err.contains("32 位"), "应说明期望长度，实际: {err}");
    }

    #[test]
    fn site_endpoints_are_distinct() {
        assert_eq!(
            LeetCodeSite::Com.graphql_endpoint(),
            "https://leetcode.com/graphql"
        );
        assert_eq!(
            LeetCodeSite::Cn.graphql_endpoint(),
            "https://leetcode.cn/graphql"
        );
        assert_eq!(LeetCodeSite::Com.host(), "leetcode.com");
        assert_eq!(LeetCodeSite::Cn.host(), "leetcode.cn");
        assert_ne!(
            LeetCodeSite::Com.host(),
            LeetCodeSite::Cn.host(),
            "两站域名必须不同——这正是凭据不可混用的原因"
        );
    }

    #[test]
    fn old_config_without_site_defaults_to_com() {
        // 向后兼容：历史配置文件没有 site 字段，必须能正常反序列化。
        let json = r#"{"session_cookie":"a","csrf_token":"b"}"#;
        let s: LeetCodeSession = serde_json::from_str(json).unwrap();
        assert_eq!(s.site, LeetCodeSite::Com, "旧配置应默认国际站");
    }
}

/// 打码敏感字符串，只保留首尾少量字符以便用户确认自己填对了哪个。
///
/// 对过短的输入完全隐藏，避免"打码"反而暴露全部内容。
pub fn mask_secret(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return "<未设置>".to_string();
    }
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars
        .iter()
        .skip(chars.len() - 4)
        .collect::<Vec<_>>()
        .into_iter()
        .collect();
    format!("{head}...{tail} (共 {} 位)", chars.len())
}

/// 完整的应用配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    /// 已绑定的 LeetCode 用户名。空表示尚未绑定。
    pub username: String,
    #[serde(default)]
    pub session: LeetCodeSession,
    #[serde(default)]
    pub llm: LlmConfig,
    /// 首页每页显示条数。
    pub page_size: usize,
    /// 界面主题（浅色 / 深色）。
    ///
    /// `#[serde(default)]` 是**硬要求**：老版本落盘的 `app_config` JSON
    /// 里没有这个字段，缺省标注让反序列化成功，从而避开
    /// `storage::load_config` 的"解析失败 → 重置全部配置 + 弹提示"路径。
    /// 用户不该因为升级而丢失已填好的凭据。
    #[serde(default)]
    pub theme: crate::ui::theme::ThemeMode,
    /// 是否启用高级玻璃质感（侧边栏、顶栏、卡片、按钮的通透效果）。
    ///
    /// 同样需要 `#[serde(default)]`。默认 `true`——高级质感开箱即用。
    #[serde(default = "default_glass_effect")]
    pub glass_effect: bool,
}

/// `glass_effect` 的缺省值：启用。
///
/// 用函数而非 `Default::default()`，因为 `bool` 的默认是 `false`，
/// 而这里要的是"老配置也升级为开启"。
fn default_glass_effect() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            session: LeetCodeSession::default(),
            llm: LlmConfig::default(),
            page_size: 100,
            theme: crate::ui::theme::ThemeMode::default(),
            glass_effect: true,
        }
    }
}

impl AppConfig {
    /// 是否已完成账号绑定（仅用户名，不含凭据）。
    pub fn has_account(&self) -> bool {
        !self.username.trim().is_empty()
    }

    /// 是否处于认证增强模式。
    pub fn is_authenticated(&self) -> bool {
        self.session.is_complete()
    }

    /// 凭据指纹：用于判断"凭据是否被改过"。
    ///
    /// ## 用途
    ///
    /// 会话校验结果会持久化，重启后直接恢复，用户不必每次打开都点一次
    /// 「校验凭据」。但持久化的结果只对**校验时的那套凭据**有效——
    /// 用户换了账号、改了 Cookie 或切换了站点，旧的校验结论就必须作废。
    ///
    /// 因此把凭据内容折算成一个短指纹一并存下，启动时比对：
    /// 一致则沿用结论，不一致则视为未校验。
    ///
    /// ## 为什么用哈希而不是原值
    ///
    /// 直接存原值等于把 Cookie 又复制了一份到数据库（本就有明文存储，
    /// 再存一份只会扩大暴露面）。指纹只需**可比对**，不需要可还原。
    ///
    /// ## 注意
    ///
    /// 这是**变更检测**而非安全机制：`DefaultHasher` 不抗碰撞，也不保证
    /// 跨版本稳定。跨版本变化最多导致一次多余的重校验，属可接受代价。
    /// 真正的安全边界是数据库文件本身的访问权限。
    pub fn credential_fingerprint(&self) -> String {
        use std::hash::{Hash, Hasher};

        let mut h = std::collections::hash_map::DefaultHasher::new();
        // 站点参与指纹：同一账号在两站是不同身份，切站必须重新校验。
        self.session.site.label_zh().hash(&mut h);
        self.session.session_cookie.hash(&mut h);
        self.session.csrf_token.hash(&mut h);
        format!("{:016x}", h.finish())
    }

    /// 规范化用户输入：去除首尾空白，防止粘贴时带入空格导致请求失败。
    pub fn normalize(&mut self) {
        self.username = self.username.trim().to_string();
        self.session.session_cookie = self.session.session_cookie.trim().to_string();
        self.session.csrf_token = self.session.csrf_token.trim().to_string();
        self.llm.base_url = self.llm.base_url.trim().trim_end_matches('/').to_string();
        self.llm.model = self.llm.model.trim().to_string();
        self.llm.api_key = self.llm.api_key.trim().to_string();
        if self.page_size == 0 {
            self.page_size = 100;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_hides_short_secrets_entirely() {
        assert_eq!(mask_secret("short"), "*****");
        assert_eq!(mask_secret(""), "<未设置>");
        assert_eq!(mask_secret("   "), "<未设置>");
    }

    #[test]
    fn mask_reveals_only_edges_for_long_secrets() {
        let m = mask_secret("abcdefghijklmnop");
        assert!(m.starts_with("abcd"));
        assert!(m.contains("mnop"));
        assert!(!m.contains("ijkl"), "中间部分必须被打码");
    }

    #[test]
    fn debug_impl_does_not_leak_api_key() {
        let cfg = LlmConfig {
            api_key: "sk-super-secret-key-1234567890".into(),
            ..Default::default()
        };
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("sk-super-secret-key-1234567890"),
            "Debug 输出泄露了完整 API Key: {rendered}"
        );
    }

    #[test]
    fn debug_impl_does_not_leak_session_cookie() {
        let s = LeetCodeSession {
            session_cookie: "eyJhbGciOiJIUzI1NiJ9.verysecretpayload".into(),
            csrf_token: "abcdef1234567890abcdef1234567890".into(),
            site: LeetCodeSite::Com,
        };
        let rendered = format!("{s:?}");
        assert!(!rendered.contains("verysecretpayload"));
        assert!(!rendered.contains("abcdef1234567890abcdef1234567890"));
    }

    #[test]
    fn llm_validate_rejects_plain_http_for_remote_hosts() {
        let cfg = LlmConfig {
            base_url: "http://api.example.com/v1".into(),
            api_key: "k".into(),
            model: "m".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err(), "远程明文 HTTP 必须被拒绝");
    }

    #[test]
    fn llm_validate_allows_local_http() {
        for url in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1234/v1",
            "http://[::1]:8080/v1",
        ] {
            let cfg = LlmConfig {
                base_url: url.into(),
                api_key: "not-needed".into(),
                model: "llama3".into(),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "{url} 应当被接受");
        }
    }

    #[test]
    fn llm_validate_rejects_out_of_range_temperature() {
        let cfg = LlmConfig {
            api_key: "k".into(),
            model: "m".into(),
            temperature: 3.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn llm_validate_accepts_missing_api_key() {
        // 缺陷回归：早期 `validate()` 无条件要求 API Key 非空，
        // 而调用方又用"Base URL / Model 是否非空"判断用户是否想启用 AI——
        // 这两者有默认值恒非空，导致用户没填 Key 点保存也被拒绝。
        //
        // 现在契约明确：API Key 为空 = 不启用 AI = 合法。
        let cfg = LlmConfig::default();
        assert!(cfg.api_key.trim().is_empty(), "前提：默认无 Key");
        assert!(
            cfg.validate().is_ok(),
            "未填写 API Key 时应允许保存（表示不启用 AI）"
        );
        assert!(!cfg.is_usable(), "但此时不应被判定为可用");
    }

    #[test]
    fn llm_validate_still_checks_other_fields_when_key_absent() {
        // 未填 Key 不等于可以填非法的 URL——避免留下"看着能用实际必失败"的状态。
        let cfg = LlmConfig {
            base_url: "http://api.example.com/v1".into(),
            api_key: String::new(),
            ..Default::default()
        };
        assert!(
            cfg.validate().is_err(),
            "即使未填 Key，远程明文 HTTP 仍应被拒绝"
        );
    }

    #[test]
    fn llm_validate_requires_model_when_key_present() {
        // 填了 Key 就要求配置完整，否则调用必然失败。
        let cfg = LlmConfig {
            api_key: "sk-test".into(),
            model: "  ".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err(), "填了 Key 但模型为空应报错");
    }

    #[test]
    fn chat_endpoint_appends_path_once() {
        let mut cfg = LlmConfig {
            base_url: "https://api.openai.com/v1".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.chat_endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
        // 用户误填完整路径时不应重复追加。
        cfg.base_url = "https://api.openai.com/v1/chat/completions".into();
        assert_eq!(
            cfg.chat_endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
        // 末尾斜杠也应被正确处理。
        cfg.base_url = "https://api.deepseek.com/v1/".into();
        assert_eq!(
            cfg.chat_endpoint(),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn session_validate_rejects_multiline_paste() {
        let s = LeetCodeSession {
            session_cookie: "abc\ndef".into(),
            csrf_token: "abcdef1234567890abcdef1234567890".into(),
            site: LeetCodeSite::Com,
        };
        let err = s.validate().unwrap_err();
        assert!(err.contains("换行符"));
    }

    #[test]
    fn session_is_complete_requires_both_fields() {
        let mut s = LeetCodeSession::default();
        assert!(!s.is_complete());
        assert!(s.is_empty());
        s.session_cookie = "x".into();
        assert!(!s.is_complete(), "仅有 session 不算完整");
        s.csrf_token = "y".into();
        assert!(s.is_complete());
        assert!(!s.is_empty());
    }

    #[test]
    fn normalize_strips_whitespace_and_trailing_slash() {
        let mut cfg = AppConfig {
            username: "  john  ".into(),
            llm: LlmConfig {
                base_url: " https://api.openai.com/v1/// ".into(),
                ..Default::default()
            },
            page_size: 0,
            ..Default::default()
        };
        cfg.normalize();
        assert_eq!(cfg.username, "john");
        assert_eq!(cfg.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(cfg.page_size, 100, "非法分页值应回退到默认");
    }

    #[test]
    fn default_config_has_no_account_and_no_auth() {
        let cfg = AppConfig::default();
        assert!(!cfg.has_account());
        assert!(!cfg.is_authenticated());
    }

    // -----------------------------------------------------------------------
    // 主题与玻璃设置的向后兼容
    // -----------------------------------------------------------------------

    /// **本组测试对应一个真实的升级风险。**
    ///
    /// `theme` / `glass_effect` 是后加的字段，老版本落盘的 JSON 里没有它们。
    /// 若忘记 `#[serde(default)]`，反序列化会失败，而 `storage::load_config`
    /// 对失败的处理是"重置为默认配置 + 提示用户重新填写"——
    /// 用户升级一次就丢掉全部凭据（Cookie、API Key），代价极大。
    #[test]
    fn config_without_theme_fields_still_deserializes() {
        // 完全模拟老版本的 JSON：没有 theme 与 glass_effect。
        let legacy = r#"{
            "username": "wuhu",
            "session": {
                "session_cookie": "sess-value",
                "csrf_token": "csrf-value",
                "site": "Cn"
            },
            "llm": {
                "provider": "OpenAiCompatible",
                "base_url": "https://api.openai.com/v1",
                "model": "gpt-4o-mini",
                "api_key": "sk-xxx",
                "temperature": 0.3,
                "timeout_secs": 60,
                "max_history_turns": 10
            },
            "page_size": 100
        }"#;

        let cfg: AppConfig =
            serde_json::from_str(legacy).expect("老配置必须能反序列化，否则升级会丢数据");

        // 关键：用户原有的凭据必须完好保留。
        assert_eq!(cfg.username, "wuhu");
        assert_eq!(cfg.session.session_cookie, "sess-value");
        assert_eq!(cfg.llm.api_key, "sk-xxx");
        assert_eq!(cfg.session.site, LeetCodeSite::Cn);

        // 新字段取默认值。
        assert_eq!(cfg.theme, crate::ui::theme::ThemeMode::Dark);
        assert!(cfg.glass_effect, "老配置升级后应默认启用玻璃质感");
    }

    /// 主题与玻璃设置必须能往返持久化。
    #[test]
    fn theme_and_glass_roundtrip_through_json() {
        for theme in [
            crate::ui::theme::ThemeMode::Dark,
            crate::ui::theme::ThemeMode::Light,
        ] {
            for glass in [true, false] {
                let cfg = AppConfig {
                    theme,
                    glass_effect: glass,
                    ..AppConfig::default()
                };
                let json = serde_json::to_string(&cfg).unwrap();
                let back: AppConfig = serde_json::from_str(&json).unwrap();
                assert_eq!(back.theme, theme);
                assert_eq!(back.glass_effect, glass);
            }
        }
    }

    /// 玻璃质感的缺省值必须是 `true`。
    ///
    /// 这条单独测是因为 `bool::default()` 是 `false`——若写成
    /// `#[serde(default)]` 而非 `#[serde(default = "...")]`，
    /// 老配置会被静默降级为"关闭玻璃"，与"开箱即用"的预期相反。
    #[test]
    fn glass_defaults_to_enabled() {
        assert!(default_glass_effect());
        assert!(AppConfig::default().glass_effect);
    }

    /// 老配置反序列化后**不得**触发指纹变化以外的副作用。
    ///
    /// 具体说：新增字段不能影响凭据指纹，否则用户升级后会被判定为
    /// "凭据已变"，从而作废已持久化的校验结论，被迫重新校验。
    #[test]
    fn new_theme_fields_do_not_affect_credential_fingerprint() {
        let base = cfg_with("sess", "csrf", LeetCodeSite::Com);
        let themed = AppConfig {
            theme: crate::ui::theme::ThemeMode::Light,
            glass_effect: false,
            ..base.clone()
        };
        assert_eq!(
            base.credential_fingerprint(),
            themed.credential_fingerprint(),
            "主题与玻璃设置与凭据无关，不应影响指纹"
        );
    }

    // -----------------------------------------------------------------------
    // 凭据指纹
    // -----------------------------------------------------------------------

    fn cfg_with(cookie: &str, csrf: &str, site: LeetCodeSite) -> AppConfig {
        AppConfig {
            username: "wuhu".into(),
            session: LeetCodeSession {
                session_cookie: cookie.into(),
                csrf_token: csrf.into(),
                site,
            },
            ..AppConfig::default()
        }
    }

    /// 同一套凭据必须得到同一指纹——否则每次启动都会判为"凭据已变"，
    /// 用户依旧要反复重校验，持久化就白做了。
    #[test]
    fn fingerprint_is_stable_for_identical_credentials() {
        let a = cfg_with("sess", "csrf", LeetCodeSite::Cn);
        let b = cfg_with("sess", "csrf", LeetCodeSite::Cn);
        assert_eq!(a.credential_fingerprint(), b.credential_fingerprint());
        assert!(!a.credential_fingerprint().is_empty());
    }

    /// 凭据任一组成部分变化，指纹必须随之改变。
    ///
    /// 否则会沿用一份属于**另一套凭据**的校验结论，界面显示"已校验"
    /// 而实际请求用的是别的身份——这种"状态与事实不符"比要求重校验更糟。
    #[test]
    fn fingerprint_changes_when_any_credential_part_changes() {
        let base = cfg_with("sess", "csrf", LeetCodeSite::Cn);

        let other_cookie = cfg_with("sess2", "csrf", LeetCodeSite::Cn);
        let other_csrf = cfg_with("sess", "csrf2", LeetCodeSite::Cn);
        // 同一账号在两站是不同身份，切站必须重新校验。
        let other_site = cfg_with("sess", "csrf", LeetCodeSite::Com);

        assert_ne!(base.credential_fingerprint(), other_cookie.credential_fingerprint());
        assert_ne!(base.credential_fingerprint(), other_csrf.credential_fingerprint());
        assert_ne!(base.credential_fingerprint(), other_site.credential_fingerprint());
    }

    /// 指纹不得等于任何一段原始凭据——它只是变更检测，不该成为
    /// 凭据的又一份明文副本。
    #[test]
    fn fingerprint_does_not_leak_raw_credential() {
        let cfg = cfg_with("super-secret-session-value", "csrf-value", LeetCodeSite::Cn);
        let fp = cfg.credential_fingerprint();
        assert!(!fp.contains("super-secret-session-value"));
        assert!(!fp.contains("csrf-value"));
    }
}
