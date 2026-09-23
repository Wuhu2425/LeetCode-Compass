//! 领域模型。
//!
//! 这一层是与 LeetCode API 解耦的"应用内部语言"。`leetcode::types` 中的 DTO
//! 负责贴合远端 schema，本模块负责表达应用真正关心的概念。两者通过 `From`
//! 实现转换，从而把 API 变更的影响限制在单个文件内。
//!
//! 设计原则：所有可能缺失的字段一律使用 `Option`，因为 LeetCode 的
//! `status` 等字段在未认证时会返回 `null`。绝不使用 `unwrap` 假设字段存在。

// 领域模型提供完整语义 API；部分方法暂未被 UI 调用。
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// 题目难度。
///
/// LeetCode 原样返回字符串，这里做穷举映射以避免散落的字符串比较。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

impl Difficulty {
    /// 从 API 字符串解析，无法识别时返回 `None` 而非 panic。
    ///
    /// **大小写不敏感**，这是刻意的：国际站返回首字母大写（`"Easy"`），
    /// 中国站返回全大写（`"EASY"`）。两站的枚举写法不同，
    /// 若不兼容会导致中国站所有题目的难度静默降级为 `Easy`——
    /// 这类错误不会报错，只会让数据变得静默错误，极难发现。
    pub fn parse(s: &str) -> Option<Self> {
        let t = s.trim();
        if t.eq_ignore_ascii_case("easy") {
            Some(Self::Easy)
        } else if t.eq_ignore_ascii_case("medium") {
            Some(Self::Medium)
        } else if t.eq_ignore_ascii_case("hard") {
            Some(Self::Hard)
        } else {
            None
        }
    }

    /// 转为 API 使用的字符串。
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::Easy => "EASY",
            Self::Medium => "MEDIUM",
            Self::Hard => "HARD",
        }
    }

    /// 用于界面展示的中文标签。
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Easy => "简单",
            Self::Medium => "中等",
            Self::Hard => "困难",
        }
    }

    /// 相对难度权重，用于推荐打分。
    pub fn weight(self) -> f64 {
        match self {
            Self::Easy => 1.0,
            Self::Medium => 1.6,
            Self::Hard => 2.4,
        }
    }

    /// 固定顺序，用于确定性排序。
    pub fn order(self) -> u8 {
        match self {
            Self::Easy => 0,
            Self::Medium => 1,
            Self::Hard => 2,
        }
    }

    /// 全部难度，用于遍历。
    pub fn all() -> [Difficulty; 3] {
        [Self::Easy, Self::Medium, Self::Hard]
    }
}

impl std::fmt::Display for Difficulty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label_zh())
    }
}

/// 用户在单道题目上的完成状态。
///
/// 这是应用中最容易出错的状态之一：`Unknown` 表示"因为未配置凭据而无法
/// 判定"，与 `Todo`（确定没做过）语义不同。UI 与推荐算法必须区分两者。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SolveStatus {
    /// 已通过（Accepted）。
    Solved,
    /// 尝试过但未通过。
    Attempted,
    /// 已确认未尝试。
    Todo,
    /// 无法判定（未认证时 API 返回 null）。
    Unknown,
}

impl SolveStatus {
    /// 从 API 的 `status` 字段解析。
    ///
    /// `null` 映射为 `Unknown`，因为无法区分"未登录"与"确实没做过"。
    pub fn from_api(s: Option<&str>) -> Self {
        match s {
            Some("AC") => Self::Solved,
            Some("TRIED") => Self::Attempted,
            Some("NOT_STARTED") => Self::Todo,
            _ => Self::Unknown,
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Solved => "已通过",
            Self::Attempted => "尝试过",
            Self::Todo => "未开始",
            Self::Unknown => "未知",
        }
    }

    /// 是否可确定已完成。推荐算法用它过滤，绝不能用 `!= Todo`。
    pub fn is_solved(self) -> bool {
        matches!(self, Self::Solved)
    }
}

