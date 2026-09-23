//! LeetCode API 响应 DTO 与 GraphQL 信封。
//!
//! ## 设计原则
//!
//! 1. **贴合远端，不美化**。DTO 的字段名与 API 一一对应，`From` 转换负责
//!    翻译为领域模型。这样 API 变更时只需改本文件。
//! 2. **宽容解析**。所有可能缺失的字段都用 `Option` + `#[serde(default)]`。
//!    LeetCode 会在不同认证状态下省略字段（例如未登录时 `status` 为 `null`，
//!    竞赛记录中 `ranking` 可能缺失）。宁可得到 `None` 也不要解析失败。
//! 3. **未知枚举值降级而非报错**。难度、状态等字符串通过 `Difficulty::parse`
//!    转成枚举，无法识别时降级到安全默认值，保证一道题的数据异常不会
//!    导致整个题库列表加载失败。
//!
//! ## 关于未读字段
//!
//! 部分 DTO 字段（如 `GraphQlError::extensions`、`CalendarNode` 的某些列）
//! 当前未被读取。保留它们是有意为之：这是远端契约的完整映射，删掉会让
//! 后续需要时无从对照。允许 dead_code 而非删除。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::models::{Difficulty, DifficultyCounts, Problem, SolveStatus, TopicTag};

// ---------------------------------------------------------------------------
// GraphQL 信封
// ---------------------------------------------------------------------------

/// GraphQL 响应信封。
///
/// `data` 与 `errors` 可能同时存在：GraphQL 规范允许部分字段失败时
/// 仍返回其余字段。因此不能"有 errors 就丢弃 data"——需由调用方判断
/// `data` 是否为 `null`。
///
/// 注意：这里**不**派生 `Deserialize`，因为下方提供了手写实现。
/// 若同时存在会触发 E0119（实现冲突）。
#[derive(Debug)]
pub struct GraphQlResponse<T> {
    pub data: Option<T>,
    pub errors: Option<Vec<GraphQlError>>,
}

/// `Option<T>` 的 `Deserialize` 实现需要 `T: Deserialize`，
/// 但 `#[serde(default)]` 在 `Option<T>` 上要求 `T: Default` 才能
/// 在字段缺失时构造默认值。为让所有 DTO 都能直接用作 `T`（而不必
/// 强制实现 `Default`），这里手动实现：字段缺失时直接给 `None`。
impl<'de, T: serde::de::DeserializeOwned> serde::de::Deserialize<'de>
    for GraphQlResponse<T>
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let value = serde_json::Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| D::Error::custom("GraphQL 响应应为 JSON 对象"))?;

        // 从 `serde_json::Value` 反序列化需要 `T: DeserializeOwned`
        // （而非仅 `Deserialize<'de>`），因为中转值的生命周期与 `'de` 无关。
        let data = match obj.get("data") {
            Some(serde_json::Value::Null) | None => None,
            Some(v) => Some(serde_json::from_value::<T>(v.clone()).map_err(D::Error::custom)?),
        };

        let errors = match obj.get("errors") {
            Some(serde_json::Value::Null) | None => None,
            Some(v) => Some(
                serde_json::from_value::<Vec<GraphQlError>>(v.clone()).map_err(D::Error::custom)?,
            ),
        };

        Ok(Self { data, errors })
    }
}

/// GraphQL 错误条目。
#[derive(Debug, Deserialize)]
pub struct GraphQlError {
    pub message: String,
    #[serde(default)]
    pub path: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub extensions: Option<serde_json::Value>,
}

impl GraphQlError {
    /// 是否为"无权限"类错误。
    ///
    /// 实测中未认证访问日历会得到 `no permission to check the calendar.`。
    /// 识别该模式可将它映射为"功能不可用"而非"程序错误"。
    pub fn is_permission_denied(&self) -> bool {
        let m = self.message.to_lowercase();
        m.contains("no permission")
            || m.contains("not authorized")
            || m.contains("unauthorized")
    }

