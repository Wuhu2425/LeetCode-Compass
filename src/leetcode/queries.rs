//! GraphQL 查询定义。
//!
//! ## 为什么集中在此
//!
//! LeetCode 的 GraphQL 端点是非官方接口，没有版本承诺，字段随时可能变更。
//! 把全部查询字符串收敛到一个文件，使得接口变更时只需修改单点，且便于
//! 与实测结果对照。
//!
//! ## 实测依据
//!
//! 下列查询均已通过 `curl` 对 `https://leetcode.com/graphql` 实测验证
//! （2026-09-22）。重要实测结论见各查询上方的注释。
//!
//! ## 字段命名陷阱
//!
//! `QuestionNode` 与 `QuestionListFilterInput` 的字段命名并不统一：
//! - 列表返回的节点类型是 `QuestionNode`，其题号字段是 **`questionFrontendId`**，
//!   而非 `frontendQuestionId`（后者在 `problem` / `question` 类型上才存在）。
//!   实测中误用该字段会直接返回
//!   `Cannot query field "frontendQuestionId" on type "QuestionNode"`。
//! - 输入过滤器的难度枚举是大写（`EASY`/`MEDIUM`/`HARD`），而输出是
//!   首字母大写（`Easy`/`Medium`/`Hard`）。二者不可混用。

// ---------------------------------------------------------------------------
// 题库列表
// ---------------------------------------------------------------------------

/// 分页拉取题库列表。
///
/// 变量：
/// - `$categorySlug`：分类，空字符串表示全站。
/// - `$limit`：单页条数。实测上限可接受 100；过大需留意响应体尺寸。
/// - `$skip`：偏移量，用于分页。
/// - `$filters`：过滤条件（难度、状态、标签、关键词）。
///
/// 说明：`status` 字段在**未认证**时恒为 `null`。若未配置会话凭据却传入
/// `filters.status`，服务端会静默返回空结果集（实测 `total: 0`），而非报错。
/// 因此本应用在未认证时**不传** `status` 过滤条件，改由客户端侧处理。
pub const PROBLEM_LIST: &str = r#"
query problemsetQuestionList(
  $categorySlug: String
  $limit: Int
  $skip: Int
  $filters: QuestionListFilterInput
) {
  problemsetQuestionList: questionList(
    categorySlug: $categorySlug
    limit: $limit
    skip: $skip
    filters: $filters
  ) {
    total: totalNum
    questions: data {
      questionId
      questionFrontendId
      title
      titleSlug
      difficulty
      acRate
      status
      isPaidOnly
      topicTags {
        name
        slug
      }
    }
  }
}
"#;

/// 全站题目统计（按难度）。匿名可用。
///
/// 返回四种条目：`All` / `Easy` / `Medium` / `Hard`。
/// 实测值（2026-09-22）：4059 / 966 / 2117 / 976。
/// 注意：该查询使用 `allQuestionsCount`，**不需要**变量。
pub const GLOBAL_QUESTION_COUNTS: &str = r#"
query globalQuestionCounts {
  allQuestionsCount {
    difficulty
    count
  }
}
"#;

// ---------------------------------------------------------------------------
// 用户画像
// ---------------------------------------------------------------------------

/// 用户公开画像 + 解题统计。匿名可用。
///
/// 变量：
/// - `$username`：LeetCode 用户名。
///
/// 返回 `matchedUser` 为 `null` 表示用户名不存在。
///
/// `submitStatsGlobal.acSubmissionNum` 是按难度分组的已解题数数组，
/// 元素形如 `{ difficulty: "Easy", count: 123 }`，固定含 `All` 一项。
///
/// 注意：`profile.ranking` 可能为 0，表示尚未上榜。
pub const USER_PROFILE: &str = r#"
query getUserProfile($username: String!) {
  matchedUser(username: $username) {
    username
    profile {
      realName
      ranking
      reputation
    }
    submitStatsGlobal {
      acSubmissionNum {
        difficulty
        count
      }
      totalSubmissionNum {
        difficulty
        count
      }
    }
  }
}
"#;