impl std::fmt::Display for SolveStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label_zh())
    }
}

/// 题目标签（知识点）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TopicTag {
    /// 展示名，例如 "Hash Table"。
    pub name: String,
    /// URL 友好的标识，例如 "hash-table"。用于与 API 的 filters 交互。
    pub slug: String,
}

/// 一道题目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    /// 站内原始 ID（内部标识，非展示用）。
    pub question_id: String,
    /// 用户可见的题号，例如 "1"。注意是字符串，可能含非数字。
    pub frontend_id: String,
    pub title: String,
    /// URL 标识，用于构造跳转链接。
    pub title_slug: String,
    pub difficulty: Difficulty,
    /// 通过率，API 返回的是百分数（如 57.9 表示 57.9%）。
    pub ac_rate: f64,
    pub tags: Vec<TopicTag>,
    pub status: SolveStatus,
    /// 是否为会员专属题。会员题应在 UI 中标记，并降低推荐权重。
    pub is_paid_only: bool,
}

impl Problem {
    /// 构造 LeetCode 题目页 URL。
    ///
    /// 站点随登录会话走：会话是中国站就构造 `leetcode.cn`，是国际站就
    /// 构造 `leetcode.com`。两站的账号体系与 Cookie 域完全独立，题目
    /// slug 基本一致（个别题目两站集合不同，跳到不存在的页面属远端
    /// 数据差异，非本方法职责）。
    ///
    /// 注意：`title_slug` 来自远端，此处仅做最小限度的白名单校验，避免
    /// 将任意字符串拼进 URL 后交给系统浏览器打开。
    pub fn url(&self, site: crate::config::LeetCodeSite) -> Option<String> {
        if !is_safe_slug(&self.title_slug) {
            return None;
        }
        Some(format!(
            "https://{}/problems/{}/",
            site.host(),
            self.title_slug
        ))
    }

    /// 题号的可排序数值形式。非数字题号（如某些特殊题）排到最后。
    pub fn frontend_id_numeric(&self) -> u32 {
        self.frontend_id.parse().unwrap_or(u32::MAX)
    }
}

/// 校验 slug 是否只含 URL 安全字符。
///
/// 防御性措施：防止远端数据异常时把 `../` 或 `?` 等字符带入 URL。
fn is_safe_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 128
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 按难度分组的解题数量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DifficultyCounts {
    pub easy: u32,
    pub medium: u32,
    pub hard: u32,
}

impl DifficultyCounts {
    pub fn total(&self) -> u32 {
        self.easy + self.medium + self.hard
    }

    /// 按难度取值的辅助方法。
    pub fn get(&self, d: Difficulty) -> u32 {
        match d {
            Difficulty::Easy => self.easy,
            Difficulty::Medium => self.medium,
            Difficulty::Hard => self.hard,
        }
    }
}

/// 用户在某个标签上的解题统计。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagProgress {
    pub tag_name: String,
    pub tag_slug: String,
    /// 已解出的题目数。
    pub solved: u32,
    /// 该标签下题库总量（由本地题库统计得出，可能滞后于远端）。
    pub total: u32,
}

impl TagProgress {
    /// 标签掌握率，范围 0.0..=1.0。`total` 为 0 时返回 0.0 以避免除零。
    pub fn mastery(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.solved as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }
}

/// 用户账号画像。
///
/// 所有字段均可缺失：部分信息仅在认证后可见，且不同账号的"已填写"程度不同。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserProfile {
    pub username: String,
    pub real_name: Option<String>,
    /// 全局排名。
    pub ranking: Option<u32>,
    pub reputation: Option<i32>,
    /// 总提交数（含未通过）。
    pub total_submissions: Option<u32>,
    /// 按难度的已解题数。
    pub solved: DifficultyCounts,
    /// 按难度的尝试失败数。
    pub failed: DifficultyCounts,
    /// 提交日历：日期（yyyy-MM-dd）到当日提交次数的映射。
    pub submission_calendar: Vec<(String, u32)>,
    /// 连续活跃天数。
    pub streak: Option<u32>,
    /// 累计活跃天数。
    pub total_active_days: Option<u32>,
    /// 最近一次成功拉取画像的时间戳（Unix 秒）。
    pub fetched_at: i64,
    /// 当前数据是否来自认证会话（true）或仅公开接口（false）。
    pub authenticated: bool,
}