    /// 汇总为单行描述，附带路径便于定位。
    pub fn describe(&self) -> String {
        match &self.path {
            Some(p) if !p.is_empty() => {
                let joined = p
                    .iter()
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(".");
                format!("{} (路径: {joined})", self.message)
            }
            _ => self.message.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// 题库列表
// ---------------------------------------------------------------------------

/// `problemsetQuestionList` 的 `data` 部分。
#[derive(Debug, Deserialize)]
pub struct ProblemListData {
    #[serde(rename = "problemsetQuestionList")]
    pub problemset_question_list: Option<ProblemListPage>,
}

/// 题库分页结果。
///
/// 同时承载两站的字段差异（用 `alias` 让同一字段接受两种命名）：
/// - 国际站：`total: totalNum` + `questions: data`
/// - 中国站：`total` + `questions`（无别名）
///
/// `has_more` 仅中国站提供，作为分页终止的补充信号。
#[derive(Debug, Deserialize)]
pub struct ProblemListPage {
    /// 满足条件的题目总数（不受当前分页限制）。用于计算分页数。
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub questions: Vec<QuestionNode>,
    /// 中国站提供的"还有下一页"标志。国际站不返回此字段。
    #[serde(default, rename = "hasMore")]
    pub has_more: Option<bool>,
}

/// 题库列表中的单个题目节点。
///
/// 国际站类型为 `QuestionNode`，中国站为 `QuestionLightNode`，
/// 两者字段命名有若干处**恰好互为倒序**，用 `alias` 同时接受：
///
/// | 含义 | 国际站 | 中国站 |
/// |------|--------|--------|
/// | 题号 | `questionFrontendId` | `frontendQuestionId` |
/// | 付费 | `isPaidOnly` | `paidOnly` |
/// | 中文标题 | 无 | `titleCn` |
///
/// 注意：serde 的 `alias` 是"主名 + 备选名"语义，反序列化时两者都接受，
/// 但序列化只输出 `rename` 指定的主名。本类型只做反序列化，因此无影响。
#[derive(Debug, Deserialize)]
pub struct QuestionNode {
    #[serde(default, rename = "questionId")]
    pub question_id: Option<String>,
    /// 题号。主名用国际站的 `questionFrontendId`，备选中国站的
    /// `frontendQuestionId`。这是全项目最容易写错的一处字段名。
    #[serde(default, rename = "questionFrontendId", alias = "frontendQuestionId")]
    pub frontend_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// 中文标题，仅中国站返回。用于在中文站点展示更友好的标题。
    #[serde(default, rename = "titleCn")]
    pub title_cn: Option<String>,
    #[serde(default, rename = "titleSlug")]
    pub title_slug: Option<String>,
    #[serde(default)]
    pub difficulty: Option<String>,
    #[serde(default, rename = "acRate")]
    pub ac_rate: Option<f64>,
    #[serde(default)]
    pub status: Option<String>,
    /// 付费标记。主名国际站的 `isPaidOnly`，备选中国站的 `paidOnly`。
    #[serde(default, rename = "isPaidOnly", alias = "paidOnly")]
    pub is_paid_only: Option<bool>,
    #[serde(default, rename = "topicTags")]
    pub topic_tags: Vec<TagNode>,
}

impl QuestionNode {
    /// 转换为领域模型。缺关键字段（题号、标题、slug）时返回 `None`，
    /// 让调用方跳过该条目而不是构造出一条残缺数据。
    ///
    /// `prefer_chinese_title`：中国站的题目节点同时提供 `title`（英文）与
    /// `titleCn`（中文）。为 true 且中文标题非空时优先取中文。
    pub fn into_problem(self) -> Option<Problem> {
        self.into_problem_with_locale(false)
    }

    /// 带语言偏好的转换。中国站有中文标题时可用。
    pub fn into_problem_with_locale(self, prefer_chinese_title: bool) -> Option<Problem> {
        let frontend_id = self.frontend_id?;
        let title = if prefer_chinese_title {
            // 中文标题可能为空串或纯空白，此时回退到英文标题。
            let cn = self.title_cn.filter(|s| !s.trim().is_empty());
            cn.or(self.title)?
        } else {
            self.title?
        };
        let title_slug = self.title_slug?;

        // 难度无法识别时降级为 Easy 而非丢弃整条数据：
        // LeetCode 未来若新增难度等级，用户仍能看到题目。
        let difficulty = self
            .difficulty
            .as_deref()
            .and_then(Difficulty::parse)
            .unwrap_or(Difficulty::Easy);

        Some(Problem {
            question_id: self.question_id.unwrap_or_else(|| frontend_id.clone()),
            frontend_id,
            title,
            title_slug,
            difficulty,
            ac_rate: self.ac_rate.unwrap_or(0.0),
            tags: self.topic_tags.into_iter().map(TagNode::into_tag).collect(),
            status: SolveStatus::from_api(self.status.as_deref()),
            is_paid_only: self.is_paid_only.unwrap_or(false),
        })
    }
}

/// 标签节点。
#[derive(Debug, Deserialize)]
pub struct TagNode {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

impl TagNode {
    pub fn into_tag(self) -> TopicTag {
        TopicTag {
            name: self.name.unwrap_or_default(),
            slug: self.slug.unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// 全局统计
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GlobalCountsData {
    #[serde(default, rename = "allQuestionsCount")]
    pub all_questions_count: Vec<DifficultyCountEntry>,
}

/// `{ difficulty, count }` 形式的条目，被多处复用。
#[derive(Debug, Deserialize)]
pub struct DifficultyCountEntry {
    #[serde(default)]
    pub difficulty: Option<String>,
    #[serde(default)]
    pub count: Option<i64>,
}

/// 把难度字符串累加进 `DifficultyCounts`。
///
/// **大小写不敏感**：国际站是 `"Easy"`/`"Medium"`/`"Hard"`，
/// 中国站是全大写 `"EASY"`/`"MEDIUM"`/`"HARD"`。
///
/// 另刻意忽略 `"All"` 之类的汇总项——总数可由三项相加得出，
/// 若把 `All` 也计入某一档会造成重复计数。
fn accumulate_difficulty(target: &mut DifficultyCounts, entry: &DifficultyCountEntry) {
    let n = entry.count.unwrap_or(0).max(0) as u32;
    match entry.difficulty.as_deref() {
        Some(d) if d.eq_ignore_ascii_case("easy") => target.easy = n,
        Some(d) if d.eq_ignore_ascii_case("medium") => target.medium = n,
        Some(d) if d.eq_ignore_ascii_case("hard") => target.hard = n,
        _ => {}
    }
}

impl GlobalCountsData {
    /// 按难度汇总总题量。
    ///
    /// 命名为 `to_counts` 而非 `into_counts`：该方法只读 `self`、不消费所有权，
    /// 调用方之后仍可能用到原始响应（例如日志或缓存）。
    pub fn to_counts(&self) -> DifficultyCounts {
        let mut c = DifficultyCounts::default();
        for e in &self.all_questions_count {
            accumulate_difficulty(&mut c, e);
        }
        c
    }
}

// ---------------------------------------------------------------------------
// 用户画像
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UserProfileData {
    #[serde(default, rename = "matchedUser")]
    pub matched_user: Option<MatchedUserProfile>,
}

#[derive(Debug, Deserialize)]
pub struct MatchedUserProfile {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub profile: Option<ProfileNode>,
    #[serde(default, rename = "submitStatsGlobal")]
    pub submit_stats: Option<SubmitStats>,
}

#[derive(Debug, Deserialize)]
pub struct ProfileNode {
    #[serde(default, rename = "realName")]
    pub real_name: Option<String>,
    #[serde(default)]
    pub ranking: Option<i64>,
    #[serde(default)]
    pub reputation: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SubmitStats {
    #[serde(default, rename = "acSubmissionNum")]
    pub ac_submission_num: Vec<DifficultyCountEntry>,
    #[serde(default, rename = "totalSubmissionNum")]
    pub total_submission_num: Vec<DifficultyCountEntry>,
}

/// 把 `[{difficulty, count}]` 数组转成 `DifficultyCounts`。
///
/// 同时处理"失败数"这类需要通过差值得到的指标。
/// 与 `accumulate_difficulty` 共用同一套大小写不敏感的匹配逻辑，
/// 避免两处实现漂移。
fn entries_to_counts(entries: &[DifficultyCountEntry]) -> DifficultyCounts {
    let mut c = DifficultyCounts::default();
    for e in entries {
        accumulate_difficulty(&mut c, e);
    }
    c
}

// ---------------------------------------------------------------------------
// 用户画像 —— 中国站
// ---------------------------------------------------------------------------
//
// 中国站没有 `matchedUser`，改用 `userProfilePublicProfile` 与
// `userProfileUserQuestionProgressV2` **两个独立查询**（国际站是一个查询
// 就能同时拿到画像与解题统计）。因此中国站的画像解析需要合并两份响应。

/// `userProfilePublicProfile` 的 `data` 部分。
#[derive(Debug, Deserialize)]
pub struct CnUserProfileData {
    #[serde(default, rename = "userProfilePublicProfile")]
    pub public_profile: Option<CnPublicProfileNode>,
}

#[derive(Debug, Deserialize)]
pub struct CnPublicProfileNode {
    #[serde(default)]
    pub username: Option<String>,
    /// 站点排名（整数）。等价于国际站的 `profile.ranking`。
    ///
    /// 中国站另有 `profile.ranking.ranking`，但那是**一个 JSON 数组的
    /// 字符串**，需要二次解析且语义更复杂（按难度分档的历年排名）。
    /// 本应用一律取 `siteRanking`，更简单也更可靠。
    #[serde(default, rename = "siteRanking")]
    pub site_ranking: Option<i64>,
    #[serde(default)]
    pub profile: Option<CnProfileNode>,
}

#[derive(Debug, Deserialize)]
pub struct CnProfileNode {
    #[serde(default, rename = "realName")]
    pub real_name: Option<String>,
    #[serde(default)]
    pub reputation: Option<i64>,
}

/// `userProfileUserQuestionProgressV2` 的 `data` 部分。
///
/// 注意：远端**直接给出已通过数与未通过数**，无需像国际站那样用
/// "总提交 - 通过"做减法。这让中国站的数据链路反而更简单、更不易出错。
#[derive(Debug, Deserialize)]
pub struct CnUserProgressData {
    #[serde(default, rename = "userProfileUserQuestionProgressV2")]
    pub progress: Option<CnUserProgressNode>,
}

#[derive(Debug, Deserialize)]
pub struct CnUserProgressNode {
    #[serde(default, rename = "numAcceptedQuestions")]
    pub num_accepted_questions: Vec<DifficultyCountEntry>,
    #[serde(default, rename = "numFailedQuestions")]
    pub num_failed_questions: Vec<DifficultyCountEntry>,
}

impl CnUserProfileData {
    /// 合并两份中国站响应为统一的领域模型。
    ///
    /// `progress` 来自 `CN_USER_PROGRESS` 查询。传 `None` 表示进度查询失败，
    /// 此时解题数全为 0——画像仍然可用（用户名、排名、声望都是有效数据）。
    ///
    /// 用户名不存在时 `public_profile` 为 `None`，返回 `None`。
    pub fn into_profile(
        self,
        progress: Option<CnUserProgressNode>,
        fallback_username: &str,
        authenticated: bool,
    ) -> Option<crate::models::UserProfile> {
        let u = self.public_profile?;
        let profile = u.profile.unwrap_or(CnProfileNode {
            real_name: None,
            reputation: None,
        });

        let (solved, failed) = match progress {
            Some(p) => (
                entries_to_counts(&p.num_accepted_questions),
                entries_to_counts(&p.num_failed_questions),
            ),
            None => (DifficultyCounts::default(), DifficultyCounts::default()),
        };

        Some(crate::models::UserProfile {
            username: u.username.unwrap_or_else(|| fallback_username.to_string()),
            real_name: profile.real_name.filter(|s| !s.trim().is_empty()),
            ranking: u.site_ranking.and_then(|r| u32::try_from(r).ok()),
            reputation: profile.reputation.and_then(|r| i32::try_from(r).ok()),
            // 中国站不返回"总提交数"，只有"通过/未通过"。
            // 用二者之和近似总提交数，语义上略有偏差（一次题目的多次提交
            // 只被计入一次），但这是远端能提供的最接近的字段。
            total_submissions: Some(solved.total() + failed.total()),
            solved,
            failed,
            submission_calendar: Vec::new(),
            streak: None,
            total_active_days: None,
            fetched_at: chrono::Utc::now().timestamp(),
            authenticated,
        })
    }
}

impl UserProfileData {
    /// 转换为领域模型。用户名不存在（`matchedUser` 为 `null`）时返回 `None`。
    pub fn into_profile(self, fallback_username: &str, authenticated: bool) -> Option<crate::models::UserProfile> {
        let u = self.matched_user?;
        let profile = u.profile.unwrap_or(ProfileNode {
            real_name: None,
            ranking: None,
            reputation: None,
        });
        let (solved, total_submitted) = match u.submit_stats {
            Some(s) => (
                entries_to_counts(&s.ac_submission_num),
                entries_to_counts(&s.total_submission_num),
            ),
            None => (DifficultyCounts::default(), DifficultyCounts::default()),
        };

        // "尝试失败数" = 总提交数 - 通过数。用 saturating 避免下溢 panic——
        // 远端数据偶有不一致（通过数大于提交数）时会触发减法溢出。
        let failed = DifficultyCounts {
            easy: total_submitted.easy.saturating_sub(solved.easy),
            medium: total_submitted.medium.saturating_sub(solved.medium),
            hard: total_submitted.hard.saturating_sub(solved.hard),
        };

        Some(crate::models::UserProfile {
            username: u.username.unwrap_or_else(|| fallback_username.to_string()),
            real_name: profile.real_name.filter(|s| !s.trim().is_empty()),
            ranking: profile.ranking.and_then(|r| u32::try_from(r).ok()),
            reputation: profile
                .reputation
                .and_then(|r| i32::try_from(r).ok()),
            total_submissions: Some(total_submitted.total()),
            solved,
            failed,
            submission_calendar: Vec::new(), // 由日历查询单独填充
            streak: None,
            total_active_days: None,
            fetched_at: chrono::Utc::now().timestamp(),
            authenticated,
        })
    }
}

// ---------------------------------------------------------------------------
// 标签统计
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TagStatsData {
    #[serde(default, rename = "matchedUser")]
    pub matched_user: Option<TagStatsUser>,
}

#[derive(Debug, Deserialize)]
pub struct TagStatsUser {
    #[serde(default, rename = "tagProblemCounts")]
    pub tag_problem_counts: Option<TagCountGroups>,
}

/// 三档标签分组。LeetCode 按抽象程度分组，但对本应用而言同属"用户的
/// 知识点分布"，因此合并处理。
#[derive(Debug, Deserialize)]
pub struct TagCountGroups {
    #[serde(default)]
    pub advanced: Vec<TagCountNode>,
    #[serde(default)]
    pub intermediate: Vec<TagCountNode>,
    #[serde(default)]
    pub fundamental: Vec<TagCountNode>,
}

#[derive(Debug, Deserialize)]
pub struct TagCountNode {
    #[serde(default, rename = "tagName")]
    pub tag_name: Option<String>,
    #[serde(default, rename = "tagSlug")]
    pub tag_slug: Option<String>,
    #[serde(default, rename = "problemsSolved")]
    pub problems_solved: Option<i64>,
}

impl TagCountGroups {
    /// 合并三档为统一列表，`total`（题库总量）留待调用方结合本地题库填充。
    ///
    /// 去重策略：同 slug 只保留第一个（`problemsSolved` 取最大值），
    /// 防止未来 LeetCode 调整分组后出现重复条目。
    pub fn into_progress(self) -> Vec<crate::models::TagProgress> {
        use std::collections::HashMap;

        let all = self
            .fundamental
            .into_iter()
            .chain(self.intermediate)
            .chain(self.advanced);

        let mut map: HashMap<String, crate::models::TagProgress> = HashMap::new();
        for n in all {
            let (Some(name), Some(slug)) = (n.tag_name, n.tag_slug) else {
                continue;
            };
            if slug.is_empty() {
                continue;
            }
            let solved = n.problems_solved.unwrap_or(0).max(0) as u32;
            map.entry(slug.clone())
                .and_modify(|e| {
                    if solved > e.solved {
                        e.solved = solved;
                    }
                })
                .or_insert(crate::models::TagProgress {
                    tag_name: name,
                    tag_slug: slug,
                    solved,
                    total: 0,
                });
        }

        let mut out: Vec<_> = map.into_values().collect();
        // 确定性排序：按已解题数降序，次数相同按 slug 升序。
        // 推荐算法依赖这个顺序保证同输入同输出。
        out.sort_by(|a, b| {
            b.solved
                .cmp(&a.solved)
                .then_with(|| a.tag_slug.cmp(&b.tag_slug))
        });
        out
    }
}

// ---------------------------------------------------------------------------
// 日历
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CalendarData {
    #[serde(default, rename = "matchedUser")]
    pub matched_user: Option<CalendarUser>,
}

#[derive(Debug, Deserialize)]
pub struct CalendarUser {
    #[serde(default, rename = "userCalendar")]
    pub user_calendar: Option<CalendarNode>,
}

#[derive(Debug, Deserialize)]
pub struct CalendarNode {
    #[serde(default)]
    pub streak: Option<i64>,
    #[serde(default, rename = "totalActiveDays")]
    pub total_active_days: Option<i64>,
    /// 这是一个 **JSON 字符串**，需二次解析为 `{ "1700000000": 3, ... }`。
    #[serde(default, rename = "submissionCalendar")]
    pub submission_calendar: Option<String>,
}

impl CalendarNode {
    /// 解析 `submissionCalendar` 字符串。
    ///
    /// 键是 Unix 秒时间戳，值是当日提交次数。返回按日期升序排列的
    /// `(yyyy-MM-dd, count)` 列表。
    ///
    /// 解析失败返回空 Vec 而非报错：日历属于锦上添花的信息，
    /// 不应因其格式变动而阻断主流程。
    pub fn parse_calendar(&self) -> Vec<(String, u32)> {
        let Some(raw) = self.submission_calendar.as_deref() else {
            return Vec::new();
        };
        let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(raw)
        else {
            return Vec::new();
        };

        let mut out: Vec<(i64, u32)> = map
            .into_iter()
            .filter_map(|(k, v)| {
                let ts: i64 = k.parse().ok()?;
                let count = match v {
                    serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) as u32,
                    serde_json::Value::String(s) => s.parse().unwrap_or(0),
                    _ => 0,
                };
                Some((ts, count))
            })
            .collect();

        out.sort_by_key(|(ts, _)| *ts);

        out.into_iter()
            .map(|(ts, c)| {
                let date = chrono::DateTime::from_timestamp(ts, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| ts.to_string());
                (date, c)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 提交记录
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SubmissionListData {
    #[serde(default, rename = "submissionList")]
    pub submission_list: Option<SubmissionListPage>,
}

#[derive(Debug, Deserialize)]
pub struct SubmissionListPage {
    #[serde(default)]
    pub submissions: Option<Vec<SubmissionNode>>,
}

#[derive(Debug, Deserialize)]
pub struct SubmissionNode {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "titleSlug")]
    pub title_slug: Option<String>,
    /// 机器可读状态，如 "Accepted" / "Wrong Answer"。
    #[serde(default)]
    pub status: Option<String>,
    /// 展示用状态（可能本地化）。
    #[serde(default, rename = "statusDisplay")]
    pub status_display: Option<String>,
    /// 字符串形式的 Unix 秒。
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub lang: Option<String>,
}

/// 简化的提交记录，供 UI 与 LLM 上下文使用。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecentSubmission {
    pub title: String,
    pub title_slug: String,
    pub status: String,
    pub is_accepted: bool,
    pub submitted_at: Option<i64>,
    pub lang: Option<String>,
}

impl SubmissionNode {
    pub fn into_recent(self) -> Option<RecentSubmission> {
        let title = self.title?;
        // 判定"通过"以机器可读的 status 为准：statusDisplay 可能被本地化，
        // 依赖它做判断在非英文环境下会失效。
        // 注意：必须在 move 出 self.status 之前完成判定，否则会触发
        // 借用已移动值的编译错误。
        let is_accepted = self.status.as_deref() == Some("Accepted");
        // 展示文案优先用 statusDisplay，缺失时回退到机器状态。
        let status = self.status_display.or(self.status).unwrap_or_default();
        Some(RecentSubmission {
            title,
            title_slug: self.title_slug.unwrap_or_default(),
            status,
            is_accepted,
            submitted_at: self.timestamp.as_deref().and_then(|s| s.parse().ok()),
            lang: self.lang.filter(|s| !s.trim().is_empty()),
        })
    }
}

// ---------------------------------------------------------------------------
// 竞赛
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ContestRankingData {
    #[serde(default, rename = "userContestRanking")]
    pub user_contest_ranking: Option<ContestRankingNode>,
}

#[derive(Debug, Deserialize)]
pub struct ContestRankingNode {
    #[serde(default, rename = "attendedContestsCount")]
    pub attended_contests_count: Option<i64>,
    #[serde(default)]
    pub rating: Option<f64>,
    #[serde(default, rename = "globalRanking")]
    pub global_ranking: Option<i64>,
    #[serde(default, rename = "totalParticipants")]
    pub total_participants: Option<i64>,
    #[serde(default, rename = "topPercentage")]
    pub top_percentage: Option<f64>,
}

impl ContestRankingData {
    pub fn into_summary(self) -> crate::models::ContestSummary {
        let Some(r) = self.user_contest_ranking else {
            return crate::models::ContestSummary::default();
        };
        crate::models::ContestSummary {
            attended_count: r
                .attended_contests_count
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0),
            rating: r.rating,
            global_ranking: r.global_ranking.and_then(|v| u32::try_from(v).ok()),
            total_participants: r.total_participants.and_then(|v| u32::try_from(v).ok()),
            top_percentage: r.top_percentage,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ContestHistoryData {
    #[serde(default, rename = "userContestRankingHistory")]
    pub history: Vec<ContestHistoryNode>,
}

#[derive(Debug, Deserialize)]
pub struct ContestHistoryNode {
    #[serde(default)]
    pub attended: Option<bool>,
    #[serde(default, rename = "problemsSolved")]
    pub problems_solved: Option<i64>,
    #[serde(default, rename = "totalProblems")]
    pub total_problems: Option<i64>,
    #[serde(default)]
    pub rating: Option<f64>,
    #[serde(default)]
    pub ranking: Option<i64>,
    #[serde(default)]
    pub contest: Option<ContestMetaNode>,
}

#[derive(Debug, Deserialize)]
pub struct ContestMetaNode {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "startTime")]
    pub start_time: Option<i64>,
}

impl ContestHistoryData {
    /// 转换为领域模型，**只保留实际参赛的场次**。
    ///
    /// 过滤 `attended: false` 是关键：历史数组中含"仅注册未参加"的记录，
    /// 若计入会污染 Rating 趋势与稳定性分析。
    ///
    /// 返回按时间升序排列的记录，便于直接绘制趋势曲线。
    pub fn into_records(self) -> Vec<crate::models::ContestRecord> {
        let mut out: Vec<crate::models::ContestRecord> = self
            .history
            .into_iter()
            .filter(|n| n.attended.unwrap_or(false))
            .filter_map(|n| {
                let meta = n.contest?;
                Some(crate::models::ContestRecord {
                    title: meta.title.unwrap_or_else(|| "未知竞赛".to_string()),
                    start_time: meta.start_time.unwrap_or(0),
                    // Rating 缺失使趋势分析失去意义，故此条数据跳过。
                    rating: n.rating?,
                    ranking: n.ranking.and_then(|r| u32::try_from(r).ok()).unwrap_or(0),
                    total_participants: 0, // 该接口不返回此字段
                    problems_solved: n.problems_solved.unwrap_or(0).max(0) as u32,
                    total_problems: n.total_problems.unwrap_or(0).max(0) as u32,
                    attended: true,
                })
            })
            .collect();

        out.sort_by_key(|r| r.start_time);
        out
    }
}

// ---------------------------------------------------------------------------
// 登录状态探测
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UserStatusData {
    #[serde(default, rename = "userStatus")]
    pub user_status: Option<UserStatusNode>,
}

#[derive(Debug, Deserialize)]
pub struct UserStatusNode {
    #[serde(default, rename = "isSignedIn")]
    pub is_signed_in: Option<bool>,
    #[serde(default)]
    pub username: Option<String>,
    /// 中国站特有的用户 slug（ASCII 标识）。
    ///
    /// 这是中国站查询 `userProfilePublicProfile(userSlug:)` 时**应该**用的值，
    /// 而非用户设置页填的显示昵称。详见 `queries::CN_USER_STATUS` 的注释。
    ///
    /// 国际站的 `userStatus` 不返回此字段，因此恒为 `None`。
    #[serde(default, rename = "userSlug")]
    pub user_slug: Option<String>,
    #[serde(default, rename = "realName")]
    pub real_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problem_list_parses_real_response_shape() {
        // 该 JSON 直接取自对 leetcode.com/graphql 的实测响应。
        let raw = r#"{"data":{"problemsetQuestionList":{"total":4059,"questions":[
            {"acRate":57.90991532916936,"difficulty":"Easy","questionFrontendId":"1",
             "questionId":"1","title":"Two Sum","titleSlug":"two-sum",
             "topicTags":[{"name":"Array","slug":"array"},{"name":"Hash Table","slug":"hash-table"}],
             "status":null,"isPaidOnly":false}]}}}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        let page = resp.data.unwrap().problemset_question_list.unwrap();
        assert_eq!(page.total, 4059);
        assert_eq!(page.questions.len(), 1);

        let p = page.questions.into_iter().next().unwrap().into_problem().unwrap();
        assert_eq!(p.frontend_id, "1");
        assert_eq!(p.title, "Two Sum");
        assert_eq!(p.difficulty, Difficulty::Easy);
        assert_eq!(p.tags.len(), 2);
        // 未认证时 status 为 null，必须映射为 Unknown。
        assert_eq!(p.status, SolveStatus::Unknown);
        assert!(p
            .url(crate::config::LeetCodeSite::Com)
            .unwrap()
            .ends_with("/problems/two-sum/"));
        assert!(p
            .url(crate::config::LeetCodeSite::Cn)
            .unwrap()
            .ends_with("/problems/two-sum/"));
    }

    #[test]
    fn problem_node_with_status_ac_parses_as_solved() {
        let raw = r#"{"data":{"problemsetQuestionList":{"total":1,"questions":[
            {"questionFrontendId":"1","questionId":"1","title":"Two Sum","titleSlug":"two-sum",
             "difficulty":"Easy","acRate":50.0,"status":"AC","isPaidOnly":false,"topicTags":[]}]}}}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        let p = resp
            .data
            .unwrap()
            .problemset_question_list
            .unwrap()
            .questions
            .into_iter()
            .next()
            .unwrap()
            .into_problem()
            .unwrap();
        assert_eq!(p.status, SolveStatus::Solved);
    }