/// 用户各标签的解题统计。匿名可用。
///
/// 这是**无认证模式下推荐算法的核心输入**：即使拿不到逐题完成状态，
/// 也能知道用户在哪些知识点上练得多、哪些练得少。
///
/// 返回三组数组（`advanced` / `intermediate` / `fundamental`），
/// 元素形如 `{ tagName: "Dynamic Programming", tagSlug: "dynamic-programming",
/// problemsSolved: 9 }`。
pub const USER_TAG_STATS: &str = r#"
query skillStats($username: String!) {
  matchedUser(username: $username) {
    tagProblemCounts {
      advanced {
        tagName
        tagSlug
        problemsSolved
      }
      intermediate {
        tagName
        tagSlug
        problemsSolved
      }
      fundamental {
        tagName
        tagSlug
        problemsSolved
      }
    }
  }
}
"#;

/// 用户提交日历。**需要认证**。
///
/// 实测：匿名调用返回 `no permission to check the calendar.`，
/// 且 `matchedUser` 为 `null`。因此调用方必须做好错误兜底。
///
/// 变量：
/// - `$username`：用户名。
/// - `$year`：年份，可为 `null` 取当前年。
///
/// `submissionCalendar` 是一个 **JSON 字符串**（不是对象），键为 Unix 秒
/// 时间戳字符串，值为当日提交次数。需要二次解析。
pub const USER_CALENDAR: &str = r#"
query userProfileCalendar($username: String!, $year: Int) {
  matchedUser(username: $username) {
    userCalendar(year: $year) {
      activeYears
      streak
      totalActiveDays
      submissionCalendar
    }
  }
}
"#;

// ---------------------------------------------------------------------------
// 提交记录
// ---------------------------------------------------------------------------

/// 最近提交记录。**需要认证**。
///
/// 实测：匿名调用返回 `submissions: null`（无 `errors` 字段），
/// 说明服务端对未登录请求静默返回空。调用方需将其视为"无数据"而非"出错"。
///
/// 变量：
/// - `$offset` / `$limit`：分页。
/// - `$lastKey`：游标；首屏传 `null`。
/// - `$questionSlug`：限定题目；查全部传 `null`。
///
/// `timestamp` 是 **字符串** 形式的 Unix 秒，需二次解析。
pub const SUBMISSION_LIST: &str = r#"
query submissionList(
  $offset: Int!
  $limit: Int!
  $lastKey: String
  $questionSlug: String
) {
  submissionList(
    offset: $offset
    limit: $limit
    lastKey: $lastKey
    questionSlug: $questionSlug
  ) {
    lastKey
    hasNext
    submissions {
      id
      title
      titleSlug
      status
      statusDisplay
      timestamp
      lang
      runtime
      memory
    }
  }
}
"#;

// ---------------------------------------------------------------------------
// 竞赛
// ---------------------------------------------------------------------------

/// 竞赛排名摘要。匿名可用（对存在竞赛记录的公开用户）。
///
/// 实测：对没有参赛记录的用户返回 `userContestRanking: null`，
/// 这是**正常情况**而非错误，UI 需展示"暂无参赛记录"。
pub const USER_CONTEST_RANKING: &str = r#"
query userContestRankingInfo($username: String!) {
  userContestRanking(username: $username) {
    attendedContestsCount
    rating
    globalRanking
    totalParticipants
    topPercentage
  }
}
"#;

/// 竞赛历史。匿名可用（对存在竞赛记录的公开用户）。
///
/// `userContestRankingHistory` 是数组，含**未实际参赛**的条目
/// （`attended: false`）。分析时必须过滤，否则会把"注册未参加"计入统计。
///
/// 嵌套结构：
/// - `contest { title startTime }`，`startTime` 为 Unix 秒整数。
/// - `rating` / `ranking` 为数值。
pub const USER_CONTEST_HISTORY: &str = r#"
query userContestRankingHistory($username: String!) {
  userContestRankingHistory(username: $username) {
    attended
    trendDirection
    problemsSolved
    totalProblems
    finishTimeInSeconds
    rating
    ranking
    contest {
      title
      startTime
    }
  }
}
"#;

// ---------------------------------------------------------------------------
// 连通性探测
// ---------------------------------------------------------------------------

/// 轻量查询，用于验证网络可达性与凭据有效性。
///
/// 选择 `userStatus` 是因为它**同时反映登录态**：已登录时返回
/// `isSignedIn: true` 与用户名，未登录时返回 `isSignedIn: false`。
/// 这是判断 session cookie 是否仍然有效的可靠手段。
pub const USER_STATUS: &str = r#"
query userStatus {
  userStatus {
    isSignedIn
    username
  }
}
"#;

