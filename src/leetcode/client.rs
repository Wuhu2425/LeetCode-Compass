//! LeetCode GraphQL HTTP 客户端。
//!
//! ## 职责
//!
//! - 构造并发送 GraphQL 请求（统一端点 `https://leetcode.com/graphql`）
//! - 按需注入会话凭据（`Cookie` + `x-csrftoken` 头）
//! - 将 HTTP / GraphQL 层面的失败统一映射为 `AppError`
//! - 提供分页拉取题库的便捷方法
//!
//! ## 凭据处理（关键实现细节）
//!
//! LeetCode 的 GraphQL 端点带 Cookie 请求时校验 CSRF：
//! `x-csrftoken` 请求头必须与 `csrftoken` cookie 的值一致，否则 403。
//! 因此二者必须**成对注入**。本客户端只在 `session.is_complete()` 为真
//! 时才注入，避免"只带 session 不带 csrftoken"造成的必然失败。
//!
//! 凭据通过每次请求动态读取（`Arc<RwLock<AppConfig>>`）而非固化在
//! 客户端构造时，这样用户在设置页更新凭据后无需重建客户端。
//!
//! ## 匿名降级
//!
//! 未配置凭据时客户端仍然工作，只是拿不到 `status` 字段。调用方不应
//! 把这种情况当作错误。

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, COOKIE, USER_AGENT};
use serde_json::json;
use tokio::sync::RwLock;

use crate::config::{AppConfig, LeetCodeSession, LeetCodeSite};
use crate::error::{AppError, AppResult};
use crate::leetcode::queries;
use crate::leetcode::types::*;
use crate::models::Problem;

/// LeetCode GraphQL 端点（国际站，匿名请求的默认目标）。
///
/// 认证请求会根据用户选择的站点动态替换域名，见 `graphql_endpoint()`。
/// 国际站与中国站（leetcode.cn）账号体系独立，凭据不可混用。
const GRAPHQL_ENDPOINT: &str = "https://leetcode.com/graphql";

/// 单页拉取的题目数。100 是实测稳定的取值：既能控制请求数，又不会
/// 让响应体过大。调大可减少请求轮次但增加单次延迟与内存峰值。
pub const PAGE_SIZE: usize = 100;

/// 题库全量拉取的安全上限。
///
/// LeetCode 当前约 4000 题（2026-09）。设置 20000 的上限是为了在
/// 远端 `total` 字段异常（例如返回极大值）时避免无限循环拉取。
const MAX_TOTAL_GUARD: i64 = 20_000;

/// 单次拉取的最大页数，与上限配合形成双重保护。
const MAX_PAGES: usize = 250;

/// LeetCode GraphQL 客户端。
///
/// 内部持有 `reqwest::Client`（连接池复用），克隆成本低。
#[derive(Clone)]
pub struct LeetCodeClient {
    http: reqwest::Client,
    config: Arc<RwLock<AppConfig>>,
}

/// 已登录用户的身份信息。
///
/// ## 为什么需要区分 `slug` 与 `username`
///
/// 中国站（leetcode.cn）允许用户设置**中文显示昵称**，但账号还有一个
/// 注册时确定的 **ASCII slug**。关键区别：
///
/// - `userProfilePublicProfile(userSlug:)` 按 **slug** 查找。
/// - 个人主页 URL 也是 slug：`https://leetcode.cn/u/<slug>/`。
/// - 拿中文昵称去查会返回 `null`，表现为"用户不存在"，
///   实际只是**查错了字段**。
///
/// 实测（2026-09-22）：`userSlug: "梧糊"` → `null`；
/// 同一账号的 ASCII slug → 正常返回。
///
/// 因此已登录时必须用 `slug` 查询；`username` 仅用于展示。
///
/// ## 可序列化
///
/// 该结构会被持久化到本地（见 `storage::SessionCheck`）：校验一次即可
/// 长期复用，重启后不必重新询问服务端。`slug` 的持久化尤其重要——
/// 它是中国站的查询标识，丢了就得重新解析一遍。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LoginIdentity {
    /// 用于查询的标识（中国站为 ASCII slug；国际站与 username 相同）。
    pub slug: String,
    /// 显示昵称，可能包含中文。仅用于展示。
    pub username: String,
    /// 真实姓名（可选）。
    pub real_name: Option<String>,
}

impl LoginIdentity {
    /// 面向用户的展示文本。
    ///
    /// 当昵称与 slug 不同（中国站改过昵称的典型情况），两者都显示出来，
    /// 用户在设置页才能对照着看懂"为什么填昵称查不到"。
    pub fn display_label(&self) -> String {
        if self.username == self.slug {
            self.username.clone()
        } else {
            format!("{}（标识 {}）", self.username, self.slug)
        }
    }
}

impl LeetCodeClient {
    /// 创建客户端。
    ///
    /// `config` 与应用的共享配置是同一个 `Arc`，因此用户在设置页修改
    /// 凭据后立即生效。
    pub fn new(config: Arc<RwLock<AppConfig>>) -> AppResult<Self> {
        // 幂等安装加密后端。
        //
        // 这里再调一次而非只依赖 `main()`：单元测试不会经过 `main()`，
        // 若不在此处兜底，所有涉及 `reqwest::Client::builder().build()`
        // 的测试都会 panic。`install_default()` 重复调用是安全的。
        crate::error::install_crypto_provider();

        let http = reqwest::Client::builder()
            // 整体超时。LeetCode 偶有慢响应，30s 足够且不会让 UI 卡太久。
            .timeout(Duration::from_secs(30))
            // 连接超时单独设置，避免 DNS 或 TCP 阶段长时间挂起。
            .connect_timeout(Duration::from_secs(10))
            // 复用连接，减少 TLS 握手开销（分页拉取时效果明显）。
            .pool_max_idle_per_host(4)
            .gzip(true)
            // 不自动跟随重定向：GraphQL 端点不应发生重定向，
            // 若发生说明端点变更，应报错而非静默跟随。
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(AppError::Network)?;

        Ok(Self { http, config })
    }

    /// 读取当前凭据快照。返回 `None` 表示处于匿名模式。
    async fn current_session(&self) -> Option<LeetCodeSession> {
        let cfg = self.config.read().await;
        if cfg.session.is_complete() {
            Some(cfg.session.clone())
        } else {
            None
        }
    }