impl UserProfile {
    /// 校验用户名是否可用。
    ///
    /// **不限制字符集**：LeetCode 同时支持国际站（leetcode.com）与中国站
    /// （leetcode.cn），后者允许中文昵称。早期版本硬性要求 ASCII
    /// 字母数字，导致中文用户名无法保存。
    ///
    /// 现在改为「黑名单」策略——只拒绝真正会造成问题的字符：
    /// - 控制字符（含换行/制表）：会破坏 URL 与 HTTP 头；
    /// - 路径分隔符与 URL 元字符：会破坏 URL 拼接；
    /// - 首尾空白由 `trim` 处理，中间空白一并拒绝（用户名不含空格）。
    ///
    /// 长度按**字符数**计（非字节数），否则中文名会在约 10 字时误报超长。
    pub fn validate_username(name: &str) -> Result<(), String> {
        let n = name.trim();
        if n.is_empty() {
            return Err("用户名不能为空".into());
        }
        // 按字符计长。中文名 30 字符是合理的宽限量。
        if n.chars().count() > 30 {
            return Err("用户名长度不能超过 30 个字符".into());
        }
        if let Some(bad) = n.chars().find(|c| {
            c.is_control()
                // URL 元字符与路径分隔符：会破坏 slug 拼接与请求构造
                || matches!(c, '/' | '\\' | '?' | '#' | '&' | '=' | '%' | '+' | '"' | '\'' | '<' | '>')
                || c.is_whitespace()
        }) {
            return Err(format!(
                "用户名不能包含空白、控制字符或 URL 特殊字符（发现 {bad:?}）"
            ));
        }
        Ok(())
    }
}

/// 单场竞赛记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContestRecord {
    pub title: String,
    /// 竞赛开始时间（Unix 秒）。
    pub start_time: i64,
    /// 该场 Rating。
    pub rating: f64,
    /// 该场名次。
    pub ranking: u32,
    /// 总参赛人数。
    pub total_participants: u32,
    /// 解出题数。
    pub problems_solved: u32,
    /// 总题数。
    pub total_problems: u32,
    /// 是否真正参赛（false 表示仅注册未参与）。
    pub attended: bool,
}

impl ContestRecord {
    /// 该场的前百分位（越小越好）。人数为 0 时返回 `None` 避免除零。
    pub fn percentile(&self) -> Option<f64> {
        if self.total_participants == 0 {
            return None;
        }
        Some(self.ranking as f64 / self.total_participants as f64 * 100.0)
    }
}

/// 竞赛总体排名摘要。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContestSummary {
    /// 参与场次。
    pub attended_count: u32,
    /// 当前 Rating。
    pub rating: Option<f64>,
    /// 全局排名。
    pub global_ranking: Option<u32>,
    pub total_participants: Option<u32>,
    pub top_percentage: Option<f64>,
}

/// 推荐结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    pub problem: Problem,
    /// 综合得分，越高越优先。
    pub score: f64,
    /// 人类可读的推荐理由。
    pub reasons: Vec<String>,
    /// 命中的薄弱标签。
    pub matched_tags: Vec<String>,
}

/// 一条聊天消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// 该消息是否为错误提示（UI 用不同颜色渲染）。
    pub is_error: bool,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
            is_error: false,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
            is_error: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