/// 登录状态探测（中国站）。
///
/// 与国际站的 `USER_STATUS` 相比多请求了 **`userSlug`** 字段，
/// 这是中国站特有的、也是**正确**的用户标识。
///
/// ## 为什么需要 userSlug
///
/// 中国站的 `userProfilePublicProfile(userSlug:)` 按 **slug** 查找用户，
/// 而 slug 与「显示昵称」**不是一回事**：
///
/// - 用户可以在个人设置里把显示昵称改成中文（如「梧糊」），
///   但 slug 仍是注册时确定的 ASCII 标识（如 `wuhu`）。
/// - 用中文昵称去查 `userProfilePublicProfile` 会返回 `null`，
///   看起来像"用户不存在"，实际只是查错了字段。
/// - 个人主页 URL 用的是 slug：`https://leetcode.cn/u/<slug>/`。
///
/// 实测（2026-09-22）：`userSlug: "梧糊"` → `null`；
/// 同一账号用其 ASCII slug 查询 → 正常返回。
///
/// 因此登录态下应**优先用 `userSlug` 作为查询标识**，
/// 而不是用户在设置页填的显示昵称。
///
/// 注意：`userSlug` 仅在已登录时非空；匿名时为 `null`。
/// 国际站没有此字段（该查询不可在国际站使用）。
pub const CN_USER_STATUS: &str = r#"
query userStatus {
  userStatus {
    isSignedIn
    username
    userSlug
    realName
  }
}
"#;

// ===========================================================================
// 中国站（leetcode.cn）
// ===========================================================================
//
// ## 为什么需要一整套独立的查询
//
// **国际站与中国站的 GraphQL schema 完全不同**——不是同一套 schema 部署在
// 两个域名，而是两套字段命名约定各异的 schema。以下为 2026-09-22 对
// `https://leetcode.cn/graphql` 的实测结论：
//
// | 国际站 | 中国站 |
// |--------|--------|
// | `matchedUser(username:)` | `userProfilePublicProfile(userSlug:)` |
// | `allQuestionsCount` | **不存在** |
// | `problemsetQuestionList.total: totalNum` | `problemsetQuestionList.total` |
// | `questions: data` | `questions`（无别名） |
// | 节点 `QuestionNode.questionFrontendId` | 节点 `QuestionLightNode.frontendQuestionId` |
// | `isPaidOnly` | `paidOnly` |
// | 难度 `"Easy"` | 难度 `"EASY"` |
// | `matchedUser.tagProblemCounts` | **不存在** |
// | `matchedUser.userCalendar` | **不存在** |
// | `userContestRanking(username:)` | 存在但参数与字段名均不同 |
//
// 之前把这两站当作"同一 API 换个域名"，只切换了端点与 Origin/Referer，
// 结果中国站的每个请求都返回 HTTP 400
// `Cannot query field "xxx" on type "Query".`。
//
// ## 探测被禁用内省的方法
//
// 中国站禁用了 `__schema` 内省（返回空 message 的 error）。但 GraphQL 的
// 校验错误消息本身会泄露 schema 信息：
// - `Field "X" of type "T" must have a sub selection` → 泄露类型名 `T`
// - `Unknown argument "a" on field "b"` → 泄露正确参数名
// - `Cannot query field "x" on type "Y"` → 可用于排除错误字段名
//
// 上述映射表即通过该方式逐字段试出，而非猜测。

/// 用户公开画像（中国站）。
///
/// 对应国际站的 `USER_PROFILE`，但结构差异显著：
///
/// - 参数名是 `userSlug`（**不是** `username`）。
/// - 排名有两个来源：`siteRanking`（整数，等价于国际站的
///   `profile.ranking`）与 `profile.ranking.ranking`。
///   后者是**一个 JSON 数组的字符串**，形如 `"[0,0,...,1146,0,1470,...]"`，
///   需二次解析。本应用取 `siteRanking`，更直接且已足够。
/// - **不返回解题统计**，需另发 `CN_USER_PROGRESS`。
/// - 无 `topicTags` 统计、无日历。
///
/// 实测（`userSlug: "leetcode"`）返回 200，含
/// `{"username":"LeetCode","siteRanking":100000,...}`。
///
/// 注意：用户名不存在时返回 `{"userProfilePublicProfile":null}`（200），
/// 调用方需按"未找到"处理。
pub const CN_USER_PROFILE: &str = r#"
query userProfilePublicProfile($userSlug: String!) {
  userProfilePublicProfile(userSlug: $userSlug) {
    username
    siteRanking
    profile {
      realName
      reputation
      userAvatar
    }
  }
}
"#;