    /// 当前生效的站点。
    ///
    /// 已配置凭据时跟随凭据所属站点；匿名模式下默认国际站
    /// （两站的公开题库接口都能用，但字段不同，必须有一个确定的默认值）。
    ///
    /// 这是查询分派与能力判断的唯一事实来源——所有需要"按站点走不同分支"
    /// 的地方都应通过它取值，避免各处自行读取配置造成不一致。
    async fn current_site(&self) -> LeetCodeSite {
        let cfg = self.config.read().await;
        if cfg.session.is_complete() {
            cfg.session.site
        } else {
            LeetCodeSite::default()
        }
    }

    /// 构造"该功能在当前站点不可用"错误，附带站点中文名。
    async fn unsupported(&self, feature: &str) -> AppError {
        let site = self.current_site().await;
        AppError::unsupported_on_site(feature, site.label_zh())
    }

    /// 解析当前请求应使用的「端点 + 域名」。
    ///
    /// 抽出为独立方法便于测试，也集中了站点选择的唯一事实来源：
    /// 未配置凭据时用国际站（匿名数据两站基本一致）；
    /// 已配置时**必须**跟随凭据所属站点，否则会话无法匹配。
    ///
    /// 返回 `(endpoint, host)`。`host` 供 `Origin` / `Referer` 使用。
    fn resolve_target(session: Option<&LeetCodeSession>) -> (String, &'static str) {
        match session {
            Some(s) => (s.site.graphql_endpoint(), s.site.host()),
            None => (GRAPHQL_ENDPOINT.to_string(), "leetcode.com"),
        }
    }

    /// 构造请求头。
    ///
    /// `csrf` 为 `Some` 时同时注入 `csrftoken` cookie 与 `x-csrftoken` 头，
    /// 二者值必须一致——这是 LeetCode 的 CSRF 校验要求。
    ///
    /// `host` 决定 `Origin` / `Referer` 的域名。这两者参与服务端的同源校验，
    /// 必须与实际请求的站点一致，否则会被判为跨站请求而拒绝。
    fn build_headers(&self, csrf: Option<&str>, host: &str) -> AppResult<HeaderMap> {
        let mut headers = HeaderMap::new();

        // 使用完整的浏览器 UA。刻意不伪装成极简 UA：LeetCode 对无 UA 或
        // 明显异常 UA 的请求会返回 403。
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
            ),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        // Origin / Referer 必须与请求站点同源。国际站与中国站的域名不同，
        // 写死其中一个会让另一站的请求因同源校验失败而被拒——
        // 这是"Cookie 有效却总报无效"的成因之一。
        let origin = format!("https://{host}");
        headers.insert(
            "Origin",
            HeaderValue::from_str(&origin)
                .map_err(|_| AppError::InvalidInput("无法构造 Origin 头".to_string()))?,
        );
        let referer = format!("{origin}/problemset/");
        headers.insert(
            "Referer",
            HeaderValue::from_str(&referer)
                .map_err(|_| AppError::InvalidInput("无法构造 Referer 头".to_string()))?,
        );

        if let Some(csrf) = csrf {
            // Cookie 头：仅需 csrftoken 即可完成 CSRF 配对校验；
            // LEETCODE_SESSION 由下方单独注入。
            let cookie_value = format!("csrftoken={csrf}");
            headers.insert(
                COOKIE,
                HeaderValue::from_str(&cookie_value).map_err(|_| {
                    AppError::InvalidInput(
                        "csrftoken 含非法字符，无法构造 Cookie 头".to_string(),
                    )
                })?,
            );

            // x-csrftoken 必须与 cookie 中的 csrftoken 完全一致。
            headers.insert(
                "x-csrftoken",
                HeaderValue::from_str(csrf).map_err(|_| {
                    AppError::InvalidInput("csrftoken 含非法字符".to_string())
                })?,
            );
        }