impl ChatRole {
    pub fn as_api_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// 题库筛选条件。
#[derive(Debug, Clone, Default)]
pub struct ProblemFilter {
    /// 空表示不限制。
    pub difficulties: Vec<Difficulty>,
    /// 空表示不限制。按 slug 匹配。
    pub tags: Vec<String>,
    /// 空表示不限制。
    pub statuses: Vec<SolveStatus>,
    /// 关键词，匹配题号或标题（不区分大小写）。
    pub keyword: String,
    /// 是否隐藏会员专属题。
    pub hide_paid_only: bool,
}

impl ProblemFilter {
    /// 判断单道题目是否满足全部条件。各条件之间是"与"关系。
    pub fn matches(&self, p: &Problem) -> bool {
        if !self.difficulties.is_empty() && !self.difficulties.contains(&p.difficulty) {
            return false;
        }
        if !self.statuses.is_empty() && !self.statuses.contains(&p.status) {
            return false;
        }
        if self.hide_paid_only && p.is_paid_only {
            return false;
        }
        // 标签采用"命中其一"语义：用户勾选多个标签时通常希望看到
        // 覆盖任一选中知识点的题目，而非要求同时覆盖全部标签。
        if !self.tags.is_empty()
            && !p
                .tags
                .iter()
                .any(|t| self.tags.iter().any(|sel| sel.eq_ignore_ascii_case(&t.slug)))
        {
            return false;
        }
        if !self.keyword.is_empty() {
            let kw = self.keyword.trim().to_lowercase();
            let hit = p.frontend_id == kw
                || p.title.to_lowercase().contains(&kw)
                || p.title_slug.to_lowercase().contains(&kw);
            if !hit {
                return false;
            }
        }
        true
    }

    /// 是否处于"未设置任何条件"的初始状态。
    pub fn is_empty(&self) -> bool {
        self.difficulties.is_empty()
            && self.tags.is_empty()
            && self.statuses.is_empty()
            && self.keyword.trim().is_empty()
            && !self.hide_paid_only
    }
}

/// 题库排序方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortBy {
    /// 按题号升序（默认，符合"顺序列出全部题目"的需求）。
    #[default]
    FrontendId,
    /// 按难度从易到难。
    Difficulty,
    /// 按通过率从高到低（先做容易过的）。
    AcRateDesc,
    /// 按通过率从低到高（挑战性优先）。
    AcRateAsc,
}