/// 用户解题进度（中国站）。
///
/// 这是中国站**唯一**能拿到"已解题数"的接口，取代国际站的
/// `matchedUser.submitStatsGlobal.acSubmissionNum`。
///
/// 两个数组都要请求：
/// - `numAcceptedQuestions`：已通过数
/// - `numFailedQuestions`：未通过数（**远端直接给出**，无需像国际站那样
///   用"总提交 - 通过"做减法）
///
/// 难度枚举是**全大写**（`EASY`/`MEDIUM`/`HARD`），国际站是首字母大写。
/// 解析层需兼容两种写法。
///
/// 实测（`userSlug: "leetcode"`）返回
/// `{"numAcceptedQuestions":[{"difficulty":"EASY","count":25},...]}`。
pub const CN_USER_PROGRESS: &str = r#"
query userProfileUserQuestionProgressV2($userSlug: String!) {
  userProfileUserQuestionProgressV2(userSlug: $userSlug) {
    numAcceptedQuestions {
      difficulty
      count
    }
    numFailedQuestions {
      difficulty
      count
    }
  }
}
"#;

/// 分页拉取题库列表（中国站）。
///
/// 与国际站的 `PROBLEM_LIST` 差异：
///
/// - 总数用 **`total`**（国际站是别名 `totalNum`）。
/// - 题目数组直接在根下叫 **`questions`**（国际站是别名 `data`）。
/// - 另提供 `hasMore`，可作分页终止的补充信号。
/// - 节点类型是 `QuestionLightNode`，题号字段是
///   **`frontendQuestionId`**——与国际站的 `questionFrontendId`
///   **恰好相反**。这是最容易写错的一处。
/// - 付费标记是 **`paidOnly`**（国际站 `isPaidOnly`）。
/// - 难度输出**全大写**。
///
/// 额外好处：中国站节点直接带 `titleCn` 与 `topicTags[].nameTranslated`，
/// 可展示中文标题。
///
/// 实测（`limit: 3`）返回 `{"total":4447,"hasMore":true,"questions":[...]}`。
pub const CN_PROBLEM_LIST: &str = r#"
query problemsetQuestionList(
  $categorySlug: String
  $limit: Int
  $skip: Int
  $filters: QuestionListFilterInput
) {
  problemsetQuestionList(
    categorySlug: $categorySlug
    limit: $limit
    skip: $skip
    filters: $filters
  ) {
    total
    hasMore
    questions {
      frontendQuestionId
      title
      titleCn
      titleSlug
      difficulty
      acRate
      status
      paidOnly
      topicTags {
        name
        slug
        nameTranslated
      }
    }
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// 保证查询常量都是可用的 GraphQL 文本。
    /// 无法在此做真实语法校验，但可以挡住最廉价的错误：空串、括号不配对。
    #[test]
    fn all_queries_are_non_empty_and_balanced() {
        let cases: [(&str, &str); 8] = [
            ("PROBLEM_LIST", PROBLEM_LIST),
            ("GLOBAL_QUESTION_COUNTS", GLOBAL_QUESTION_COUNTS),
            ("USER_PROFILE", USER_PROFILE),
            ("USER_TAG_STATS", USER_TAG_STATS),
            ("USER_CALENDAR", USER_CALENDAR),
            ("SUBMISSION_LIST", SUBMISSION_LIST),
            ("USER_CONTEST_RANKING", USER_CONTEST_RANKING),
            ("USER_CONTEST_HISTORY", USER_CONTEST_HISTORY),
        ];
        for (name, q) in cases {
            assert!(!q.trim().is_empty(), "{name} 不应为空");
            let open = q.matches('{').count();
            let close = q.matches('}').count();
            assert_eq!(open, close, "{name} 大括号不配对: {open} vs {close}");
            let paren_open = q.matches('(').count();
            let paren_close = q.matches(')').count();
            assert_eq!(
                paren_open, paren_close,
                "{name} 小括号不配对: {paren_open} vs {paren_close}"
            );
            assert_eq!(q.matches('"').count() % 2, 0, "{name} 引号数量为奇数");
        }
    }

    /// 回归防护：必须使用 `questionFrontendId`。
    ///
    /// 实测中 `QuestionNode` 类型上不存在 `frontendQuestionId` 字段，
    /// 误用会导致整个查询失败。此测试锁死正确字段名。
    #[test]
    fn problem_list_uses_correct_frontend_id_field() {
        assert!(
            PROBLEM_LIST.contains("questionFrontendId"),
            "PROBLEM_LIST 必须请求 questionFrontendId"
        );
        assert!(
            !PROBLEM_LIST.contains("frontendQuestionId"),
            "QuestionNode 上不存在 frontendQuestionId，会导致查询失败"
        );
    }

    /// 确保每个带变量的查询都声明了对应变量，避免漏写 $ 符号。
    #[test]
    fn queries_declaring_variables_use_them() {
        for (name, q, var) in [
            ("USER_PROFILE", USER_PROFILE, "$username"),
            ("USER_TAG_STATS", USER_TAG_STATS, "$username"),
            ("USER_CONTEST_RANKING", USER_CONTEST_RANKING, "$username"),
            ("USER_CONTEST_HISTORY", USER_CONTEST_HISTORY, "$username"),
            ("PROBLEM_LIST", PROBLEM_LIST, "$filters"),
        ] {
            assert!(q.contains(&format!("{var}:")) || q.contains(&format!("{var} ")));
            let uses = q.matches(var).count();
            assert!(uses >= 2, "{name} 声明了 {var} 但未使用（出现 {uses} 次）");
        }
    }

    #[test]
    fn operation_names_are_present_and_unique() {
        let names = [
            "problemsetQuestionList",
            "globalQuestionCounts",
            "getUserProfile",
            "skillStats",
            "userProfileCalendar",
            "submissionList",
            "userContestRankingInfo",
            "userContestRankingHistory",
            "userStatus",
        ];
        let all = [
            PROBLEM_LIST,
            GLOBAL_QUESTION_COUNTS,
            USER_PROFILE,
            USER_TAG_STATS,
            USER_CALENDAR,
            SUBMISSION_LIST,
            USER_CONTEST_RANKING,
            USER_CONTEST_HISTORY,
            USER_STATUS,
        ];
        for (name, text) in names.iter().zip(all.iter()) {
            assert!(
                text.contains(&format!("query {name}")),
                "缺少 operation 名 {name}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 中国站查询的字段名回归防护
    // -----------------------------------------------------------------------
    //
    // 这些测试的价值在于：两站字段名有若干处**恰好互为倒序**或**极其相似**
    // （`questionFrontendId` ↔ `frontendQuestionId`、
    // `isPaidOnly` ↔ `paidOnly`、`totalNum` ↔ `total`）。
    // 写错不会编译报错，只会在运行时得到一个 HTTP 400，
    // 而 400 的成因极难从错误文案反推。因此必须用测试锁死。

    /// 中国站的题号字段是 `frontendQuestionId`，与国际站**恰好相反**。
    ///
    /// 这是全项目最容易写错的一处字段名。
    #[test]
    fn cn_problem_list_uses_cn_frontend_id_field() {
        assert!(
            CN_PROBLEM_LIST.contains("frontendQuestionId"),
            "中国站 QuestionLightNode 的题号字段是 frontendQuestionId"
        );
        assert!(
            !CN_PROBLEM_LIST.contains("questionFrontendId"),
            "中国站不存在 questionFrontendId，会返回 HTTP 400"
        );
    }

    /// 中国站付费标记是 `paidOnly`，国际站是 `isPaidOnly`。
    #[test]
    fn cn_problem_list_uses_paid_only_field() {
        assert!(CN_PROBLEM_LIST.contains("paidOnly"));
        assert!(!CN_PROBLEM_LIST.contains("isPaidOnly"));
    }

    /// 中国站总数是 `total`，没有 `totalNum`。
    #[test]
    fn cn_problem_list_uses_total_not_total_num() {
        assert!(CN_PROBLEM_LIST.contains("total"));
        assert!(
            !CN_PROBLEM_LIST.contains("totalNum"),
            "中国站 QuestionListNode 上不存在 totalNum"
        );
        // 中国站的题目数组直接在根下，不是 data 别名。
        assert!(
            !CN_PROBLEM_LIST.contains("questions: data"),
            "中国站不提供 data 别名"
        );
    }

    /// 中国站用户查询的参数名是 `userSlug`，不是 `username`。
    #[test]
    fn cn_user_queries_use_user_slug_argument() {
        for (name, q) in [
            ("CN_USER_PROFILE", CN_USER_PROFILE),
            ("CN_USER_PROGRESS", CN_USER_PROGRESS),
        ] {
            assert!(
                q.contains("userSlug: $userSlug"),
                "{name} 必须用 userSlug 参数"
            );
            assert!(
                !q.contains("username: $username"),
                "{name} 用的是 username，中国站会报 Unknown argument"
            );
        }
    }

    /// 中国站必须用 `userProfilePublicProfile`，国际站的 `matchedUser` 不存在。
    #[test]
    fn cn_queries_avoid_com_only_root_fields() {
        for (name, q) in [
            ("CN_USER_PROFILE", CN_USER_PROFILE),
            ("CN_USER_PROGRESS", CN_USER_PROGRESS),
            ("CN_PROBLEM_LIST", CN_PROBLEM_LIST),
        ] {
            for forbidden in ["matchedUser", "allQuestionsCount", "tagProblemCounts"] {
                assert!(
                    !q.contains(forbidden),
                    "{name} 含国际站专有字段 {forbidden}，在中国站会返回 HTTP 400"
                );
            }
        }
    }

    /// 国际站查询不得被中国站字段污染（双向防护）。
    #[test]
    fn com_queries_are_not_polluted_by_cn_field_names() {
        for (name, q) in [
            ("PROBLEM_LIST", PROBLEM_LIST),
            ("USER_PROFILE", USER_PROFILE),
            ("USER_TAG_STATS", USER_TAG_STATS),
        ] {
            assert!(
                !q.contains("frontendQuestionId"),
                "{name} 含中国站字段名，国际站会返回 HTTP 400"
            );
            assert!(
                !q.contains("userSlug"),
                "{name} 含中国站参数名 userSlug"
            );
        }
    }

    /// 中国站查询同样要满足通用的结构合法性（括号配对、非空）。
    #[test]
    fn cn_queries_are_non_empty_and_balanced() {
        let cases: [(&str, &str); 4] = [
            ("CN_USER_PROFILE", CN_USER_PROFILE),
            ("CN_USER_PROGRESS", CN_USER_PROGRESS),
            ("CN_PROBLEM_LIST", CN_PROBLEM_LIST),
            ("CN_USER_STATUS", CN_USER_STATUS),
        ];
        for (name, q) in cases {
            assert!(!q.trim().is_empty(), "{name} 不应为空");
            assert_eq!(
                q.matches('{').count(),
                q.matches('}').count(),
                "{name} 大括号不配对"
            );
            assert_eq!(
                q.matches('(').count(),
                q.matches(')').count(),
                "{name} 小括号不配对"
            );
            assert_eq!(q.matches('"').count() % 2, 0, "{name} 引号数量为奇数");
        }
    }

    /// **本组测试对应一个真实缺陷。**
    ///
    /// 中国站的账号查询标识（`userSlug`）**不等于**显示昵称：用户可以把
    /// 昵称改成中文，但 slug 仍是注册时的 ASCII 名。用户通常只知道昵称，
    /// 于是拿昵称去查一律 `null`。
    ///
    /// `userStatus` 是唯一的补救渠道——它由服务端返回当前登录态的
    /// `userSlug`。因此这个查询**必须同时取 `userSlug` 和展示字段**：
    /// 少了 `userSlug` 就无法自动纠正用户填错的名称。
    #[test]
    fn cn_user_status_exposes_slug_for_identity_resolution() {
        assert!(
            CN_USER_STATUS.contains("userSlug"),
            "CN_USER_STATUS 必须取 userSlug——这是唯一能自动解析出正确标识的字段"
        );
        assert!(
            CN_USER_STATUS.contains("isSignedIn"),
            "必须判断登录态，否则未登录时无从区分"
        );
        // 昵称/真名用于展示，缺失时用户看不出"已登录为谁"。
        assert!(CN_USER_STATUS.contains("username"), "应取 username 供展示");
    }

    /// `userStatus` 是**两站通用**的根字段，参数表为空。
    ///
    /// 这一条把"中国站不需要 `username` 参数"的事实固化下来，避免有人
    /// 好心地把 `USER_STATUS` 的参数（若有）复制过来。
    #[test]
    fn cn_user_status_takes_no_arguments() {
        assert!(
            !CN_USER_STATUS.contains('$'),
            "userStatus 不接受参数，出现了 $ 说明误加了变量"
        );
    }
}