        Ok(headers)
    }

    /// 发送 GraphQL 请求并解析为 `T`。
    ///
    /// `requires_auth` 为 `true` 时，若未配置凭据会**立即返回
    /// `Unauthorized`**，不浪费一次网络往返，也让错误信息更明确。
    ///
    /// `op_name` 用于错误信息定位（GraphQL 的 `operationName`）。
    pub async fn query<T>(
        &self,
        operation_name: &str,
        query: &str,
        variables: serde_json::Value,
        requires_auth: bool,
    ) -> AppResult<T>
    where
        T: serde::de::DeserializeOwned,
    {        let session = self.current_session().await;

        if requires_auth && session.is_none() {
            return Err(AppError::Unauthorized(format!(
                "{operation_name} 需要登录凭据。请在设置页填入 LEETCODE_SESSION 与 csrftoken"
            )));
        }

        // 端点与 Origin/Referer 都必须跟随凭据所属站点。
        let (endpoint, host) = Self::resolve_target(session.as_ref());

        let csrf = session.as_ref().map(|s| s.csrf_token.as_str());
        let mut headers = self.build_headers(csrf, host)?;

        // LEETCODE_SESSION 与 csrftoken 需合并进同一个 Cookie 头。
        // 分开设置会互相覆盖（HeaderMap 的 insert 是替换语义）。
        if let Some(s) = &session {
            let cookie = format!(
                "LEETCODE_SESSION={}; csrftoken={}",
                s.session_cookie, s.csrf_token
            );
            headers.insert(
                COOKIE,
                HeaderValue::from_str(&cookie).map_err(|_| {
                    AppError::InvalidInput(
                        "会话凭据含非法字符，无法构造 Cookie 头（请检查是否误粘贴了多行内容）"
                            .to_string(),
                    )
                })?,
            );
        }

        // GraphQL 请求体遵循规范：query + variables + operationName。
        // 显式提供 operationName 便于服务端与日志定位。
        let body = json!({
            "query": query,
            "variables": variables,
            "operationName": operation_name,
        });

        let resp = self
            .http
            .post(&endpoint)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(AppError::Network)?;

        let status = resp.status();

        // **先读取响应体，再按状态码分类。**
        //
        // 这个顺序至关重要：GraphQL 的错误详情全部在响应体里
        // （`{"errors":[{"message":"Cannot query field ..."}]}`）。
        // 早期版本在每个非 200 分支直接 return，**丢弃了响应体**，
        // 导致中国站的 HTTP 400 只能报出"返回异常状态码 HTTP 400"，
        // 真正的原因 `Cannot query field "matchedUser" on type "Query"`
        // 被完全掩盖，排查时只能从头猜起。
        //
        // 教训：错误分支丢弃响应体是反模式。任何"按状态码提前返回"的代码
        // 都必须先把诊断信息取出来。
        let raw = resp.text().await.map_err(AppError::Network)?;

        match status.as_u16() {
            200 => {}
            401 | 403 => {
                return Err(AppError::Unauthorized(format!(
                    "{operation_name} 被拒绝（HTTP {}）。凭据可能已过期，请在设置页更新{}",
                    status.as_u16(),
                    describe_error_body(&raw)
                )));
            }
            429 => return Err(AppError::RateLimited),
            404 => {
                return Err(AppError::NotFound(format!(
                    "{operation_name} 端点不存在（HTTP 404），接口可能已变更{}",
                    describe_error_body(&raw)
                )))
            }
            code if code >= 500 => {
                // 5xx 归为"可重试"类。reqwest 没有公开的错误构造器，
                // 因此不用 Network 变体（其内层是 reqwest::Error），
                // 改用 Other 并在文案中标明可重试。
                return Err(AppError::Other(format!(
                    "{operation_name} 服务端暂时不可用（HTTP {code}），请稍后重试{}",
                    describe_error_body(&raw)
                )));
            }
            code => {
                // 400 属于"请求不合法"，最常见的成因是**客户端请求了
                // 当前站点不存在的字段**（两站 schema 不同）。
                // 因此把响应体里的 GraphQL 错误原文附上，让成因可见。
                return Err(AppError::Other(format!(
                    "{operation_name} 请求不被接受（HTTP {code}）{}",
                    describe_error_body(&raw)
                )));
            }
        }

        let envelope: GraphQlResponse<T> = serde_json::from_str(&raw)
            .map_err(|e| AppError::parse(format!("GraphQL 响应解析失败: {e}"), Some(&raw)))?;

        // 处理 GraphQL 层错误。注意 data 与 errors 可能并存，
        // 因此仅在 data 缺失时才因 errors 直接失败。
        if let Some(errs) = envelope.errors.as_ref().filter(|e| !e.is_empty()) {
            if envelope.data.is_none() {
                let all_permission = errs.iter().all(GraphQlError::is_permission_denied);
                let detail = errs
                    .iter()
                    .map(GraphQlError::describe)
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(if all_permission {
                    AppError::Unauthorized(format!("{operation_name}: {detail}"))
                } else {
                    AppError::GraphQl(format!("{operation_name}: {detail}"))
                });
            }
            // data 存在：记录性忽略部分字段错误，由调用方通过 Option 处理。
        }

        envelope.data.ok_or_else(|| {
            AppError::parse(
                format!("{operation_name} 响应缺少 data 字段"),
                Some(&raw),
            )
        })
    }

    // -----------------------------------------------------------------------
    // 业务方法
    // -----------------------------------------------------------------------

    /// 验证凭据是否有效。
    ///
    /// 返回 `Ok(Some(identity))` 表示已登录；`Ok(None)` 表示凭据未配置
    /// 或无效（匿名可访问但未登录）。
    ///
    /// 这里**不**使用 `requires_auth = true`：该查询匿名也可调用，
    /// 通过返回值区分登录态才是正确用法。
    ///
    /// ## 站点分派
    ///
    /// 中国站的 `userStatus` 额外提供 `userSlug`，这是查询该站用户数据时
    /// **应该**使用的标识（详见 `queries::CN_USER_STATUS`）。
    /// 国际站无此字段，走原查询。
    pub async fn check_session(&self) -> AppResult<Option<LoginIdentity>> {
        let is_cn = self.current_site().await == LeetCodeSite::Cn;

        let (op_name, query_text) = if is_cn {
            ("userStatus", queries::CN_USER_STATUS)
        } else {
            ("userStatus", queries::USER_STATUS)
        };

        let data: UserStatusData = self
            .query(op_name, query_text, json!({}), false)
            .await?;

        let status = data
            .user_status
            .ok_or_else(|| AppError::parse("userStatus 缺失", None))?;

        Ok(match (status.is_signed_in, status.username) {
            (Some(true), Some(name)) => Some(LoginIdentity {
                // `userSlug` 缺失时回退到 `username`：至少比没有标识好，
                // 虽然中文昵称会查不到，但国际站与部分中国站账号
                // 的两者恰好相同（slug 就是用户名）。
                slug: status
                    .user_slug
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| name.clone()),
                username: name,
                real_name: status.real_name.filter(|s| !s.trim().is_empty()),
            }),
            _ => None,
        })
    }

    /// 拉取全站题目统计（按难度）。匿名可用。**仅国际站支持**。
    ///
    /// 中国站没有 `allQuestionsCount` 根字段，因此返回
    /// `UnsupportedOnSite` 而非空数据——UI 需改用题库列表返回的 `total`
    /// 作为总量，并据此隐藏"全站题量"这块展示。
    pub async fn fetch_global_counts(&self) -> AppResult<crate::models::DifficultyCounts> {
        if !self.current_site().await.supports_global_counts() {
            return Err(self.unsupported("全站题量统计").await);
        }

        let data: GlobalCountsData = self
            .query(
                "globalQuestionCounts",
                queries::GLOBAL_QUESTION_COUNTS,
                json!({}),
                false,
            )
            .await?;
        Ok(data.to_counts())
    }

    /// 拉取单页题库。
    ///
    /// `filter_status` 控制是否在**服务端**按完成状态过滤。
    ///
    /// 重要：该参数仅在已配置有效凭据时才有意义。实测表明未认证时传入
    /// `status` 过滤会让服务端返回空结果集（`total: 0`）。因此本方法
    /// 在匿名模式下**忽略**该参数，改为返回全量，由调用方在本地过滤。
    ///
    /// 返回 `(总题数, 本页题目)`。
    ///
    /// ## 两站分派
    ///
    /// 国际站与中国站的题库查询字段名不同（见 `queries` 模块的对比表），
    /// 因此按当前站点选择查询文本。响应 DTO 用 `serde alias` 同时兼容
    /// 两种命名，解析路径无需分派。
    pub async fn fetch_problem_page(
        &self,
        skip: usize,
        limit: usize,
        difficulty: Option<crate::models::Difficulty>,
    ) -> AppResult<(i64, Vec<Problem>)> {
        let mut filters = serde_json::Map::new();

        if let Some(d) = difficulty {
            // 输入过滤器使用**大写**枚举值。两站的输入枚举都是大写
            // （`EASY`/`MEDIUM`/`HARD`），因此这里无需按站点区分。
            filters.insert(
                "difficulty".to_string(),
                json!(d.as_api_str()),
            );
        }

        let variables = json!({
            "categorySlug": "",
            "skip": skip,
            "limit": limit,
            "filters": serde_json::Value::Object(filters),
        });

        let site = self.current_site().await;
        let query_text = match site {
            LeetCodeSite::Com => queries::PROBLEM_LIST,
            LeetCodeSite::Cn => queries::CN_PROBLEM_LIST,
        };

        let data: ProblemListData = self
            .query("problemsetQuestionList", query_text, variables, false)
            .await?;

        let page = data
            .problemset_question_list
            .ok_or_else(|| AppError::parse("problemsetQuestionList 缺失", None))?;

        // 中国站带中文标题，优先展示以便中文用户阅读。
        let prefer_cn = site.prefers_chinese_titles();
        let problems = page
            .questions
            .into_iter()
            .filter_map(|n| n.into_problem_with_locale(prefer_cn))
            .collect();

        Ok((page.total, problems))
    }

    /// 全量拉取题库（自动分页）。
    ///
    /// `on_progress` 在每个分页返回后被调用，参数是 `(已拉取数, 总数)`，
    /// 供 UI 展示进度。
    ///
    /// 三层保护防止异常情况下的失控：
    /// 1. `MAX_PAGES` 限制轮次
    /// 2. `MAX_TOTAL_GUARD` 限制总量
    /// 3. 单页返回空则提前终止
    pub async fn fetch_all_problems<F>(&self, mut on_progress: F) -> AppResult<Vec<Problem>>
    where
        F: FnMut(usize, i64) + Send,
    {
        let mut all: Vec<Problem> = Vec::new();
        let mut skip = 0usize;
        let mut total: i64 = 0;

        for page_idx in 0..MAX_PAGES {
            let (reported_total, problems) =
                self.fetch_problem_page(skip, PAGE_SIZE, None).await?;

            if page_idx == 0 {
                total = reported_total;
                if total > MAX_TOTAL_GUARD {
                    // 远端总数异常，降级为受保护的上限，避免无限拉取。
                    total = MAX_TOTAL_GUARD;
                }
            }

            let got = problems.len();
            all.extend(problems);
            on_progress(all.len(), total);

            // 终止条件：本页为空（已到末尾），或已拉满。
            if got == 0 {
                break;
            }
            if all.len() as i64 >= total {
                break;
            }
            skip += got;
        }

        Ok(all)
    }

    /// 拉取用户画像。匿名可用，但 `status` 类字段需要认证。
    ///
    /// ## 两站分派
    ///
    /// 两站的画像接口结构差异最大，需要**分别实现**：
    ///
    /// - 国际站：一个 `getUserProfile` 查询同时返回画像与解题统计
    ///   （`submitStatsGlobal`），并可直接用 `username` 参数。
    /// - 中国站：没有 `matchedUser`，且画像与解题进度是**两个独立查询**
    ///   （`userProfilePublicProfile` + `userProfileUserQuestionProgressV2`），
    ///   参数名是 `userSlug`。这里串行发两次请求后合并。
    ///
    /// 中国站的进度查询失败时降级：画像本身仍然有效（用户名、排名、声望），
    /// 只是解题数为 0。这比整体失败对用户更友好。
    ///
    /// ## 中国站的 slug 回退（重要）
    ///
    /// `username` 参数在中国站语义上是 **slug**。若调用方传入的是中文显示
    /// 昵称（如「梧糊」），远端会返回 `null`。此时若已登录，会自动改用
    /// 登录态中的真实 slug 重试一次——因为用户往往只知道自己的
    /// 显示昵称，而不知道 slug。
    pub async fn fetch_profile(
        &self,
        username: &str,
    ) -> AppResult<crate::models::UserProfile> {
        let authenticated = self.current_session().await.is_some();

        match self.current_site().await {
            LeetCodeSite::Com => self.fetch_profile_com(username, authenticated).await,
            LeetCodeSite::Cn => self.fetch_profile_cn(username, authenticated).await,
        }
    }

    /// 国际站画像：单查询路径。
    async fn fetch_profile_com(
        &self,
        username: &str,
        authenticated: bool,
    ) -> AppResult<crate::models::UserProfile> {
        let data: UserProfileData = self
            .query(
                "getUserProfile",
                queries::USER_PROFILE,
                json!({ "username": username }),
                false,
            )
            .await?;

        data.into_profile(username, authenticated).ok_or_else(|| {
            AppError::NotFound(format!("未找到用户 「{username}」，请检查用户名拼写"))
        })
    }

    /// 中国站画像：画像 + 解题进度两个查询合并。
    ///
    /// ## 标识解析（本方法的核心职责）
    ///
    /// 中国站的查询标识是 **ASCII slug**，而用户在设置页填的、界面展示的
    /// 都是**显示昵称**。用户改过昵称后两者不一致，此时拿昵称去查会得到
    /// `null`——报错却是"用户不存在"，严重误导。
    ///
    /// 因此这里做两级解析，并把**最终生效的标识**回带给调用方：
    ///
    /// 1. 先按传入名称直接查（它就是 slug 时一次命中，无额外开销）；
    /// 2. 未命中且已登录 → 从 `userStatus` 取真实 slug 重试。
    ///
    /// 回带标识很重要：调用方若不知道实际生效的是哪个标识，
    /// 后续的标签/竞赛/提交查询会继续用错误的昵称，导致"画像对了、
    /// 其它全空"这种更难排查的半失效状态。
    async fn fetch_profile_cn(
        &self,
        username: &str,
        authenticated: bool,
    ) -> AppResult<crate::models::UserProfile> {
        // 首次尝试：用调用方传入的名称（可能是昵称，也可能是 slug）。
        if let Some(profile) = self.try_fetch_profile_cn(username, authenticated).await? {
            return Ok(profile);
        }

        // 未找到。若已登录，用登录态中的 slug 再试一次。
        //
        // 这一步很关键：中国站用户改过中文昵称后，昵称与 slug 不一致，
        // 而用户通常只知道昵称。已登录时我们能从 userStatus 拿到 slug，
        // 从而自动完成映射，无需用户手工查找。
        if authenticated {
            if let Ok(Some(identity)) = self.check_session().await {
                let slug = identity.slug;
                // 避免重复请求同一个名称。
                if slug != username {
                    if let Some(profile) =
                        self.try_fetch_profile_cn(&slug, authenticated).await?
                    {
                        return Ok(profile);
                    }
                }
            }
        }

        Err(AppError::NotFound(format!(
            "未找到用户 「{username}」。\
             力扣中国站按账号标识（slug，通常是注册时的英文/拼音名）查询，\
             而非显示昵称——若你设置的是中文昵称，请改填账号标识，\
             或在设置页填写会话凭据后重试（应用可自动从登录态识别）。"
        )))
    }

    /// 解析出**实际可用于查询**的账号标识。
    ///
    /// 调用方在拉取画像后应据此校正本地保存的用户名，否则后续查询
    /// （标签、竞赛、提交记录）会继续使用用户填写的显示昵称而失败。
    ///
    /// 返回 `None` 表示无法解析（未登录、或未确认）。
    pub async fn resolve_query_identifier(
        &self,
        configured: &str,
    ) -> Option<String> {
        if self.current_site().await != LeetCodeSite::Cn {
            // 国际站 `username` 本身就是查询标识，无需解析。
            return None;
        }
        let identity = self.check_session().await.ok().flatten()?;
        let slug = identity.slug.trim();
        if slug.is_empty() || slug == configured {
            None
        } else {
            Some(slug.to_string())
        }
    }

    /// 尝试用给定标识查询中国站画像。返回 `Ok(None)` 表示该标识不存在。
    async fn try_fetch_profile_cn(
        &self,
        slug: &str,
        authenticated: bool,
    ) -> AppResult<Option<crate::models::UserProfile>> {
        let profile: CnUserProfileData = self
            .query(
                "userProfilePublicProfile",
                queries::CN_USER_PROFILE,
                json!({ "userSlug": slug }),
                false,
            )
            .await?;

        // 进度是附加信息：失败时降级为 None 而非中断整个画像流程。
        let progress: Option<CnUserProgressNode> = match self
            .query::<CnUserProgressData>(
                "userProfileUserQuestionProgressV2",
                queries::CN_USER_PROGRESS,
                json!({ "userSlug": slug }),
                false,
            )
            .await
        {
            Ok(d) => d.progress,
            Err(_) => None,
        };

        Ok(profile.into_profile(progress, slug, authenticated))
    }

    /// 拉取用户技能标签统计。匿名可用。这是推荐算法的核心输入。
    ///
    /// **仅国际站支持**：中国站没有 `matchedUser.tagProblemCounts`。
    /// 缺失时推荐算法会降级（仅基于题库元数据），UI 应据此调整文案。
    pub async fn fetch_tag_stats(
        &self,
        username: &str,
    ) -> AppResult<Vec<crate::models::TagProgress>> {
        if !self.current_site().await.supports_tag_stats() {
            return Err(self.unsupported("技能标签统计").await);
        }

        let data: TagStatsData = self
            .query(
                "skillStats",
                queries::USER_TAG_STATS,
                json!({ "username": username }),
                false,
            )
            .await?;

        let mut stats = data
            .matched_user
            .and_then(|u| u.tag_problem_counts)
            .map(TagCountGroups::into_progress)
            .unwrap_or_default();

        // 防御性去重：远端在不同分组（fundamental / intermediate / advanced）
        // 中可能返回同名标签，重复项会让"掌握度"统计被重复计数。
        crate::logic::recommend::dedupe_tags(&mut stats);

        Ok(stats)
    }

    /// 拉取提交日历。**需要认证**。
    ///
    /// 失败时返回空日历而非错误：日历是附加信息，其缺失不应阻断主流程。
    /// 调用方会把它填充进已有的 `UserProfile`。
    /// 拉取提交日历。需要认证。
    ///
    /// 返回 `(日期→提交次数, 连续活跃天数, 累计活跃天数)`。
    ///
    /// 三者在同一个 GraphQL 查询中一并返回，因此放在一个方法里；
    /// 日历不可用时静默降级为空值，不向上传播错误（未认证是预期情况）。
    pub async fn fetch_calendar(
        &self,
        username: &str,
    ) -> AppResult<(Vec<(String, u32)>, Option<u32>, Option<u32>)> {
        if !self.current_site().await.supports_calendar() {
            return Err(self.unsupported("提交日历").await);
        }

        let result: AppResult<CalendarData> = self
            .query(
                "userProfileCalendar",
                queries::USER_CALENDAR,
                json!({ "username": username, "year": serde_json::Value::Null }),
                true,
            )
            .await;

        match result {
            Ok(data) => match data.matched_user.and_then(|u| u.user_calendar) {
                Some(cal) => {
                    let days = cal.parse_calendar();
                    // DTO 用 i64（远端可能给出任意整数），领域模型用 u32。
                    // 负值或溢出时保守地丢弃，避免把异常值带进 UI。
                    let streak = cal.streak.and_then(|v| u32::try_from(v).ok());
                    let total = cal.total_active_days.and_then(|v| u32::try_from(v).ok());
                    Ok((days, streak, total))
                }
                None => Ok((Vec::new(), None, None)),
            },
            // 日历不可用时静默降级，不向上传播错误。
            Err(_) => Ok((Vec::new(), None, None)),
        }
    }

    /// 拉取最近提交记录。需要认证。
    ///
    /// 未认证时返回空列表而非错误（实测服务端会返回 `submissions: null`
    /// 且不带 errors，属"无数据"语义）。
    pub async fn fetch_recent_submissions(
        &self,
        limit: usize,
    ) -> AppResult<Vec<RecentSubmission>> {
        // 中国站的提交节点类型不含 `titleSlug`，无法映射到题目，
        // 因此视为不支持而非返回缺字段的残缺数据。
        if !self.current_site().await.supports_recent_submissions() {
            return Err(self.unsupported("最近提交记录").await);
        }

        if self.current_session().await.is_none() {
            return Ok(Vec::new());
        }

        let variables = json!({
            "offset": 0,
            "limit": limit.min(50),
            "lastKey": serde_json::Value::Null,
            "questionSlug": serde_json::Value::Null,
        });

        let data: SubmissionListData = self
            .query(
                "submissionList",
                queries::SUBMISSION_LIST,
                variables,
                true,
            )
            .await?;

        Ok(data
            .submission_list
            .and_then(|p| p.submissions)
            .map(|list| list.into_iter().filter_map(SubmissionNode::into_recent).collect())
            .unwrap_or_default())
    }

    /// 拉取竞赛排名摘要。匿名可用（无参赛记录时返回全零摘要）。
    /// **仅国际站支持**。
    pub async fn fetch_contest_summary(
        &self,
        username: &str,
    ) -> AppResult<crate::models::ContestSummary> {
        if !self.current_site().await.supports_contest() {
            return Err(self.unsupported("竞赛复盘").await);
        }

        let data: ContestRankingData = self
            .query(
                "userContestRankingInfo",
                queries::USER_CONTEST_RANKING,
                json!({ "username": username }),
                false,
            )
            .await?;
        Ok(data.into_summary())
    }

    /// 拉取竞赛历史。匿名可用。**仅国际站支持**。
    pub async fn fetch_contest_history(
        &self,
        username: &str,
    ) -> AppResult<Vec<crate::models::ContestRecord>> {
        if !self.current_site().await.supports_contest() {
            return Err(self.unsupported("竞赛复盘").await);
        }

        let data: ContestHistoryData = self
            .query(
                "userContestRankingHistory",
                queries::USER_CONTEST_HISTORY,
                json!({ "username": username }),
                false,
            )
            .await?;
        Ok(data.into_records())
    }
}