impl SortBy {
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::FrontendId => "题号顺序",
            Self::Difficulty => "难度递进",
            Self::AcRateDesc => "通过率降序",
            Self::AcRateAsc => "通过率升序",
        }
    }

    pub fn all() -> [SortBy; 4] {
        [
            Self::FrontendId,
            Self::Difficulty,
            Self::AcRateDesc,
            Self::AcRateAsc,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_problem(slug: &str, status: SolveStatus) -> Problem {
        Problem {
            question_id: "1".into(),
            frontend_id: "1".into(),
            title: "Two Sum".into(),
            title_slug: slug.into(),
            difficulty: Difficulty::Easy,
            ac_rate: 57.9,
            tags: vec![TopicTag {
                name: "Array".into(),
                slug: "array".into(),
            }],
            status,
            is_paid_only: false,
        }
    }

    #[test]
    fn status_null_maps_to_unknown_not_todo() {
        // 这是最关键的一条语义：null 不可等同于"未做"。
        assert_eq!(SolveStatus::from_api(None), SolveStatus::Unknown);
        assert_ne!(SolveStatus::from_api(None), SolveStatus::Todo);
    }

    #[test]
    fn status_ac_maps_to_solved() {
        assert_eq!(SolveStatus::from_api(Some("AC")), SolveStatus::Solved);
        assert!(SolveStatus::from_api(Some("AC")).is_solved());
        assert!(!SolveStatus::Unknown.is_solved());
    }

    #[test]
    fn unknown_difficulty_returns_none() {
        assert_eq!(Difficulty::parse("Extreme"), None);
        assert_eq!(Difficulty::parse(""), None);
        assert_eq!(Difficulty::parse("Hard"), Some(Difficulty::Hard));
    }

    /// 难度解析必须**大小写不敏感**。
    ///
    /// 国际站返回首字母大写（`"Easy"`），中国站返回全大写（`"EASY"`）。
    /// 若只认一种写法，另一站的所有题目难度会静默降级为 `Easy`——
    /// 不报错、不崩溃，只是数据全错，属于最难发现的缺陷类型。
    ///
    /// 注意：此测试**故意**替换了早期一条断言 `parse("easy") == None`，
    /// 那条断言把"大小写敏感"这一缺陷固化了。
    #[test]
    fn difficulty_parse_is_case_insensitive_for_two_sites() {
        for (input, expected) in [
            ("Easy", Difficulty::Easy),
            ("easy", Difficulty::Easy),
            ("EASY", Difficulty::Easy),
            ("Medium", Difficulty::Medium),
            ("MEDIUM", Difficulty::Medium),
            ("medium", Difficulty::Medium),
            ("Hard", Difficulty::Hard),
            ("HARD", Difficulty::Hard),
            ("  Easy  ", Difficulty::Easy), // 容忍空白
        ] {
            assert_eq!(
                Difficulty::parse(input),
                Some(expected),
                "{input:?} 应解析为 {expected:?}（两站枚举写法不同）"
            );
        }
    }

    #[test]
    fn url_rejects_unsafe_slug() {
        let com = crate::config::LeetCodeSite::Com;
        assert!(mk_problem("two-sum", SolveStatus::Solved).url(com).is_some());
        assert!(mk_problem("../../etc/passwd", SolveStatus::Solved)
            .url(com)
            .is_none());
        assert!(mk_problem("a?b=c", SolveStatus::Solved).url(com).is_none());
        assert!(mk_problem("", SolveStatus::Solved).url(com).is_none());
        assert!(mk_problem("has space", SolveStatus::Solved)
            .url(com)
            .is_none());
    }

    #[test]
    fn url_follows_the_session_site() {
        use crate::config::LeetCodeSite;
        let p = mk_problem("two-sum", SolveStatus::Solved);
        // 登录会话是国内站 → 跳国内题库；是国际站 → 跳国际题库。
        // 两个站点共用同一套 slug，只有域名不同。
        assert_eq!(
            p.url(LeetCodeSite::Cn).as_deref(),
            Some("https://leetcode.cn/problems/two-sum/")
        );
        assert_eq!(
            p.url(LeetCodeSite::Com).as_deref(),
            Some("https://leetcode.com/problems/two-sum/")
        );
        // slug 白名单校验对两站一视同仁。
        assert!(mk_problem("a b", SolveStatus::Solved)
            .url(LeetCodeSite::Cn)
            .is_none());
    }

    #[test]
    fn frontend_id_sorting_handles_non_numeric() {
        let mut p = mk_problem("x", SolveStatus::Todo);
        p.frontend_id = "面试题 01.01".into();
        assert_eq!(p.frontend_id_numeric(), u32::MAX);
        p.frontend_id = "42".into();
        assert_eq!(p.frontend_id_numeric(), 42);
    }

    #[test]
    fn tag_mastery_avoids_division_by_zero() {
        let t = TagProgress {
            tag_name: "DP".into(),
            tag_slug: "dynamic-programming".into(),
            solved: 5,
            total: 0,
        };
        assert_eq!(t.mastery(), 0.0);
    }

    #[test]
    fn username_validation() {
        assert!(UserProfile::validate_username("john_doe-1").is_ok());
        assert!(UserProfile::validate_username("").is_err());
        assert!(UserProfile::validate_username("has space").is_err());
        assert!(UserProfile::validate_username("a".repeat(31).as_str()).is_err());
    }

    #[test]
    fn username_accepts_non_ascii_and_cjk() {
        // LeetCode 中国站允许中文昵称。早期版本硬性要求 ASCII，导致中文用户名
        // 无法保存——这是用户实际反馈的缺陷。此处固化为回归测试。
        assert!(
            UserProfile::validate_username("合法中文").is_ok(),
            "中文用户名必须被接受"
        );
        assert!(UserProfile::validate_username("用户_2024").is_ok());
        assert!(UserProfile::validate_username("Ünïcode-名").is_ok());
        assert!(UserProfile::validate_username("日本語ユーザー").is_ok());
        // 首尾空白由 trim 处理，不应因粘贴带入的空格而失败。
        assert!(UserProfile::validate_username("  中文名  ").is_ok());
    }

    #[test]
    fn username_length_counts_characters_not_bytes() {
        // 30 个汉字 = 90 字节。若按字节判断会在第 10 个字就误报超长。
        let thirty_cjk = "汉".repeat(30);
        assert_eq!(thirty_cjk.len(), 90, "前提：该字符串字节数确实超过 30");
        assert!(
            UserProfile::validate_username(&thirty_cjk).is_ok(),
            "应按字符数计长，30 个汉字应通过"
        );
        assert!(UserProfile::validate_username(&"汉".repeat(31)).is_err());
    }

    #[test]
    fn username_rejects_url_and_control_characters() {
        // 这些字符会破坏 URL 拼接或 HTTP 头构造，必须拒绝。
        for bad in ["a/b", "a\\b", "a?b", "a#b", "a&b", "a=b", "a%b", "a+b", "a\"b"] {
            assert!(
                UserProfile::validate_username(bad).is_err(),
                "{bad:?} 含 URL 特殊字符，应被拒绝"
            );
        }
        assert!(UserProfile::validate_username("a\nb").is_err(), "换行必须拒绝");
        assert!(UserProfile::validate_username("a\tb").is_err(), "制表符必须拒绝");
    }

    #[test]
    fn filter_tag_matching_is_case_insensitive_and_or_semantics() {
        let f = ProblemFilter {
            tags: vec!["ARRAY".into()],
            ..Default::default()
        };
        assert!(f.matches(&mk_problem("a", SolveStatus::Todo)));
    }

    #[test]
    fn filter_status_unknown_requires_explicit_selection() {
        // 空 statuses 表示不过滤，Unknown 状态题目应当可见。
        let f = ProblemFilter::default();
        assert!(f.matches(&mk_problem("a", SolveStatus::Unknown)));

        // 显式筛选"已通过"时，Unknown 必须被排除——否则未认证用户会
        // 看到满屏"已完成"的假象。
        let f = ProblemFilter {
            statuses: vec![SolveStatus::Solved],
            ..Default::default()
        };
        assert!(!f.matches(&mk_problem("a", SolveStatus::Unknown)));
    }

    #[test]
    fn filter_keyword_matches_id_title_and_slug() {
        let p = mk_problem("two-sum", SolveStatus::Todo);
        for kw in ["1", "two sum", "TWO-SUM"] {
            let f = ProblemFilter {
                keyword: kw.into(),
                ..Default::default()
            };
            assert!(f.matches(&p), "keyword {kw} 应当命中");
        }
        let f = ProblemFilter {
            keyword: "nonexistent".into(),
            ..Default::default()
        };
        assert!(!f.matches(&p));
    }

    #[test]
    fn difficulty_counts_total_and_get() {
        let c = DifficultyCounts {
            easy: 10,
            medium: 5,
            hard: 2,
        };
        assert_eq!(c.total(), 17);
        assert_eq!(c.get(Difficulty::Medium), 5);
    }

    #[test]
    fn contest_percentile_guards_zero_participants() {
        let mut r = ContestRecord {
            title: "Weekly".into(),
            start_time: 0,
            rating: 1500.0,
            ranking: 100,
            total_participants: 0,
            problems_solved: 3,
            total_problems: 4,
            attended: true,
        };
        assert_eq!(r.percentile(), None);
        r.total_participants = 1000;
        assert_eq!(r.percentile(), Some(10.0));
    }
}