    #[test]
    fn problem_node_missing_critical_fields_is_skipped() {
        // 缺少 titleSlug 的条目应被跳过，而不是产生一条半残数据。
        let raw = r#"{"data":{"problemsetQuestionList":{"total":1,"questions":[
            {"questionFrontendId":"1","title":"Two Sum"}]}}}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        let node = resp
            .data
            .unwrap()
            .problemset_question_list
            .unwrap()
            .questions
            .into_iter()
            .next()
            .unwrap();
        assert!(node.into_problem().is_none());
    }

    #[test]
    fn unknown_difficulty_degrades_to_easy_not_dropped() {
        let raw = r#"{"data":{"problemsetQuestionList":{"total":1,"questions":[
            {"questionFrontendId":"1","title":"X","titleSlug":"x","difficulty":"Nightmare"}]}}}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        let p = resp
            .data
            .unwrap()
            .problemset_question_list
            .unwrap()
            .questions
            .into_iter()
            .next()
            .unwrap()
            .into_problem()
            .unwrap();
        // 未知难度降级而非丢弃，保证题目仍可见。
        assert_eq!(p.difficulty, Difficulty::Easy);
    }

    #[test]
    fn global_counts_excludes_all_and_sums_correctly() {
        let raw = r#"{"data":{"allQuestionsCount":[
            {"difficulty":"All","count":4059},{"difficulty":"Easy","count":966},
            {"difficulty":"Medium","count":2117},{"difficulty":"Hard","count":976}]}}"#;
        let resp: GraphQlResponse<GlobalCountsData> = serde_json::from_str(raw).unwrap();
        let c = resp.data.unwrap().to_counts();
        assert_eq!(c.easy, 966);
        assert_eq!(c.medium, 2117);
        assert_eq!(c.hard, 976);
        assert_eq!(c.total(), 4059, "三项之和应等于 All 的计数");
    }

    #[test]
    fn user_profile_parses_and_computes_failed_by_subtraction() {
        let raw = r#"{"data":{"matchedUser":{"username":"LeetCode",
            "profile":{"realName":"LeetCode","ranking":2894098,"reputation":77616},
            "submitStatsGlobal":{
              "acSubmissionNum":[{"difficulty":"All","count":45},{"difficulty":"Easy","count":12},
                                 {"difficulty":"Medium","count":22},{"difficulty":"Hard","count":11}],
              "totalSubmissionNum":[{"difficulty":"All","count":100},{"difficulty":"Easy","count":30},
                                    {"difficulty":"Medium","count":50},{"difficulty":"Hard","count":20}]}}}}"#;
        let resp: GraphQlResponse<UserProfileData> = serde_json::from_str(raw).unwrap();
        let p = resp.data.unwrap().into_profile("fallback", false).unwrap();
        assert_eq!(p.username, "LeetCode");
        assert_eq!(p.solved.easy, 12);
        assert_eq!(p.solved.total(), 45);
        assert_eq!(p.failed.easy, 18, "30 - 12 = 18");
        assert_eq!(p.failed.hard, 9, "20 - 11 = 9");
        assert_eq!(p.total_submissions, Some(100));
    }

    #[test]
    fn user_profile_failed_does_not_underflow_on_inconsistent_data() {
        // 远端数据异常时（通过数 > 提交数），减法必须饱和而非 panic。
        let raw = r#"{"data":{"matchedUser":{"username":"u","profile":{"ranking":1},
            "submitStatsGlobal":{
              "acSubmissionNum":[{"difficulty":"Easy","count":50}],
              "totalSubmissionNum":[{"difficulty":"Easy","count":10}]}}}}"#;
        let resp: GraphQlResponse<UserProfileData> = serde_json::from_str(raw).unwrap();
        let p = resp.data.unwrap().into_profile("u", false).unwrap();
        assert_eq!(p.failed.easy, 0, "下溢应饱和到 0");
    }

    // -----------------------------------------------------------------------
    // 中国站解析
    // -----------------------------------------------------------------------
    //
    // 下列 JSON 均直接取自对 https://leetcode.cn/graphql 的**实测响应**，
    // 而非按文档臆造。两站字段命名差异大，用真实样本才能防止"照着 .com
    // 的结构写测试、测试通过但线上 400"这类假性通过。

    /// 中国站难度枚举是全大写，必须能被正确识别。
    ///
    /// 若此处失灵，所有题目的难度会静默降级为 Easy——
    /// 不报错、不崩溃，只是数据全错，属于最难发现的缺陷类型。
    #[test]
    fn cn_difficulty_uppercase_is_recognized() {
        let raw = r#"{"data":{"userProfileUserQuestionProgressV2":{
            "numAcceptedQuestions":[{"difficulty":"EASY","count":25},
                                    {"difficulty":"MEDIUM","count":26},
                                    {"difficulty":"HARD","count":14}],
            "numFailedQuestions":[{"difficulty":"EASY","count":31},
                                  {"difficulty":"MEDIUM","count":31},
                                  {"difficulty":"HARD","count":13}]}}}"#;
        let resp: GraphQlResponse<CnUserProgressData> = serde_json::from_str(raw).unwrap();
        let p = resp.data.unwrap().progress.unwrap();

        let solved = entries_to_counts(&p.num_accepted_questions);
        assert_eq!(solved.easy, 25, "EASY 必须被识别为 Easy，而非静默降级");
        assert_eq!(solved.medium, 26);
        assert_eq!(solved.hard, 14);
        assert_eq!(solved.total(), 65);
    }

    /// 中国站画像 + 进度的合并解析（两份响应）。
    #[test]
    fn cn_profile_merges_profile_and_progress_responses() {
        let profile_raw = r#"{"data":{"userProfilePublicProfile":{
            "username":"LeetCode","siteRanking":100000,
            "profile":{"realName":"LeetCode","reputation":2121,
                       "userAvatar":"https://assets.leetcode.cn/x.png"}}}}"#;
        let progress_raw = r#"{"data":{"userProfileUserQuestionProgressV2":{
            "numAcceptedQuestions":[{"difficulty":"EASY","count":25},
                                    {"difficulty":"MEDIUM","count":26},
                                    {"difficulty":"HARD","count":14}],
            "numFailedQuestions":[{"difficulty":"EASY","count":31},
                                  {"difficulty":"MEDIUM","count":31},
                                  {"difficulty":"HARD","count":13}]}}}"#;

        let pd: GraphQlResponse<CnUserProfileData> =
            serde_json::from_str(profile_raw).unwrap();
        let gd: GraphQlResponse<CnUserProgressData> =
            serde_json::from_str(progress_raw).unwrap();

        let p = pd
            .data
            .unwrap()
            .into_profile(gd.data.unwrap().progress, "fallback", true)
            .unwrap();

        assert_eq!(p.username, "LeetCode");
        assert_eq!(p.real_name.as_deref(), Some("LeetCode"));
        assert_eq!(p.ranking, Some(100_000), "siteRanking 应映射为 ranking");
        assert_eq!(p.reputation, Some(2121));
        assert_eq!(p.solved.easy, 25);
        assert_eq!(p.solved.total(), 65);
        assert_eq!(p.failed.easy, 31, "中国站直接给出失败数，无需做减法");
        assert_eq!(p.failed.total(), 75);
        // 中国站无"总提交数"字段，用通过 + 未通过近似。
        assert_eq!(p.total_submissions, Some(140));
    }

    /// 中国站进度查询失败时，画像本身仍应可用。
    #[test]
    fn cn_profile_survives_missing_progress() {
        let profile_raw = r#"{"data":{"userProfilePublicProfile":{
            "username":"someone","siteRanking":5000,"profile":{"reputation":10}}}}"#;
        let pd: GraphQlResponse<CnUserProfileData> =
            serde_json::from_str(profile_raw).unwrap();
        let p = pd
            .data
            .unwrap()
            .into_profile(None, "fallback", false)
            .unwrap();
        assert_eq!(p.username, "someone");
        assert_eq!(p.ranking, Some(5000));
        // 进度缺失时解题数为 0，但不影响画像其他字段。
        assert_eq!(p.solved.total(), 0);
    }

    /// 用户名不存在时 `userProfilePublicProfile` 为 `null`（HTTP 200）。
    #[test]
    fn cn_profile_returns_none_when_user_absent() {
        let raw = r#"{"data":{"userProfilePublicProfile":null}}"#;
        let resp: GraphQlResponse<CnUserProfileData> = serde_json::from_str(raw).unwrap();
        assert!(resp.data.unwrap().into_profile(None, "ghost", false).is_none());
    }

    /// 中国站题号字段是 `frontendQuestionId`（与国际站恰好相反）。
    ///
    /// JSON 样本取自对中国站的实测响应。
    #[test]
    fn cn_problem_list_parses_frontend_question_id() {
        let raw = r#"{"data":{"problemsetQuestionList":{
            "total":4447,"hasMore":true,"questions":[
            {"acRate":0.5523291465483793,"difficulty":"EASY","paidOnly":false,
             "status":"NOT_STARTED","frontendQuestionId":"1","title":"Two Sum",
             "titleCn":"两数之和","titleSlug":"two-sum",
             "topicTags":[{"id":"wg0rh","name":"Array","slug":"array",
                           "nameTranslated":"数组"}]}]}}}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        let page = resp.data.unwrap().problemset_question_list.unwrap();
        assert_eq!(page.total, 4447);
        assert_eq!(page.has_more, Some(true));
        assert_eq!(page.questions.len(), 1);

        let p = page
            .questions
            .into_iter()
            .next()
            .unwrap()
            .into_problem()
            .unwrap();
        assert_eq!(p.frontend_id, "1", "中国站的 frontendQuestionId 必须被读到");
        assert_eq!(p.title, "Two Sum");
        assert_eq!(p.difficulty, Difficulty::Easy, "全大写 EASY 必须被识别");
        assert!(!p.is_paid_only, "paidOnly 必须被读到");
        assert_eq!(p.status, SolveStatus::Todo, "NOT_STARTED 应映射为未开始");
        assert_eq!(p.tags.len(), 1);
    }

    /// 偏好中文标题时，中国站的 `titleCn` 应被采用。
    #[test]
    fn cn_problem_list_can_prefer_chinese_title() {
        let raw = r#"{"data":{"problemsetQuestionList":{"total":1,"questions":[
            {"frontendQuestionId":"1","title":"Two Sum","titleCn":"两数之和",
             "titleSlug":"two-sum","difficulty":"EASY"}]}}}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        let node = resp
            .data
            .unwrap()
            .problemset_question_list
            .unwrap()
            .questions
            .into_iter()
            .next()
            .unwrap();
        let p = node.into_problem_with_locale(true).unwrap();
        assert_eq!(p.title, "两数之和");
        assert_eq!(p.frontend_id, "1");
    }

    /// 中文标题为空时必须回退到英文，不能产生空标题。
    #[test]
    fn cn_blank_chinese_title_falls_back_to_english() {
        let raw = r#"{"data":{"problemsetQuestionList":{"total":1,"questions":[
            {"frontendQuestionId":"1","title":"Two Sum","titleCn":"   ",
             "titleSlug":"two-sum","difficulty":"EASY"}]}}}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        let node = resp
            .data
            .unwrap()
            .problemset_question_list
            .unwrap()
            .questions
            .into_iter()
            .next()
            .unwrap();
        let p = node.into_problem_with_locale(true).unwrap();
        assert_eq!(p.title, "Two Sum", "空白中文标题应回退到英文");
    }

    /// 国际站的样本必须仍然能解析（两站兼容是双向的，不能修好一个弄坏另一个）。
    #[test]
    fn com_shape_still_parses_alongside_cn_support() {
        let raw = r#"{"data":{"problemsetQuestionList":{"total":4059,"questions":[
            {"acRate":57.9,"difficulty":"Easy","questionFrontendId":"1",
             "questionId":"1","title":"Two Sum","titleSlug":"two-sum",
             "topicTags":[{"name":"Array","slug":"array"}],
             "status":null,"isPaidOnly":false}]}}}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        let page = resp.data.unwrap().problemset_question_list.unwrap();
        assert_eq!(page.total, 4059);
        assert_eq!(page.has_more, None, "国际站不返回 hasMore，应为 None");
        let p = page
            .questions
            .into_iter()
            .next()
            .unwrap()
            .into_problem()
            .unwrap();
        assert_eq!(p.frontend_id, "1");
        assert_eq!(p.difficulty, Difficulty::Easy);
    }

    #[test]
    fn nonexistent_username_yields_none() {
        let raw = r#"{"data":{"matchedUser":null}}"#;
        let resp: GraphQlResponse<UserProfileData> = serde_json::from_str(raw).unwrap();
        assert!(resp.data.unwrap().into_profile("ghost", false).is_none());
    }

    #[test]
    fn tag_stats_merge_dedupes_and_sorts_deterministically() {
        let raw = r#"{"data":{"matchedUser":{"tagProblemCounts":{
            "advanced":[{"tagName":"Dynamic Programming","tagSlug":"dynamic-programming","problemsSolved":9},
                        {"tagName":"Game Theory","tagSlug":"game-theory","problemsSolved":1}],
            "intermediate":[{"tagName":"DP Dup","tagSlug":"dynamic-programming","problemsSolved":9}],
            "fundamental":[{"tagName":"Array","tagSlug":"array","problemsSolved":75}]}}}}"#;
        let resp: GraphQlResponse<TagStatsData> = serde_json::from_str(raw).unwrap();
        let tags = resp.data.unwrap().matched_user.unwrap().tag_problem_counts.unwrap().into_progress();
        assert_eq!(tags.len(), 3, "重复 slug 应被合并");
        // 排序：先按 solved 降序，再按 slug 升序。
        assert_eq!(tags[0].tag_slug, "array");
        assert_eq!(tags[0].solved, 75);
        assert_eq!(tags[1].tag_slug, "dynamic-programming");
        assert_eq!(tags[2].tag_slug, "game-theory");
    }

    #[test]
    fn calendar_string_is_parsed_into_sorted_dates() {
        let node = CalendarNode {
            streak: Some(5),
            total_active_days: Some(100),
            // 1700000000 = 2023-11-14 (UTC), 1700086400 = 2023-11-15 (UTC)
            submission_calendar: Some(
                r#"{"1700086400":3,"1700000000":"2","badkey":9}"#.to_string(),
            ),
        };
        let cal = node.parse_calendar();
        assert_eq!(cal.len(), 2, "无效键应被丢弃");
        assert!(cal[0].0 < cal[1].0, "结果应按时间升序");
        assert_eq!(cal[0].1, 2);
        assert_eq!(cal[1].1, 3);
    }

    #[test]
    fn calendar_parse_handles_garbage_without_panicking() {
        let node = CalendarNode {
            streak: None,
            total_active_days: None,
            submission_calendar: Some("not json at all".into()),
        };
        assert!(node.parse_calendar().is_empty());
    }

    #[test]
    fn submission_list_null_submissions_is_tolerated() {
        // 实测：匿名请求返回 submissions 为 null 且无 errors。
        let raw = r#"{"data":{"submissionList":{"lastKey":null,"hasNext":null,"submissions":null}}}"#;
        let resp: GraphQlResponse<SubmissionListData> = serde_json::from_str(raw).unwrap();
        assert!(resp
            .data
            .unwrap()
            .submission_list
            .unwrap()
            .submissions
            .is_none());
    }

    #[test]
    fn submission_accepted_detection_uses_machine_status_not_display() {
        // statusDisplay 可能是本地化文本，判定必须依赖 status。
        let node = SubmissionNode {
            id: Some("1".into()),
            title: Some("Two Sum".into()),
            title_slug: Some("two-sum".into()),
            status: Some("Accepted".into()),
            status_display: Some("通过".into()),
            timestamp: Some("1700000000".into()),
            lang: Some("rust".into()),
        };
        let s = node.into_recent().unwrap();
        assert!(s.is_accepted);
        assert_eq!(s.submitted_at, Some(1700000000));
    }

    #[test]
    fn submission_wrong_answer_is_not_accepted() {
        let node = SubmissionNode {
            id: None,
            title: Some("X".into()),
            title_slug: None,
            status: Some("Wrong Answer".into()),
            status_display: Some("解答错误".into()),
            timestamp: None,
            lang: None,
        };
        let s = node.into_recent().unwrap();
        assert!(!s.is_accepted);
        assert_eq!(s.title_slug, "");
    }

    #[test]
    fn contest_history_filters_unattended_and_sorts() {
        let raw = r#"{"data":{"userContestRankingHistory":[
            {"attended":true,"problemsSolved":3,"totalProblems":4,"rating":1500.0,"ranking":500,
             "contest":{"title":"Weekly 380","startTime":1700000000}},
            {"attended":false,"problemsSolved":0,"totalProblems":4,"rating":0.0,"ranking":0,
             "contest":{"title":"Weekly 379","startTime":1699000000}},
            {"attended":true,"problemsSolved":4,"totalProblems":4,"rating":1550.0,"ranking":300,
             "contest":{"title":"Weekly 381","startTime":1699500000}}]}}"#;
        let resp: GraphQlResponse<ContestHistoryData> = serde_json::from_str(raw).unwrap();
        let records = resp.data.unwrap().into_records();
        assert_eq!(records.len(), 2, "未参赛场次必须被过滤");
        assert!(records[0].start_time < records[1].start_time, "应按时间升序");
        assert_eq!(records[0].rating, 1550.0);
        assert_eq!(records[1].rating, 1500.0);
    }

    #[test]
    fn contest_history_skips_records_without_rating() {
        let raw = r#"{"data":{"userContestRankingHistory":[
            {"attended":true,"problemsSolved":3,"totalProblems":4,"ranking":500,
             "contest":{"title":"X","startTime":1700000000}}]}}"#;
        let resp: GraphQlResponse<ContestHistoryData> = serde_json::from_str(raw).unwrap();
        assert!(resp.data.unwrap().into_records().is_empty(), "缺 rating 的记录应跳过");
    }

    #[test]
    fn contest_ranking_null_is_graceful() {
        // 实测：无参赛记录的用户返回 null，属正常情况。
        let raw = r#"{"data":{"userContestRanking":null,"userContestRankingHistory":[]}}"#;
        let resp: GraphQlResponse<ContestRankingData> = serde_json::from_str(raw).unwrap();
        let s = resp.data.unwrap().into_summary();
        assert_eq!(s.attended_count, 0);
        assert_eq!(s.rating, None);
    }

    #[test]
    fn graphql_errors_are_parsed_and_classified() {
        let raw = r#"{"errors":[{"message":"no permission to check the calendar.",
            "locations":[{"line":1,"column":96}],"path":["matchedUser","userCalendar"],
            "extensions":{"handled":true}}],"data":{"matchedUser":null}}"#;
        let resp: GraphQlResponse<CalendarData> = serde_json::from_str(raw).unwrap();
        let errs = resp.errors.unwrap();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].is_permission_denied(), "应识别为权限错误");
        assert!(errs[0].describe().contains("matchedUser.userCalendar"));
    }

    #[test]
    fn graphql_error_path_with_integer_index_renders() {
        let raw = r#"{"errors":[{"message":"boom","path":["a",0,"b"]}]}"#;
        let resp: GraphQlResponse<serde_json::Value> = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.errors.unwrap()[0].describe(), "boom (路径: a.0.b)");
    }

    #[test]
    fn response_with_only_errors_has_null_data() {
        let raw = r#"{"errors":[{"message":"Syntax Error: Expected Name, found }"}]}"#;
        let resp: GraphQlResponse<ProblemListData> = serde_json::from_str(raw).unwrap();
        assert!(resp.data.is_none());
        assert!(!resp.errors.unwrap()[0].is_permission_denied());
    }

    #[test]
    fn user_status_detects_signin_state() {
        let raw = r#"{"data":{"userStatus":{"isSignedIn":true,"username":"alice"}}}"#;
        let resp: GraphQlResponse<UserStatusData> = serde_json::from_str(raw).unwrap();
        let s = resp.data.unwrap().user_status.unwrap();
        assert_eq!(s.is_signed_in, Some(true));
        assert_eq!(s.username.as_deref(), Some("alice"));
    }

    /// **本组测试对应一个真实缺陷。**
    ///
    /// 中国站的 `userStatus` 额外返回 `userSlug`，而它才是查询账号数据时
    /// 该用的标识。昵称可以是中文（与 slug 不同），此时若拿昵称去查，
    /// 服务端返回 `null`，界面误报"用户不存在"。
    ///
    /// 该 JSON 取自对中国站的实测响应。
    #[test]
    fn cn_user_status_parses_user_slug() {
        let raw = r#"{"data":{"userStatus":{"isSignedIn":true,"username":"梧糊",
                             "userSlug":"wuhu","realName":"吴湖"}}}"#;
        let resp: GraphQlResponse<UserStatusData> = serde_json::from_str(raw).unwrap();
        let s = resp.data.unwrap().user_status.unwrap();
        assert_eq!(s.is_signed_in, Some(true));
        assert_eq!(s.username.as_deref(), Some("梧糊"));
        // 关键：slug 必须被解析出来——它是唯一可用的查询标识。
        assert_eq!(s.user_slug.as_deref(), Some("wuhu"));
        assert_eq!(s.real_name.as_deref(), Some("吴湖"));
    }

    /// 国际站的 `userStatus` 不含 `userSlug`，解析必须仍然成功。
    ///
    /// 同一个 DTO 服务两站，若把 `user_slug` 当作必需字段，国际站会在
    /// 反序列化阶段直接失败——这是上一轮"两站同构"错误的翻版。
    #[test]
    fn com_user_status_parses_without_slug() {
        let raw = r#"{"data":{"userStatus":{"isSignedIn":true,"username":"alice"}}}"#;
        let resp: GraphQlResponse<UserStatusData> = serde_json::from_str(raw).unwrap();
        let s = resp.data.unwrap().user_status.unwrap();
        assert_eq!(s.user_slug, None, "国际站无 slug，应为 None 而非报错");
    }
}