/// 从错误响应体中提取可读的诊断信息，用于拼接进错误消息。
///
/// ## 为什么需要它
///
/// LeetCode 在非 200 时仍会返回结构化 JSON，形如：
///
/// ```json
/// {"errors":[{"message":"Cannot query field \"matchedUser\" on type \"Query\"."}]}
/// ```
///
/// 这条 message 是排查**唯一**的线索。早期实现丢弃了响应体，只报状态码，
/// 使一个"字段名不匹配"的简单问题变成了无从下手的谜题。
///
/// 返回格式：`（原因：Cannot query field "matchedUser" ...）`，
/// 无法提取时返回空串，不干扰主消息。
///
/// 响应体会被截断，避免把整页 HTML 塞进错误文案。
fn describe_error_body(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // 优先按 GraphQL 错误结构解析，取出全部 message。
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(errs) = v.get("errors").and_then(|e| e.as_array()) {
            let msgs: Vec<String> = errs
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .collect();
            if !msgs.is_empty() {
                return format!("（原因：{}）", truncate_chars(&msgs.join("; "), 200));
            }
        }
        // 非 GraphQL 结构但也可能带 message 字段。
        if let Some(m) = v.get("message").and_then(|m| m.as_str()) {
            if !m.trim().is_empty() {
                return format!("（原因：{}）", truncate_chars(m.trim(), 200));
            }
        }
    }

    // 非 JSON（例如 HTML 错误页）：给出开头片段即可，不必深挖。
    format!("（响应片段：{}）", truncate_chars(trimmed, 160))
}

/// 按**字符**截断（而非字节），避免在多字节 UTF-8 边界上切断字符串
/// 产生乱码或 panic。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Arc<RwLock<AppConfig>> {
        Arc::new(RwLock::new(AppConfig::default()))
    }

    #[test]
    fn client_builds_successfully() {
        let c = LeetCodeClient::new(test_config());
        assert!(c.is_ok());
    }

    #[test]
    fn headers_without_csrf_have_no_cookie() {
        let c = LeetCodeClient::new(test_config()).unwrap();
        let h = c.build_headers(None, "leetcode.com").unwrap();
        assert!(!h.contains_key(COOKIE), "匿名请求不应携带 Cookie");
        assert!(!h.contains_key("x-csrftoken"), "匿名请求不应携带 csrf 头");
        assert!(h.contains_key(USER_AGENT));
        assert_eq!(h.get(CONTENT_TYPE).unwrap(), "application/json");
    }

    #[test]
    fn headers_with_csrf_pair_cookie_and_header() {
        let c = LeetCodeClient::new(test_config()).unwrap();
        let h = c.build_headers(Some("tok123"), "leetcode.com").unwrap();
        assert_eq!(h.get("x-csrftoken").unwrap(), "tok123");
        assert_eq!(h.get(COOKIE).unwrap(), "csrftoken=tok123");
    }

    #[test]
    fn headers_origin_and_referer_follow_the_site() {
        // 缺陷回归：Origin/Referer 曾写死为 leetcode.com，
        // 使 leetcode.cn 的凭据因同源校验失败而被判"Cookie 无效"。
        let c = LeetCodeClient::new(test_config()).unwrap();

        let com = c.build_headers(None, "leetcode.com").unwrap();
        assert_eq!(com.get("Origin").unwrap(), "https://leetcode.com");
        assert_eq!(
            com.get("Referer").unwrap(),
            "https://leetcode.com/problemset/"
        );

        let cn = c.build_headers(None, "leetcode.cn").unwrap();
        assert_eq!(cn.get("Origin").unwrap(), "https://leetcode.cn");
        assert_eq!(
            cn.get("Referer").unwrap(),
            "https://leetcode.cn/problemset/"
        );
    }

    #[tokio::test]
    async fn endpoint_follows_configured_site() {
        // 匿名时应指向国际站。
        let (ep, host) = LeetCodeClient::resolve_target(None);
        assert_eq!(ep, "https://leetcode.com/graphql");
        assert_eq!(host, "leetcode.com");

        // 中国站凭据必须切到 leetcode.cn，否则会话无法匹配——
        // 这是"Cookie 有效却总报无效"的核心成因。
        let cn = LeetCodeSession {
            session_cookie: "session-value".into(),
            csrf_token: "abcdef1234567890abcdef1234567890".into(),
            site: crate::config::LeetCodeSite::Cn,
        };
        let (ep, host) = LeetCodeClient::resolve_target(Some(&cn));
        assert_eq!(ep, "https://leetcode.cn/graphql");
        assert_eq!(host, "leetcode.cn");

        // 国际站凭据指向国际站。
        let com = LeetCodeSession {
            site: crate::config::LeetCodeSite::Com,
            ..cn
        };
        let (ep, host) = LeetCodeClient::resolve_target(Some(&com));
        assert_eq!(ep, "https://leetcode.com/graphql");
        assert_eq!(host, "leetcode.com");
    }

    #[tokio::test]
    async fn current_session_returns_none_when_incomplete() {
        let cfg = test_config();
        let c = LeetCodeClient::new(cfg.clone()).unwrap();
        assert!(c.current_session().await.is_none());

        // 只填一半不应视为已认证。
        {
            let mut g = cfg.write().await;
            g.session.session_cookie = "abc".into();
        }
        assert!(c.current_session().await.is_none(), "缺 csrftoken 不算已认证");

        {
            let mut g = cfg.write().await;
            g.session.csrf_token = "def".into();
        }
        let s = c.current_session().await.expect("两项齐全后应返回凭据");
        assert_eq!(s.session_cookie, "abc");
    }

    #[tokio::test]
    async fn requires_auth_fails_fast_without_credentials() {
        let c = LeetCodeClient::new(test_config()).unwrap();
        let r: AppResult<CalendarData> = c
            .query(
                "userProfileCalendar",
                queries::USER_CALENDAR,
                json!({ "username": "x", "year": serde_json::Value::Null }),
                true,
            )
            .await;
        // 应在发起网络请求前就失败，且错误被识别为鉴权类。
        match r {
            Err(e) => assert!(e.is_auth_related(), "应为鉴权错误，实得: {e}"),
            Ok(_) => panic!("未配置凭据时不应成功"),
        }
    }

    #[tokio::test]
    async fn fetch_recent_submissions_short_circuits_when_anonymous() {
        let c = LeetCodeClient::new(test_config()).unwrap();
        // 匿名时应直接返回空，不发起网络请求。
        let subs = c.fetch_recent_submissions(10).await.unwrap();
        assert!(subs.is_empty());
    }

    // -----------------------------------------------------------------------
    // 站点分派与能力边界
    // -----------------------------------------------------------------------

    /// 已配置凭据时，`current_site` 必须跟随凭据站点。
    ///
    /// 这是所有查询分派的依据：取错站点会让请求发往错误域名并携带
    /// 错误字段名，直接得到 HTTP 400。
    #[tokio::test]
    async fn current_site_follows_configured_session() {
        let cfg = test_config();
        let c = LeetCodeClient::new(cfg.clone()).unwrap();

        // 匿名时默认国际站。
        assert_eq!(c.current_site().await, LeetCodeSite::Com);

        {
            let mut g = cfg.write().await;
            g.session = LeetCodeSession {
                session_cookie: "s".into(),
                csrf_token: "c".into(),
                site: LeetCodeSite::Cn,
            };
        }
        assert_eq!(
            c.current_site().await,
            LeetCodeSite::Cn,
            "已配置中国站凭据时必须切到中国站"
        );
    }

    /// 中国站不支持的功能必须返回 `UnsupportedOnSite`，而不是发起请求。
    ///
    /// 关键在于错误**可被识别**：UI 需要把它渲染为信息提示而非故障告警。
    /// 若这些方法发起了真实请求，中国站会返回 HTTP 400
    /// `Cannot query field ...`，用户会误以为程序坏了。
    #[tokio::test]
    async fn cn_site_reports_unsupported_features_without_network_call() {
        let cfg = test_config();
        let c = LeetCodeClient::new(cfg.clone()).unwrap();
        {
            let mut g = cfg.write().await;
            g.session = LeetCodeSession {
                session_cookie: "s".into(),
                csrf_token: "c".into(),
                site: LeetCodeSite::Cn,
            };
        }

        // 逐个验证：这些调用必须立即返回，不经过网络。
        let cases: Vec<(&str, AppResult<()>)> = vec![
            (
                "fetch_tag_stats",
                c.fetch_tag_stats("梧糊").await.map(|_| ()),
            ),
            (
                "fetch_calendar",
                c.fetch_calendar("梧糊").await.map(|_| ()),
            ),
            (
                "fetch_global_counts",
                c.fetch_global_counts().await.map(|_| ()),
            ),
            (
                "fetch_contest_summary",
                c.fetch_contest_summary("梧糊").await.map(|_| ()),
            ),
            (
                "fetch_contest_history",
                c.fetch_contest_history("梧糊").await.map(|_| ()),
            ),
            (
                "fetch_recent_submissions",
                c.fetch_recent_submissions(10).await.map(|_| ()),
            ),
        ];

        for (name, result) in cases {
            match result {
                Err(e) => assert!(
                    e.is_unsupported_on_site(),
                    "{name} 应返回 UnsupportedOnSite，实得: {e}"
                ),
                Ok(_) => panic!("{name} 在中国站不应成功"),
            }
        }
    }

    /// 错误消息必须点明是哪个站点、哪个功能，便于用户理解。
    #[tokio::test]
    async fn cn_unsupported_error_names_feature_and_site() {
        let cfg = test_config();
        let c = LeetCodeClient::new(cfg.clone()).unwrap();
        {
            let mut g = cfg.write().await;
            g.session = LeetCodeSession {
                session_cookie: "s".into(),
                csrf_token: "c".into(),
                site: LeetCodeSite::Cn,
            };
        }

        let msg = c
            .fetch_contest_summary("梧糊")
            .await
            .expect_err("应为不支持")
            .to_string();
        assert!(msg.contains("竞赛"), "错误消息应说明功能名: {msg}");
        assert!(msg.contains("中国站"), "错误消息应说明站点: {msg}");
    }

    /// 国际站不应被这些能力门禁拦住（门禁不能误伤）。
    #[test]
    fn com_site_declares_full_capability() {
        let com = LeetCodeSite::Com;
        assert!(com.supports_tag_stats());
        assert!(com.supports_calendar());
        assert!(com.supports_contest());
        assert!(com.supports_recent_submissions());
        assert!(com.supports_global_counts());
        assert!(!com.prefers_chinese_titles());
    }

    /// 中国站的能力声明必须反映实测结果。
    #[test]
    fn cn_site_declares_limited_capability() {
        let cn = LeetCodeSite::Cn;
        assert!(!cn.supports_tag_stats(), "中国站无 tagProblemCounts");
        assert!(!cn.supports_calendar(), "中国站无 userCalendar");
        assert!(!cn.supports_contest(), "中国站竞赛接口参数不同，未适配");
        assert!(!cn.supports_recent_submissions(), "中国站提交节点无 titleSlug");
        assert!(!cn.supports_global_counts(), "中国站无 allQuestionsCount");
        assert!(cn.prefers_chinese_titles(), "中国站提供 titleCn");
    }

    // -----------------------------------------------------------------------
    // 错误响应体提取
    // -----------------------------------------------------------------------

    /// **本组测试对应一个真实缺陷。**
    ///
    /// 早期实现在非 200 分支直接 return、丢弃响应体，导致中国站的
    /// HTTP 400 只能报出状态码，真正原因
    /// `Cannot query field "matchedUser" on type "Query"` 被掩盖。
    #[test]
    fn error_body_extracts_graphql_error_messages() {
        let raw = r#"{"errors":[{"message":"Cannot query field \"matchedUser\" on type \"Query\"."}]}"#;
        let s = describe_error_body(raw);
        assert!(
            s.contains("Cannot query field"),
            "必须提取出 GraphQL 错误原文，实得: {s}"
        );
        assert!(s.contains("matchedUser"), "必须保留出错的字段名");
    }

    /// 多条错误应全部保留，而不是只取第一条。
    #[test]
    fn error_body_joins_multiple_graphql_errors() {
        let raw = r#"{"errors":[{"message":"Unknown argument \"username\""},
                                 {"message":"Field \"userSlug\" is required"}]}"#;
        let s = describe_error_body(raw);
        assert!(s.contains("Unknown argument"), "实得: {s}");
        assert!(s.contains("userSlug"), "实得: {s}");
    }

    /// 空响应体不应产生噪音。
    #[test]
    fn error_body_handles_empty_input() {
        assert_eq!(describe_error_body(""), "");
        assert_eq!(describe_error_body("   \n  "), "");
    }

    /// 非 JSON 响应（例如 HTML 错误页）也应给出可读片段。
    #[test]
    fn error_body_handles_non_json_response() {
        let s = describe_error_body("<html><body>502 Bad Gateway</body></html>");
        assert!(s.contains("502"), "实得: {s}");
    }

    /// 超长响应体必须被截断，且**不能在多字节字符边界切断**。
    ///
    /// 用中文构造样本：若按字节截断会 panic 或产生乱码。
    #[test]
    fn error_body_truncation_respects_utf8_boundaries() {
        let msg = "错".repeat(500);
        let raw = format!(r#"{{"errors":[{{"message":"{msg}"}}]}}"#);
        let s = describe_error_body(&raw);
        // 不应 panic，且长度受控。
        assert!(s.chars().count() < 260, "应被截断，实得 {} 字符", s.chars().count());
    }

    /// `truncate_chars` 按字符而非字节计数。
    #[test]
    fn truncate_chars_counts_characters_not_bytes() {
        // 3 个汉字 = 9 字节，但只有 3 个字符。
        let s = truncate_chars("中文测试", 3);
        assert_eq!(s, "中文测…");
        // 未超限时原样返回，不加省略号。
        assert_eq!(truncate_chars("abc", 5), "abc");
    }

    // -----------------------------------------------------------------------
    // 登录身份：标识（slug）≠ 显示昵称
    // -----------------------------------------------------------------------

    /// **本组测试对应一个真实缺陷。**
    ///
    /// 用户在中国站的显示昵称是「梧糊」，但账号查询标识是 ASCII slug。
    /// 拿昵称去查一律返回 `null`，报错文案却是"未找到用户"——误导方向。
    /// 修复的核心是**把标识与昵称分开保存**，因此这里锁定两者都能被
    /// 正确表达。
    #[test]
    fn login_identity_keeps_slug_separate_from_display_name() {
        let id = LoginIdentity {
            slug: "wuhu".into(),
            username: "梧糊".into(),
            real_name: None,
        };
        // 查询必须用 slug。
        assert_eq!(id.slug, "wuhu");
        // 展示必须用昵称，且**同时**提示标识，用户才看得懂为何填昵称无效。
        let label = id.display_label();
        assert!(label.contains("梧糊"), "展示应含昵称: {label}");
        assert!(label.contains("wuhu"), "展示应含标识: {label}");
    }

    /// 昵称与标识相同时（国际站、以及未改昵称的中国站账号）
    /// 不应产生冗余的括号说明。
    #[test]
    fn login_identity_label_omits_redundant_slug() {
        let id = LoginIdentity {
            slug: "LeetCode".into(),
            username: "LeetCode".into(),
            real_name: None,
        };
        assert_eq!(id.display_label(), "LeetCode");
    }

    /// 国际站不做标识解析——`username` 本身就是查询标识。
    ///
    /// 这条断言防止有人把中国站的逻辑无差别套到国际站上：那会在每次
    /// 刷新账号时都多打一次 `userStatus` 请求。
    #[tokio::test]
    async fn resolve_query_identifier_is_noop_on_com_site() {
        let c = LeetCodeClient::new(test_config()).unwrap();
        // 默认即国际站。
        assert_eq!(
            c.resolve_query_identifier("anything").await,
            None,
            "国际站不应触发标识解析"
        );
    }

    /// 中国站且未登录时无法解析，应返回 `None` 而不是报错。
    ///
    /// UI 依赖"解析不出就不改用户名"这一行为；若此处 panic 或返回
    /// 错误，刷新账号会在匿名场景下崩掉。
    #[tokio::test]
    async fn resolve_query_identifier_returns_none_when_anonymous_on_cn() {
        let cfg = test_config();
        let c = LeetCodeClient::new(cfg.clone()).unwrap();
        {
            let mut g = cfg.write().await;
            g.session = LeetCodeSession {
                session_cookie: String::new(), // 匿名
                csrf_token: String::new(),
                site: LeetCodeSite::Cn,
            };
        }
        assert_eq!(
            c.resolve_query_identifier("梧糊").await,
            None,
            "匿名中国站无法解析标识，应安静返回 None"
        );
    }
}
