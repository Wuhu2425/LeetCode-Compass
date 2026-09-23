//! 应用状态与消息泵。
//!
//! ## 线程模型
//!
//! egui 的主循环运行在 UI 线程上，且每帧都会重新执行 `update()`。
//! 网络请求绝不能在这个循环里同步执行——那会让界面完全冻结。
//!
//! 本应用的方案：
//! 1. UI 线程只负责渲染与事件收集；
//! 2. 所有网络/I/O 任务通过 `tokio::spawn` 派发到 runtime；
//! 3. 后台任务完成后，把结果通过 `mpsc` channel 投回；
//! 4. UI 线程在每帧开头做一次非阻塞的 `try_recv`，把积压的消息消费掉。
//!
//! 关键细节：后台任务完成后必须调用 `ctx.request_repaint()`，否则
//! egui 在无用户输入时不会重绘，异步结果要等到下一次鼠标移动才显示。
//!
//! ## 状态分层
//!
//! - [`CompassApp`]：顶层容器，持有所有页面共享的状态
//! - 各页面状态（`HomeState` 等）：只被对应页面读写

use std::sync::Arc;

use egui::Context;
use tokio::sync::{mpsc, RwLock};

use crate::config::AppConfig;
use crate::error::{AppError, AppResult};
use crate::leetcode::client::LoginIdentity;
use crate::leetcode::types::RecentSubmission;
use crate::leetcode::LeetCodeClient;
use crate::llm::LlmClient;
use crate::logic::assistant::AssistantContext;
use crate::logic::contest::ContestAnalysis;
use crate::logic::recommend::{RecommendConfig, RecommendMode, RecommendationSet};
use crate::models::{ChatMessage, ContestRecord, Problem, TagProgress, UserProfile};
use crate::storage::{self, Database};

/// 后台任务完成后回传给 UI 的消息。
///
/// 每个变体对应一类异步操作的完成事件。UI 层在 `drain_messages` 中
/// 集中处理，因此新增异步操作只需添加一个变体。
#[derive(Debug)]
pub enum AppMessage {
    /// 题库拉取进度：(已拉取, 总数)
    ProblemSyncProgress(usize, i64),
    /// 题库拉取完成
    ///
    /// `Err` 携带 `(消息, 是否可重试)`。把 `is_retryable` 的判断在网络
    /// 层完成并随消息传回，UI 才有依据决定是否给出"重试"按钮——
    /// 对一个用户名拼错的 404 显示"重试"只会误导用户。
    ProblemsLoaded(Result<Vec<Problem>, (String, bool)>),
    /// 配置保存完成
    ConfigSaved(Result<(), String>),
    /// 账号数据刷新完成（画像 + 标签 + 竞赛 + 提交）
    AccountRefreshed(Box<AccountSnapshot>),
    /// LLM 连通性测试
    LlmTestResult(Result<String, String>),
    /// 助理回复
    AssistantReply(Result<String, String>),
    /// 会话凭据校验
    ///
    /// 携带完整的 [`LoginIdentity`] 而非仅用户名：中国站的**查询标识
    /// （slug）与显示昵称不同**，UI 需要两者才能既正确展示又能用
    /// slug 去查询账号数据。
    ///
    /// 第二项标明触发方式，决定结果是否需要打扰用户。
    SessionChecked(Result<Option<LoginIdentity>, String>, CheckTrigger),
    /// 通用操作提示（成功/失败）
    Toast(ToastKind, String),
}

/// 会话校验的触发方式，决定结果要不要打扰用户。
///
/// 区分两者是必要的：启动时的后台复验对用户是"隐形"的——成功了不该弹
/// 提示（用户什么都没做，弹窗只会造成困惑），但**失败了必须说**，
/// 否则用户会一直看着"凭据已校验"却取不到数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckTrigger {
    /// 用户主动点击「校验凭据」：成功与失败都要明确反馈。
    Manual,
    /// 启动时的后台复验：成功保持安静，失败才提示。
    Silent,
}

/// 账号数据快照。一次性回传，避免多次通道往返。
#[derive(Debug)]
pub struct AccountSnapshot {
    pub profile: Option<UserProfile>,
    pub tag_stats: Vec<TagProgress>,
    pub contest_records: Vec<ContestRecord>,
    pub contest_analysis: Option<ContestAnalysis>,
    pub recent_submissions: Vec<RecentSubmission>,
    /// 刷新过程中的非致命错误（例如日历不可用），用于提示而非中断。
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
    Info,
}

/// 顶部导航页面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Home,
    Recommend,
    Contest,
    Assistant,
    Settings,
}

impl Page {
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Home => "题库",
            Self::Recommend => "推荐",
            Self::Contest => "竞赛复盘",
            Self::Assistant => "AI 助理",
            Self::Settings => "设置",
        }
    }

    pub fn all() -> [Page; 5] {
        [
            Self::Home,
            Self::Recommend,
            Self::Contest,
            Self::Assistant,
            Self::Settings,
        ]
    }
}

/// 题库页状态。
#[derive(Default)]
pub struct HomeState {
    pub filter: crate::models::ProblemFilter,
    pub sort: crate::models::SortBy,
    /// 筛选后的结果缓存，避免每帧重算 4000 条数据的过滤与排序。
    ///
    /// 使用虚拟滚动（`ScrollArea::show_rows`）渲染全部结果，因此不需要
    /// 分页字段——分页与虚拟滚动是互斥的两种方案，同时存在只会互相干扰。
    filtered_indices: Vec<usize>,
    /// 缓存对应的筛选条件版本号，用于判断是否需要重算。
    cache_key: u64,
    pub expanded_tags: bool,
}

/// 设置页的编辑态。
///
/// 与 `AppConfig` 分离的原因：用户可能填到一半就切走页面，
/// 或想放弃修改。用独立的编辑缓冲区可以支持"保存/放弃"语义。
#[derive(Default)]
pub struct SettingsDraft {
    pub username: String,
    pub session_cookie: String,
    pub csrf_token: String,
    /// 用户粘贴的整段 Cookie，用于「自动提取」按钮。
    ///
    /// 不持久化：它只是提取过程的临时载体，提取后即清空。
    pub cookie_blob: String,
    /// 凭据所属站点。「Cookie 总无效」的首要成因是站点选错，故必须可切换。
    pub site_idx: usize,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub temperature: f32,
    pub provider_idx: usize,
    pub page_size: usize,
    /// 是否明文显示 `session_cookie`。
    pub reveal_session: bool,
    /// 是否明文显示 `csrf_token`。
    pub reveal_csrf: bool,
    /// 是否明文显示 `api_key`。
    pub reveal_api_key: bool,
    /// 草稿是否已从配置初始化过。
    initialized: bool,
}

impl SettingsDraft {
    /// 从配置初始化草稿（仅在首次进入或用户主动重置时调用）。
    pub fn sync_from(&mut self, cfg: &AppConfig) {
        self.username = cfg.username.clone();
        self.session_cookie = cfg.session.session_cookie.clone();
        self.csrf_token = cfg.session.csrf_token.clone();
        self.site_idx = crate::config::LeetCodeSite::all()
            .iter()
            .position(|s| *s == cfg.session.site)
            .unwrap_or(0);
        self.base_url = cfg.llm.base_url.clone();
        self.model = cfg.llm.model.clone();
        self.api_key = cfg.llm.api_key.clone();
        self.temperature = cfg.llm.temperature;
        self.provider_idx = crate::config::LlmProvider::all()
            .iter()
            .position(|p| *p == cfg.llm.provider)
            .unwrap_or(0);
        self.page_size = cfg.page_size;
        self.initialized = true;
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

/// 推荐页状态。
#[derive(Default)]
pub struct RecommendState {
    pub result: Option<RecommendationSet>,
    pub config: RecommendConfig,
    /// 当前结果的生成时刻（Unix 秒）。`None` 表示本次运行尚未生成过。
    ///
    /// 结果会持久化并在启动时恢复，因此必须让用户看得见"这份结果是何时
    /// 算出来的"。否则用户会把几天前的推荐当成最新结论——推荐的价值
    /// 与新鲜度直接相关。
    pub generated_at: Option<i64>,
}

/// 竞赛页状态。
#[derive(Default)]
pub struct ContestState {
    pub analysis: Option<ContestAnalysis>,
}

/// 助理页状态。
#[derive(Default)]
pub struct AssistantState {
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub thinking: bool,
    /// Markdown 渲染缓存（`egui_commonmark` 按内容哈希复用布局，
    /// 避免每帧重新解析整段回复）。
    pub markdown_cache: egui_commonmark::CommonMarkCache,
}

/// 应用顶层状态。
pub struct CompassApp {
    /// 共享配置。`Arc<RwLock<..>>` 使其可被客户端动态读取。
    pub config: Arc<RwLock<AppConfig>>,
    pub db: Arc<Database>,
    pub leetcode: LeetCodeClient,
    pub llm: LlmClient,
    /// tokio runtime 句柄，用于从 UI 线程派发后台任务。
    runtime: tokio::runtime::Handle,
    /// 后台 → UI 的消息通道。
    tx: mpsc::UnboundedSender<AppMessage>,
    rx: mpsc::UnboundedReceiver<AppMessage>,

    pub page: Page,
    pub home: HomeState,
    pub settings: SettingsDraft,
    pub recommend: RecommendState,
    pub contest: ContestState,
    pub assistant: AssistantState,

    /// 当前视觉状态：主题模式 + 玻璃开关 + 对应色板。
    ///
    /// 独立于 `settings` 草稿：主题切换要求**即时生效**，而草稿的语义是
    /// "编辑中、保存才生效"。若把主题塞进草稿，用户点切换按钮后界面不会
    /// 变化，必须再点保存——与需求"即时生效"直接冲突。
    ///
    /// 落盘时机：用户点「保存设置」时，当前值随 `AppConfig` 一起写入。
    pub ui_theme: crate::ui::theme::UiTheme,
    /// 已应用到 egui 全局样式的视觉状态。
    ///
    /// 用于判断是否需要重装样式：`Visuals` 只需在主题或玻璃开关**变化**时
    /// 重写。每帧无条件重写会造成 `Style` 的 `Arc::make_mut` 每帧深拷贝，
    /// 不仅浪费，还会让 egui 的样式缓存全部失效。
    applied_theme: Option<(crate::ui::theme::ThemeMode, bool)>,

    /// 全量题库（内存缓存）。
    pub problems: Vec<Problem>,
    /// LeetCode 官方公布的全球题量（按难度）。用于校验本地缓存的完整性。
    /// 未同步过时为 `None`。
    pub global_counts: Option<crate::models::DifficultyCounts>,
    pub profile: Option<UserProfile>,
    pub tag_stats: Vec<TagProgress>,
    pub contest_records: Vec<ContestRecord>,
    pub recent_submissions: Vec<RecentSubmission>,

    /// 题库同步进行中。
    pub syncing: bool,
    /// 同步进度 (已完成, 总数)。
    pub sync_progress: (usize, i64),
    /// 账号数据刷新中。
    pub refreshing_account: bool,

    /// 待显示的一次性提示。
    pub toast: Option<(ToastKind, String, f64)>,
    /// 永久性的错误提示（需用户处理后才消失）。
    /// 持久性错误：消息 + 是否值得重试。
    pub persistent_error: Option<(String, bool)>,
    /// 已授权登录的身份（由服务端确认，而非仅本地填写）。
    ///
    /// 保存完整身份而非仅用户名：中国站的**查询标识（slug）与显示昵称
    /// 可能不同**，只留昵称会丢失"该用什么去查"这个关键信息。
    ///
    /// 该状态会持久化（见 `storage::SessionCheck`），重启后直接恢复，
    /// 用户不必每次打开都重新校验。
    pub verified: Option<LoginIdentity>,

    /// 最近一帧的 egui 上下文。
    ///
    /// 后台任务完成后需要调用 `request_repaint()` 唤醒 UI——否则 egui
    /// 在无用户输入时不会重绘，异步结果要等到用户移动鼠标才显示。
    /// 从 UI 线程派发任务时把当前 `Context` 的克隆一并传进去，
    /// 因此需要在应用里暂存一份。`Context` 的克隆是廉价的（内部为 Arc）。
    last_ctx: Option<Context>,
}

impl CompassApp {
    /// 创建应用。
    ///
    /// 构造阶段会：
    /// 1. 打开数据库（失败则显示错误但仍启动，让用户能看到提示）
    /// 2. 从数据库加载配置
    /// 3. 尝试从缓存加载题库
    /// 4. 若缓存过期或为空，派发后台拉取任务
    pub fn new(cc: &eframe::CreationContext<'_>) -> AppResult<Self> {
        // 必须最先执行：egui 内置字体不含 CJK 字形，不注入中文字体
        // 所有汉字都会渲染成豆腐块（□）。详见 `crate::fonts`。
        crate::fonts::install(&cc.egui_ctx);

        // 之后才能配置样式：字体必须先就位，字号才谈得上意义。
        //
        // 明暗两套 `Visuals` 在此一次性装好（含语义色板、字号层级、
        // 间距栅格与玻璃材质）。此后切换主题只需 `ctx.set_theme()`，
        // 目标主题的样式已就绪。
        //
        // 注意：此处使用默认外观（深色 + 玻璃）。真正的用户配置在
        // 构造完 `CompassApp` 后由首帧的 `sync_theme` 应用——
        // 那时才读得到已加载的 `AppConfig`。
        crate::ui::theme::install(&cc.egui_ctx, &crate::ui::theme::UiTheme::default());

        // 数据库放在应用数据目录，避免把数据混进工作目录。
        let db_path = default_db_path();
        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                // 无法打开数据库时，退化为内存数据库以保证应用仍能启动。
                // 用户会看到提示——总比窗口打不开好。
                eprintln!("警告：无法打开数据库 {db_path:?}: {e}；改用内存模式");
                Database::open_in_memory().map_err(|e2| {
                    AppError::Other(format!("数据库初始化失败: {e}；内存模式亦失败: {e2}"))
                })?
            }
        };

        let (config, config_reset) = storage::load_config(&db).unwrap_or_else(|e| {
            eprintln!("警告：配置加载失败: {e}");
            (AppConfig::default(), false)
        });

        let config = Arc::new(RwLock::new(config));

        // tokio runtime。用独立的 runtime 而非 `#[tokio::main]`：
        // egui 的主循环是阻塞的，两者不能共用一个线程。
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("compass-worker")
            .build()
            .map_err(|e| AppError::Other(format!("无法创建异步运行时: {e}")))?;
        let runtime_handle = runtime.handle().clone();
        // runtime 需要存活到应用结束。交给一个守护线程持有。
        std::thread::Builder::new()
            .name("compass-runtime".into())
            .spawn(move || {
                // 阻塞在此直到进程退出。
                runtime.block_on(std::future::pending::<()>());
            })
            .map_err(|e| AppError::Other(format!("无法启动运行时线程: {e}")))?;

        let (tx, rx) = mpsc::unbounded_channel();

        let leetcode = LeetCodeClient::new(config.clone())?;
        let llm = LlmClient::new(config.clone());

        // 从配置恢复外观设置。
        //
        // 必须通过 `try_read` 取快照而非持有 guard——`config` 随后会被移动
        // 进 `Self`，guard 存活期间无法移动（E0505）。
        let (theme_mode, glass_effect) = config
            .try_read()
            .map(|c| (c.theme, c.glass_effect))
            .unwrap_or((crate::ui::theme::ThemeMode::default(), true));

        let mut app = Self {
            config,
            db: Arc::new(db),
            leetcode,
            llm,
            runtime: runtime_handle,
            tx,
            rx,
            page: Page::Home,
            home: HomeState::default(),
            settings: SettingsDraft::default(),
            recommend: RecommendState::default(),
            contest: ContestState::default(),
            assistant: AssistantState::default(),
            ui_theme: crate::ui::theme::UiTheme::new(theme_mode, glass_effect),
            applied_theme: None,
            problems: Vec::new(),
            global_counts: None,
            profile: None,
            tag_stats: Vec::new(),
            contest_records: Vec::new(),
            recent_submissions: Vec::new(),
            syncing: false,
            sync_progress: (0, 0),
            refreshing_account: false,
            toast: None,
            persistent_error: None,
            verified: None,
            last_ctx: Some(cc.egui_ctx.clone()),
        };

        if config_reset {
            app.show_toast(
                ToastKind::Info,
                "配置文件损坏，已重置为默认值，请重新填写设置。",
            );
        }

        // 从缓存加载题库（同步，因为数据量小且是启动关键路径）。
        app.load_problems_from_cache();

        // 恢复上次的会话校验结论与推荐结果，使重启后不必重做这两件事。
        app.restore_persisted_state();

        // 恢复学习助理的对话历史（持久化在数据库中）。
        app.restore_assistant_history();

        // 后台静默复验一次凭据。
        //
        // 恢复的结论来自上次运行，期间 Cookie 可能已在服务端失效。若不复查，
        // 用户会一直看到"凭据已校验"却取不到数据——那比明确提示过期更糟。
        //
        // 静默：成功不打扰（用户没做任何操作），失败才提示。
        // 未配置完整凭据时 `verify_session` 会直接返回，不产生请求。
        app.verify_session(CheckTrigger::Silent);

        // 缓存缺失或过期则后台拉取。
        app.maybe_sync_problems(true);

        Ok(app)
    }

    /// 从本地恢复"上次已经算好/验好"的状态。
    ///
    /// 恢复两类数据，都遵循同一个原则：**只要结论仍然成立就直接复用，
    /// 不让用户重复劳动**。
    ///
    /// 1. **会话校验结论**——仅在凭据指纹与当前配置一致时沿用。
    ///    凭据被改过（换账号、换 Cookie、切站点）则作废，因为旧结论
    ///    描述的是另一套凭据，沿用会让界面显示"已校验"而实际请求用的是
    ///    别的身份。
    /// 2. **推荐结果**——仅在绑定账号未变时沿用。账号变了，旧推荐
    ///    与新账号无关。
    ///
    /// 全部为同步读取：数据量很小，且属于启动关键路径——若异步加载，
    /// 用户会先看到"未校验/无推荐"再突然跳变，反而显得闪烁。
    fn restore_persisted_state(&mut self) {
        // ---- 会话校验结论 ----
        let fingerprint = self
            .config
            .try_read()
            .ok()
            .map(|c| c.credential_fingerprint());

        if let (Some(fingerprint), Ok(Some(check))) =
            (fingerprint, storage::load_session_check(&self.db))
        {
            if check.fingerprint == fingerprint {
                // 顺手把配置用户名校正为查询标识。存下的结论来自服务端，
                // 比用户手填的更权威；在启动阶段就校正，后续"刷新账号"
                // 第一次请求即可命中，无需再走一轮网络解析。
                self.adopt_login_slug(&check.identity);
                self.verified = Some(check.identity);
            } else {
                // 凭据已变更，旧结论作废——顺手清掉，避免留下误导性数据。
                let _ = storage::clear_session_check(&self.db);
            }
        }

        // ---- 推荐结果 ----
        let username = self
            .config
            .try_read()
            .ok()
            .map(|c| c.username.clone())
            .unwrap_or_default();

        if let Ok(Some(stored)) = storage::load_recommendations(&self.db) {
            if recommendation_cache_applies(&stored.username, &username, self.verified.as_ref()) {
                self.recommend.result = Some(stored.set);
                self.recommend.generated_at = Some(stored.generated_at);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 消息泵
    // -----------------------------------------------------------------------

    /// 处理所有积压的后台消息。每帧调用一次。
    fn drain_messages(&mut self, ctx: &Context) {
        // 限制单帧处理量，防止极端情况下大量消息导致一帧过长而卡顿。
        const MAX_PER_FRAME: usize = 64;

        let mut count = 0;
        while count < MAX_PER_FRAME {
            let Ok(msg) = self.rx.try_recv() else {
                break;
            };
            self.handle_message(msg);
            count += 1;
        }

        // 若仍有余量，请求下一帧继续处理。
        if count == MAX_PER_FRAME {
            ctx.request_repaint();
        }
    }

    fn handle_message(&mut self, msg: AppMessage) {
        match msg {
            AppMessage::ProblemSyncProgress(done, total) => {
                self.syncing = true;
                self.sync_progress = (done, total);
            }
            AppMessage::ProblemsLoaded(result) => {
                self.syncing = false;
                match result {
                    Ok(list) => {
                        let n = list.len();
                        self.problems = list;
                        // 缓存已被后台任务写入数据库。
                        let _ = self.db.mark_problems_fetched();
                        // 刷新全球题量（后台任务已落库）。
                        self.global_counts = self.db.load_global_counts().ok().flatten();
                        self.home.cache_key = 0; // 强制重算筛选缓存
                        self.show_toast(ToastKind::Success, format!("题库同步完成，共 {n} 道题"));
                    }
                    Err((e, retryable)) => {
                        self.persistent_error =
                            Some((format!("题库同步失败：{e}"), retryable));
                    }
                }
            }
            AppMessage::ConfigSaved(result) => match result {
                Ok(()) => self.show_toast(ToastKind::Success, "设置已保存"),
                Err(e) => self.show_toast(ToastKind::Error, format!("设置保存失败：{e}")),
            },
            AppMessage::AccountRefreshed(snapshot) => {
                self.refreshing_account = false;
                let snap = *snapshot;
                self.profile = snap.profile;
                self.tag_stats = snap.tag_stats;
                self.contest_records = snap.contest_records;
                self.recent_submissions = snap.recent_submissions;
                self.contest.analysis = snap.contest_analysis;

                if !snap.warnings.is_empty() {
                    self.show_toast(ToastKind::Info, snap.warnings.join("；"));
                } else {
                    self.show_toast(ToastKind::Success, "账号数据已更新");
                }
            }
            AppMessage::LlmTestResult(result) => match result {
                Ok(reply) => self.show_toast(
                    ToastKind::Success,
                    format!("大模型连接成功，模型回复：{reply}"),
                ),
                Err(e) => self.show_toast(ToastKind::Error, format!("连接失败：{e}")),
            },
            AppMessage::AssistantReply(result) => {
                self.assistant.thinking = false;
                let msg = match result {
                    Ok(text) => ChatMessage::assistant(text),
                    Err(e) => ChatMessage::error(format!("请求失败：{e}")),
                };
                // 持久化：落库失败不阻塞 UI（本帧仍显示回复），只留排障日志。
                if let Err(e) = self.db.append_chat_message(&msg) {
                    eprintln!("保存助理消息失败：{e}");
                }
                self.assistant.messages.push(msg);
            }
            AppMessage::SessionChecked(result, trigger) => match result {
                Ok(Some(identity)) => {
                    self.verified = Some(identity.clone());
                    // 会话里拿到的 slug 比用户在设置页手填的更权威：
                    // 用户手填的多半是昵称，用它查中国站必然 404。
                    // 这里顺手回写，让"刷新账号"能直接命中。
                    self.adopt_login_slug(&identity);
                    // 持久化，使下次启动无需重新校验。
                    self.persist_session_check(&identity);

                    if trigger == CheckTrigger::Manual {
                        let shown = identity.display_label();
                        self.show_toast(
                            ToastKind::Success,
                            format!("凭据有效，已登录为 {shown}"),
                        );
                    }
                }
                Ok(None) => {
                    // 服务端明确表示未登录 —— 凭据确实失效，结论必须作废。
                    let was_verified = self.verified.take().is_some();
                    self.clear_persisted_session_check();
                    match trigger {
                        CheckTrigger::Manual => self.show_toast(
                            ToastKind::Error,
                            "凭据无效或已过期。请重新从浏览器获取 cookie。",
                        ),
                        // 静默复验失败：只在"此前确为已校验"时提示——那说明
                        // Cookie 刚刚失效，用户需要知道。从未校验过则不必打扰。
                        CheckTrigger::Silent if was_verified => self.show_toast(
                            ToastKind::Error,
                            "登录凭据已失效，请重新从浏览器获取 cookie 后重新校验。",
                        ),
                        CheckTrigger::Silent => {}
                    }
                }
                Err(e) => {
                    // 网络类错误**不改变校验结论**。
                    //
                    // 无法确认凭据失效时就不应擅自清除：否则断网启动会把
                    // 一个本来有效的凭据判死，用户白白重填一次。
                    // "查不到"与"确认不存在"是两回事。
                    if trigger == CheckTrigger::Manual {
                        self.show_toast(ToastKind::Error, format!("校验失败：{e}"));
                    }
                }
            },
            AppMessage::Toast(kind, text) => self.show_toast(kind, text),
        }
    }

    /// 显示一次性提示。`kind` 决定颜色。
    pub fn show_toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        // 记录当前时间用于自动消失。使用 `Instant` 语义（此处用秒数近似）。
        self.toast = Some((kind, text.into(), now_secs()));
    }

    /// 当前生效的站点，供 UI 判断功能是否可用。
    ///
    /// 用 `try_read` 而非阻塞读：本方法在**每个 UI 帧**都可能被调用，
    /// 若在 UI 线程阻塞等锁会导致界面卡顿。拿不到锁时返回默认站点
    /// （国际站）——这是安全降级，因为下一帧就会重试。
    pub fn current_site(&self) -> crate::config::LeetCodeSite {
        match self.config.try_read() {
            Ok(g) => g.session.site,
            Err(_) => crate::config::LeetCodeSite::default(),
        }
    }

    /// 检查并清理过期提示（保留 4 秒）。
    fn tick_toast(&mut self) {
        const TTL: f64 = 4.0;
        if let Some((_, _, shown_at)) = &self.toast {
            if now_secs() - shown_at > TTL {
                self.toast = None;
            }
        }
    }

    // -----------------------------------------------------------------------
    // 数据操作
    // -----------------------------------------------------------------------

    /// 从数据库缓存加载题库。
    fn load_problems_from_cache(&mut self) {
        match self.db.load_problems() {
            Ok(list) if !list.is_empty() => {
                self.problems = list;
                self.home.cache_key = 0;
            }
            Ok(_) => {}
            Err(e) => eprintln!("警告：读取题库缓存失败: {e}"),
        }

        // 全球题量统计与题库分离存储，独立读取（缺失不影响启动）。
        self.global_counts = self.db.load_global_counts().ok().flatten();

        // 账号相关缓存：让应用在离线或未刷新时也能展示上次的数据。
        // 没有配置用户名时跳过——无从查起。
        let username = match self.config.try_read() {
            Ok(c) => c.username.trim().to_string(),
            Err(_) => String::new(),
        };

        if username.is_empty() {
            return;
        }

        // 标签统计：推荐算法的输入，缺失时推荐会退化为难度递进模式。
        if let Ok(stats) = self.db.load_tag_stats(&username) {
            if !stats.is_empty() {
                // 用题库总量补全每个标签的 total（缓存中未持久化该值）。
                let mut totals: std::collections::HashMap<String, u32> =
                    std::collections::HashMap::new();
                for p in &self.problems {
                    for t in &p.tags {
                        if !t.slug.is_empty() {
                            *totals.entry(t.slug.clone()).or_insert(0) += 1;
                        }
                    }
                }

                self.tag_stats = stats
                    .into_iter()
                    .map(|mut t| {
                        if let Some(n) = totals.get(&t.tag_slug) {
                            t.total = *n;
                        }
                        t
                    })
                    .collect();
            }
        }

        // 竞赛记录：可用于直接重建分析面板，不必等网络。
        if let Ok(records) = self.db.load_contest_records(&username) {
            if !records.is_empty() {
                // 缓存不含总体排名摘要，因此 summary 用默认值；
                // 刷新账号后会被真实数据覆盖。
                self.contest.analysis = Some(ContestAnalysis::analyze(
                    &records,
                    crate::models::ContestSummary::default(),
                ));
                self.contest_records = records;
            }
        }
    }

    /// 视缓存状态决定是否拉取题库。
    ///
    /// `force` 为 `true` 时无条件刷新（用户手动点击刷新）。
    pub fn maybe_sync_problems(&mut self, force: bool) {
        if self.syncing {
            return;
        }

        if !force {
            // 缓存新鲜且有数据则跳过。
            //
            // 双重校验：时间戳新鲜 **且** 缓存中确实有题。只信时间戳不够——
            // 若上次同步在中途失败并写入了 fetched_at，缓存可能是空的。
            let fresh = self.db.is_problem_cache_fresh().unwrap_or(false);
            let has_rows = self.db.problem_count().map(|n| n > 0).unwrap_or(false);
            if fresh && has_rows && !self.problems.is_empty() {
                return;
            }
        }

        self.start_problem_sync();
    }

    /// 派发题库全量拉取任务。
    fn start_problem_sync(&mut self) {
        self.syncing = true;
        self.sync_progress = (0, 0);
        self.persistent_error = None;

        let client = self.leetcode.clone();
        let db = self.db.clone();
        let tx = self.tx.clone();
        let ctx = self.last_ctx.clone();

        self.runtime.spawn(async move {
            // 先取权威总题数（匿名可用）。它用于：
            // 1. 校验全量拉取是否完整（拉到的数量应不少于该值）；
            // 2. 首页显示"已缓存 / 全球总量"的比例。
            // 失败不影响主流程，仅退化为本地统计。
            if let Ok(counts) = client.fetch_global_counts().await {
                if let Err(e) = db.save_global_counts(&counts) {
                    eprintln!("警告：全球题量统计写入失败: {e}");
                }
            }

            // 进度回调：把进度投回 UI。
            let progress_tx = tx.clone();
            let progress_ctx = ctx.clone();
            let mut last_reported = 0usize;

            let result = client
                .fetch_all_problems(move |done, total| {
                    // 节流：每 200 条或拉完时上报一次，避免消息洪泛。
                    if done - last_reported >= 200 || done as i64 >= total {
                        last_reported = done;
                        let _ = progress_tx.send(AppMessage::ProblemSyncProgress(done, total));
                        if let Some(c) = &progress_ctx {
                            c.request_repaint();
                        }
                    }
                })
                .await;

            match result {
                Ok(problems) => {
                    // 落库失败不应阻止 UI 更新，但要告知用户。
                    let save_err = db
                        .upsert_problems(&problems)
                        .err()
                        .map(|e| format!("题库缓存写入失败：{e}"));

                    let _ = tx.send(AppMessage::ProblemsLoaded(Ok(problems)));
                    if let Some(e) = save_err {
                        let _ = tx.send(AppMessage::Toast(ToastKind::Info, e));
                    }
                }
                Err(e) => {
                    let retryable = e.is_retryable();
                    let _ = tx.send(AppMessage::ProblemsLoaded(Err((e.to_string(), retryable))));
                }
            }
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        });
    }

    /// 把会话中解析出的账号标识回写进配置。
    ///
    /// ## 为什么需要回写
    ///
    /// 中国站的账号查询标识是 **ASCII slug**（如 `wuhu`），而用户在设置页
    /// 填写、以及界面各处展示的都是**显示昵称**（如「梧糊」）。用户改过
    /// 昵称后两者不一致，而用户通常只知道昵称——直接拿昵称去查一律返回
    /// `null`，表现为"用户不存在"，实际只是查错了字段。
    ///
    /// 会话（`userStatus`）是唯一能拿到 slug 的来源：它来自服务端自己
    /// 认定的登录身份，比用户手填的权威。因此一旦校验通过就顺手写回，
    /// 后续"刷新账号"即可直接命中。
    ///
    /// ## 只在中国站回写
    ///
    /// 国际站的 `userSlug` 与 `username` 语义一致（也是 URL 里那一段），
    /// 回写只是把用户填的规范化大小写，无副作用但也没必要——保持配置
    /// 不可变性更省心。
    fn adopt_login_slug(&mut self, identity: &LoginIdentity) {
        let slug = identity.slug.trim();
        if slug.is_empty() {
            return;
        }

        // 站点判断要用 try_read 的非阻塞写法，并且**在写锁之前**完成，
        // 否则会同时持有读锁与写锁（同一把 RwLock，必然死锁）。
        let is_cn = matches!(
            self.config.try_read().as_deref().map(|c| c.session.site),
            Ok(crate::config::LeetCodeSite::Cn)
        );
        if !is_cn {
            return;
        }

        // 用 try_write 而非阻塞写：这里是 UI 线程。
        let Ok(mut cfg) = self.config.try_write() else {
            return;
        };

        if cfg.username == slug {
            return;
        }

        cfg.username = slug.to_string();

        // 落盘失败不阻断流程：内存里已经改好，本次会话可用；
        // 下次启动会回到旧值，但届时校验会话会再次回写。
        let snapshot = cfg.clone();
        drop(cfg);

        let db = self.db.clone();
        self.runtime.spawn(async move {
            if let Err(e) = storage::save_config(&db, &snapshot) {
                eprintln!("回写账号标识失败：{e}");
            }
        });
    }

    /// 刷新账号数据（画像、标签、竞赛、提交）。
    pub fn refresh_account(&mut self) {
        // 同步读取用户名：用 try_read 避免在 UI 线程阻塞。
        //
        // 关键写法：先把 `Option<String>` 取出来，让读锁 guard 在语句结束
        // 时立即释放；若直接在 `match` 里调用 `self.show_toast(..)`，
        // guard 会活到表达式末尾，与 `&mut self` 冲突（E0502）。
        let cfg_username = match self.config.try_read() {
            Ok(g) => Some(g.username.clone()),
            Err(_) => None,
        };

        // 同样用 guard-即释的写法取出站点中文名，供错误文案使用。
        let site_label = match self.config.try_read() {
            Ok(g) => g.session.site.label_zh(),
            Err(_) => "未知站点",
        };

        let Some(cfg_username) = cfg_username else {
            self.show_toast(ToastKind::Info, "配置正忙，请稍后重试");
            return;
        };

        if cfg_username.trim().is_empty() {
            self.show_toast(ToastKind::Error, "请先在设置页填写 LeetCode 用户名");
            return;
        }

        if let Err(e) = UserProfile::validate_username(&cfg_username) {
            self.show_toast(ToastKind::Error, e);
            return;
        }

        if self.refreshing_account {
            return;
        }
        self.refreshing_account = true;

        let client = self.leetcode.clone();
        let db = self.db.clone();
        let tx = self.tx.clone();
        let ctx = self.last_ctx.clone();
        // 可变：中国站若发现用户填的是显示昵称，会用登录态里的 slug 纠正它，
        // 后续所有查询都必须跟着切换到正确的标识（见下方自救逻辑）。
        let mut username = cfg_username;

        self.runtime.spawn(async move {
            let mut warnings: Vec<String> = Vec::new();

            // 画像：关键数据，失败则整体失败。
            let profile = match client.fetch_profile(&username).await {
                Ok(p) => Some(p),
                Err(e) => {
                    if e.is_auth_related() {
                        warnings.push(format!("画像获取受限：{e}"));
                    } else {
                        // 附上当前站点，因为"两站账号不通用"是最常见的成因：
                        // 用户在国际站查中国站的用户名，会得到"未找到用户"。
                        // 不点明站点的话，用户只会反复核对用户名拼写。
                        let _ = tx.send(AppMessage::AccountRefreshed(Box::new(AccountSnapshot {
                            profile: None,
                            tag_stats: Vec::new(),
                            contest_records: Vec::new(),
                            contest_analysis: None,
                            recent_submissions: Vec::new(),
                            warnings: vec![format!(
                                "账号数据获取失败（当前站点：{}）：{e}",
                                site_label
                            )],
                        })));
                        if let Some(c) = &ctx {
                            c.request_repaint();
                        }
                        return;
                    }
                    None
                }
            };

            // 校正查询标识。
            //
            // 中国站的查询标识是 **slug**，与显示昵称可能不同。用户填昵称时
            // `fetch_profile` 内部会自救（成功则不报错），但**本地 `username`
            // 仍是错的**——后续的标签、竞赛、提交记录查询会继续用错误标识，
            // 得到"画像有数据、其余全空"的半失效状态，比彻底失败更难排查。
            //
            // 因此这里无条件解析一次真实标识并切换。国际站直接返回 `None`，
            // 不做任何额外请求。解析不出（未登录）时 `username` 保持原样。
            let had_profile = profile.is_some();
            if let Some(slug) = client.resolve_query_identifier(&username).await {
                // 仅当画像确实拿到了才提示，否则交给下面的失败文案统一说明。
                if had_profile {
                    warnings.push(format!(
                        "已自动改用账号标识 「{slug}」 查询\
                         （你填写的 「{username}」 是显示昵称，本站按标识查找）。",
                    ));
                }
                username = slug;
            }

            if profile.is_none() {
                // 自救也失败：走到这里说明确实无法解析账号，保持整体失败。
                let _ = tx.send(AppMessage::AccountRefreshed(Box::new(AccountSnapshot {
                    profile: None,
                    tag_stats: Vec::new(),
                    contest_records: Vec::new(),
                    contest_analysis: None,
                    recent_submissions: Vec::new(),
                    warnings: vec![format!(
                        "账号数据获取失败（当前站点：{}）：未能解析账号「{username}」。\
                         该站按账号标识（slug）查询，若你的标识与昵称不同，\
                         请在设置页填写会话凭据后点击「校验凭据」，\
                         应用会自动识别正确的标识。",
                        site_label
                    )],
                })));
                if let Some(c) = &ctx {
                    c.request_repaint();
                }
                return;
            };
            // 标签统计：推荐算法的核心输入，失败仅告警。
            //
            // 中国站不支持此查询（无 tagProblemCounts），此时**不应报为错误**——
            // 那是站点能力边界而非故障。静默降级为空，由推荐页自行说明。
            let mut tag_stats = match client.fetch_tag_stats(&username).await {
                Ok(t) => t,
                Err(e) if e.is_unsupported_on_site() => Vec::new(),
                Err(e) => {
                    warnings.push(format!("技能分布获取失败：{e}"));
                    Vec::new()
                }
            };

            // 用题库总量补全每个标签的 total 字段。
            if let Ok(problems) = db.load_problems() {
                let mut totals: std::collections::HashMap<String, u32> =
                    std::collections::HashMap::new();
                for p in &problems {
                    for t in &p.tags {
                        if !t.slug.is_empty() {
                            *totals.entry(t.slug.clone()).or_insert(0) += 1;
                        }
                    }
                }
                for t in tag_stats.iter_mut() {
                    if let Some(n) = totals.get(&t.tag_slug) {
                        t.total = *n;
                    }
                }
            }

            // 落库标签统计（失败不致命）。
            if let Err(e) = db.save_tag_stats(&username, &tag_stats) {
                warnings.push(format!("技能分布缓存失败：{e}"));
            }

            // 提交日历：需要认证。失败仅告警——它只影响画像页的活跃度展示。
            let mut profile = profile;
            if let Some(p) = profile.as_mut() {
                match client.fetch_calendar(&username).await {
                    Ok(cal) => {
                        p.submission_calendar = cal.0;
                        p.streak = cal.1;
                        p.total_active_days = cal.2;
                    }
                    Err(e) => {
                        // 未认证时这是预期行为，不必打扰用户。
                        // 站点不支持同理——属能力边界，不是故障。
                        if !e.is_auth_related() && !e.is_unsupported_on_site() {
                            warnings.push(format!("提交日历获取失败：{e}"));
                        }
                    }
                }
            }

            // 竞赛：可缺失。中国站不支持竞赛接口，静默降级。
            let contest_records = match client.fetch_contest_history(&username).await {
                Ok(r) => r,
                Err(e) if e.is_unsupported_on_site() => Vec::new(),
                Err(e) => {
                    warnings.push(format!("竞赛记录获取失败：{e}"));
                    Vec::new()
                }
            };
            let contest_summary = client
                .fetch_contest_summary(&username)
                .await
                .unwrap_or_default();

            let contest_analysis = if contest_records.is_empty() {
                None
            } else {
                let analysis = ContestAnalysis::analyze(&contest_records, contest_summary);
                if let Err(e) = db.save_contest_records(&username, &contest_records) {
                    warnings.push(format!("竞赛记录缓存失败：{e}"));
                }
                Some(analysis)
            };

            // 近期提交：需要认证，匿名时返回空。
            let recent_submissions = client.fetch_recent_submissions(20).await.unwrap_or_default();

            // 用提交记录回写已缓存题目的完成状态。
            //
            // 场景：用户先匿名同步了题库（status 全为 Unknown），之后才配置
            // 会话凭据。仅刷新账号不会重新拉取题库，若不回写，首页的"完成
            // 状态"会一直停在"未知"。这里把从提交记录中确知的状态增量落库。
            if !recent_submissions.is_empty() {
                let updates: Vec<(String, crate::models::SolveStatus)> = recent_submissions
                    .iter()
                    .map(|s| {
                        let status = if s.is_accepted {
                            crate::models::SolveStatus::Solved
                        } else {
                            crate::models::SolveStatus::Attempted
                        };
                        (s.title_slug.clone(), status)
                    })
                    .collect();

                if let Err(e) = db.update_problem_statuses(&updates) {
                    warnings.push(format!("完成状态回写失败：{e}"));
                }
            }

            let _ = tx.send(AppMessage::AccountRefreshed(Box::new(AccountSnapshot {
                profile,
                tag_stats,
                contest_records,
                contest_analysis,
                recent_submissions,
                warnings,
            })));

            if let Some(c) = &ctx {
                c.request_repaint();
            }
        });
    }

    /// 校验当前会话凭据是否仍然有效。
    ///
    /// `trigger` 决定结果是否打扰用户，见 [`CheckTrigger`]。
    /// 未配置完整凭据时直接返回——省掉一次注定失败的请求。
    pub fn verify_session(&mut self, trigger: CheckTrigger) {
        let authenticated = self
            .config
            .try_read()
            .map(|c| c.is_authenticated())
            .unwrap_or(false);
        if !authenticated {
            if trigger == CheckTrigger::Manual {
                self.show_toast(ToastKind::Error, "请先填写完整的会话凭据");
            }
            return;
        }

        let client = self.leetcode.clone();
        let tx = self.tx.clone();
        let ctx = self.last_ctx.clone();

        self.runtime.spawn(async move {
            let result = client
                .check_session()
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::SessionChecked(result, trigger));
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        });
    }

    /// 持久化会话校验结论，使重启后无需重新校验。
    fn persist_session_check(&self, identity: &LoginIdentity) {
        let Some(fingerprint) = self
            .config
            .try_read()
            .ok()
            .map(|c| c.credential_fingerprint())
        else {
            // 拿不到配置就不写：宁可下次重新校验，也不要存一份指纹不明的
            // 结论——那会让启动时的比对失去意义。
            return;
        };

        let check = storage::SessionCheck {
            identity: identity.clone(),
            checked_at: now_unix_i64(),
            fingerprint,
        };

        let db = self.db.clone();
        self.runtime.spawn(async move {
            if let Err(e) = storage::save_session_check(&db, &check) {
                eprintln!("保存会话校验结论失败：{e}");
            }
        });
    }

    /// 清除持久化的会话校验结论。
    fn clear_persisted_session_check(&self) {
        let db = self.db.clone();
        self.runtime.spawn(async move {
            if let Err(e) = storage::clear_session_check(&db) {
                eprintln!("清除会话校验结论失败：{e}");
            }
        });
    }

    /// 生成推荐。
    pub fn generate_recommendations(&mut self) {
        if self.problems.is_empty() {
            self.show_toast(ToastKind::Error, "题库为空，请先同步题库");
            return;
        }

        let authenticated = self.config.try_read().map(|c| c.is_authenticated()).unwrap_or(false);
        let mode = RecommendMode::detect(authenticated);

        let set = crate::logic::recommend::RecommendationEngine::recommend(
            &self.problems,
            &self.tag_stats,
            self.profile.as_ref(),
            mode,
            &self.recommend.config,
        );

        // 推荐是纯 CPU 计算（4000 条数据的打分排序在毫秒级），
        // 无需派发到后台线程。
        self.recommend.result = Some(set.clone());
        self.recommend.generated_at = Some(now_unix_i64());

        // 落库缓存：推荐由本地数据推导，输入不变则结果必然相同，
        // 重启后直接复用比让用户再点一次「生成推荐」更合理。
        let stored = storage::StoredRecommendations {
            generated_at: self.recommend.generated_at.unwrap_or_default(),
            username: self
                .config
                .try_read()
                .map(|c| c.username.clone())
                .unwrap_or_default(),
            set,
        };
        let db = self.db.clone();
        self.runtime.spawn(async move {
            if let Err(e) = storage::save_recommendations(&db, &stored) {
                eprintln!("缓存推荐结果失败：{e}");
            }
        });
    }

    /// 测试大模型连通性。
    pub fn test_llm(&mut self) {
        let llm = self.llm.clone();
        let tx = self.tx.clone();
        let ctx = self.last_ctx.clone();

        self.runtime.spawn(async move {
            let result = llm
                .test_connection()
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::LlmTestResult(result));
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        });
    }

    /// 发送助理消息。
    /// 从数据库恢复学习助理的对话历史。
    ///
    /// 只加载最近 [`MAX_HISTORY_MESSAGES`] 条：更早的历史保留在数据库里
    /// （查看完整历史的需求出现时再做分页），加载过多会拖慢启动。
    pub fn restore_assistant_history(&mut self) {
        const MAX_HISTORY_MESSAGES: usize =
            crate::logic::assistant::MAX_HISTORY_MESSAGES;
        match self.db.load_chat_messages(MAX_HISTORY_MESSAGES) {
            Ok(messages) if !messages.is_empty() => {
                self.assistant.messages = messages;
            }
            Ok(_) => {}
            Err(e) => eprintln!("恢复助理对话历史失败：{e}"),
        }
    }

    /// 清空学习助理的对话历史（数据库 + 内存）。
    pub fn clear_assistant_history(&mut self) {
        if let Err(e) = self.db.clear_chat_messages() {
            eprintln!("清空助理对话历史失败：{e}");
        }
        self.assistant.messages.clear();
        self.show_toast(ToastKind::Info, "对话已清空");
    }

    pub fn send_assistant_message(&mut self) {
        let input = self.assistant.input.trim().to_string();
        if input.is_empty() || self.assistant.thinking {
            return;
        }

        let user_msg = ChatMessage::user(input.clone());
        // 持久化：落库失败不阻塞发送（本会话内仍可见），只留排障日志。
        if let Err(e) = self.db.append_chat_message(&user_msg) {
            eprintln!("保存用户消息失败：{e}");
        }
        self.assistant.messages.push(user_msg);
        self.assistant.input.clear();
        self.assistant.thinking = true;

        // 组装上下文（在 UI 线程完成，因为需要访问本地状态）。
        let ctx_data = self.build_assistant_context();
        let history = self.assistant.messages.clone();
        let max_turns = self
            .config
            .try_read()
            .map(|c| c.llm.max_history_turns)
            .unwrap_or(10);

        let messages = ctx_data.build_messages(&history, &input, max_turns);

        let llm = self.llm.clone();
        let tx = self.tx.clone();
        let ui_ctx = self.last_ctx.clone();

        self.runtime.spawn(async move {
            let result = llm.chat(messages).await.map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::AssistantReply(result));
            if let Some(c) = &ui_ctx {
                c.request_repaint();
            }
        });
    }

    /// 组装助理所需的上下文数据。
    pub fn build_assistant_context(&self) -> AssistantContext {
        let authenticated = self
            .config
            .try_read()
            .map(|c| c.is_authenticated())
            .unwrap_or(false);

        AssistantContext {
            profile: self.profile.clone(),
            tag_stats: self.tag_stats.clone(),
            recommendations: self.recommend.result.clone(),
            contest_records: self.contest_records.clone(),
            contest_analysis: self.contest.analysis.clone(),
            recent_submissions: self.recent_submissions.clone(),
            problem_count: self.problems.len(),
            authenticated,
            site: self.current_site(),
        }
    }

    /// 保存设置草稿到配置与数据库。
    pub fn save_settings(&mut self) {
        let provider = crate::config::LlmProvider::all()
            .get(self.settings.provider_idx)
            .copied()
            .unwrap_or(crate::config::LlmProvider::OpenAiCompatible);

        let new_cfg = AppConfig {
            username: self.settings.username.clone(),
            session: crate::config::LeetCodeSession {
                session_cookie: self.settings.session_cookie.clone(),
                csrf_token: self.settings.csrf_token.clone(),
                site: crate::config::LeetCodeSite::all()
                    .get(self.settings.site_idx)
                    .copied()
                    .unwrap_or_default(),
            },
            llm: crate::config::LlmConfig {
                provider,
                base_url: self.settings.base_url.clone(),
                model: self.settings.model.clone(),
                api_key: self.settings.api_key.clone(),
                temperature: self.settings.temperature,
                timeout_secs: 60,
                max_history_turns: 10,
            },
            page_size: self.settings.page_size,
            // 外观设置由独立的状态字段持有（不走设置草稿）：
            // 主题切换要求**即时生效**，而草稿的语义是"保存才生效"。
            // 因此这两项跟随 `self.ui_theme` 当前值落盘。
            theme: self.ui_theme.mode,
            glass_effect: self.ui_theme.glass,
        };

        // 校验：把问题在保存前暴露，而不是等到使用时才报错。
        if !new_cfg.username.trim().is_empty() {
            if let Err(e) = UserProfile::validate_username(&new_cfg.username) {
                self.show_toast(ToastKind::Error, e);
                return;
            }
        }
        if let Err(e) = new_cfg.session.validate() {
            // 未配置凭据是允许的（降级模式），只有填了一半才报警。
            if !new_cfg.session.is_empty() {
                self.show_toast(ToastKind::Error, e);
                return;
            }
        }
        // LLM 配置校验。
        //
        // 说明：Base URL 与 Model 有默认值，因此"是否填过字段"无法用来判断
        // 用户意图（默认值恒非空）。真正的判据是 **API Key 是否为空**——
        // 空表示不启用 AI，属于合法状态，由 `LlmConfig::validate()` 内部处理。
        if let Err(e) = new_cfg.llm.validate() {
            self.show_toast(ToastKind::Error, e);
            return;
        }

        // 写入共享配置。
        //
        // 先记下旧值：凭据或账号一旦变化，之前持久化的"校验结论"与
        // "推荐缓存"就都属于另一套身份/另一份数据，必须一并作废。
        // 这类失效判断只能在**写入前**取到旧值，写完就无从比较了。
        let old_fingerprint = self
            .config
            .try_read()
            .ok()
            .map(|g| g.credential_fingerprint());
        let old_username = self.config.try_read().ok().map(|g| g.username.clone());

        {
            let mut normalized = new_cfg.clone();
            normalized.normalize();
            // 使用 blocking_write 可能死锁（同线程已持有读锁时），
            // 因此这里用 try_write 并在失败时提示重试。
            //
            // 注意借用生命周期：guard 必须先取出并立即消费，再调用
            // `self.show_toast(..)`；否则 guard 与 `&mut self` 冲突（E0502）。
            let wrote = match self.config.try_write() {
                Ok(mut guard) => {
                    *guard = normalized;
                    true
                }
                Err(_) => false,
            };

            if !wrote {
                self.show_toast(ToastKind::Info, "配置正忙，请稍后再试");
                return;
            }
        }

        // 依据新旧值差异作废过期缓存。
        let new_fingerprint = self
            .config
            .try_read()
            .ok()
            .map(|g| g.credential_fingerprint());
        let new_username = self.config.try_read().ok().map(|g| g.username.clone());

        let invalid = invalidation_for(
            old_fingerprint.as_deref(),
            new_fingerprint.as_deref(),
            old_username.as_deref(),
            new_username.as_deref(),
        );

        if invalid.session_check {
            // 凭据变了：旧的校验结论描述的是另一套凭据。
            // 不清掉的话，界面会显示"凭据已校验"，而实际请求用的是新凭据——
            // 状态与事实不符，比要求重校验更糟。
            if self.verified.is_some() {
                self.show_toast(ToastKind::Info, "凭据已变更，原校验结论已失效。");
            }
            self.verified = None;
            self.clear_persisted_session_check();
        }

        if invalid.recommendations {
            // 换了账号：旧推荐与新账号无关，继续展示会误导。
            self.recommend.result = None;
            self.recommend.generated_at = None;
        }

        // 落库。
        let db = self.db.clone();
        let cfg_snapshot = new_cfg.clone();
        let tx = self.tx.clone();
        let ctx = self.last_ctx.clone();

        self.runtime.spawn(async move {
            let result = storage::save_config(&db, &cfg_snapshot).map_err(|e| e.to_string());
            let _ = tx.send(AppMessage::ConfigSaved(result));
            if let Some(c) = &ctx {
                c.request_repaint();
            }
        });
    }

    /// 计算首页筛选结果。
    ///
    /// 结果被缓存在 `HomeState.filtered_indices` 中，仅当筛选条件或
    /// 题库变化时重算——否则每帧过滤 4000+ 条数据会造成明显卡顿。
    pub fn filtered_problems(&mut self) -> Vec<usize> {
        let key = self.compute_filter_key();

        if key != self.home.cache_key {
            let mut idx: Vec<usize> = self
                .problems
                .iter()
                .enumerate()
                .filter(|(_, p)| self.home.filter.matches(p))
                .map(|(i, _)| i)
                .collect();

            let problems = &self.problems;
            match self.home.sort {
                crate::models::SortBy::FrontendId => idx.sort_by(|&a, &b| {
                    problems[a]
                        .frontend_id_numeric()
                        .cmp(&problems[b].frontend_id_numeric())
                        .then_with(|| problems[a].frontend_id.cmp(&problems[b].frontend_id))
                }),
                crate::models::SortBy::Difficulty => idx.sort_by(|&a, &b| {
                    problems[a]
                        .difficulty
                        .order()
                        .cmp(&problems[b].difficulty.order())
                        .then_with(|| {
                            problems[a]
                                .frontend_id_numeric()
                                .cmp(&problems[b].frontend_id_numeric())
                        })
                }),
                crate::models::SortBy::AcRateDesc => idx.sort_by(|&a, &b| {
                    problems[b]
                        .ac_rate
                        .partial_cmp(&problems[a].ac_rate)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| {
                            problems[a]
                                .frontend_id_numeric()
                                .cmp(&problems[b].frontend_id_numeric())
                        })
                }),
                crate::models::SortBy::AcRateAsc => idx.sort_by(|&a, &b| {
                    problems[a]
                        .ac_rate
                        .partial_cmp(&problems[b].ac_rate)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| {
                            problems[a]
                                .frontend_id_numeric()
                                .cmp(&problems[b].frontend_id_numeric())
                        })
                }),
            }

            self.home.filtered_indices = idx;
            self.home.cache_key = key;
        }

        self.home.filtered_indices.clone()
    }

    /// 计算筛选条件的缓存键。
    fn compute_filter_key(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut h = DefaultHasher::new();
        self.problems.len().hash(&mut h);
        self.home.sort.hash(&mut h);
        self.home.filter.keyword.hash(&mut h);
        self.home.filter.hide_paid_only.hash(&mut h);

        // 难度：用固定顺序编码，避免依赖 Vec 的迭代顺序。
        for d in crate::models::Difficulty::all() {
            self.home.filter.difficulties.contains(&d).hash(&mut h);
        }
        for s in [
            crate::models::SolveStatus::Solved,
            crate::models::SolveStatus::Attempted,
            crate::models::SolveStatus::Todo,
            crate::models::SolveStatus::Unknown,
        ] {
            self.home.filter.statuses.contains(&s).hash(&mut h);
        }

        // 标签：排序后哈希，保证与顺序无关。
        let mut tags = self.home.filter.tags.clone();
        tags.sort();
        for t in &tags {
            t.hash(&mut h);
        }

        h.finish()
    }

    /// 强制刷新筛选缓存（题库或状态变化后调用）。
    pub fn invalidate_filter_cache(&mut self) {
        self.home.cache_key = 0;
    }
}

/// 让 `SortBy` 可参与哈希（用于缓存键）。
impl std::hash::Hash for crate::models::SortBy {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (*self as u8).hash(state);
    }
}

/// 取当前时间（秒，浮点）。用于提示自动消失的计时。
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// 当前 Unix 时间戳（整秒）。
///
/// 与 [`now_secs`] 分开：前者用于 toast 计时（需要亚秒精度），
/// 这个用于**持久化的时间戳**（只需要秒级，且整数便于序列化与比较）。
fn now_unix_i64() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 配置变更后需要作废哪些缓存。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct CacheInvalidation {
    /// 凭据已变更 → 会话校验结论作废。
    session_check: bool,
    /// 账号已变更 → 推荐缓存作废。
    recommendations: bool,
}

/// 比较新旧配置，判定哪些缓存失效。
///
/// 抽成纯函数的原因：`CompassApp` 需要 eframe 的 `CreationContext` 才能
/// 构造，难以单元测试；而这段判定恰恰是最容易出错的地方——漏判会让界面
/// 显示与实际凭据不符的状态，误判则让用户白白重做一次。
///
/// 入参为 `None` 表示**读取失败**（配置锁被占用）。此时一律判为"不变"：
/// 拿不到确定的新旧值就不该擅自作废缓存，否则一次瞬时锁竞争就会清掉
/// 用户的有效数据。
fn invalidation_for(
    old_fingerprint: Option<&str>,
    new_fingerprint: Option<&str>,
    old_username: Option<&str>,
    new_username: Option<&str>,
) -> CacheInvalidation {
    CacheInvalidation {
        session_check: matches!(
            (old_fingerprint, new_fingerprint),
            (Some(a), Some(b)) if a != b
        ),
        recommendations: matches!(
            (old_username, new_username),
            (Some(a), Some(b)) if a != b
        ),
    }
}

/// 判断一份推荐缓存是否还属于当前账号。
///
/// ## 为什么不能只比较配置里的用户名
///
/// 中国站的配置用户名会被 `adopt_login_slug` **自动改写**：用户原本填的
/// 是显示昵称（如「梧糊」），校验会话后应用把配置改成查询标识（`wuhu`）。
/// 若只比较配置用户名，这个改写会让此前生成的推荐缓存**凭空失效**——
/// 用户既没换账号也没改数据，却看到推荐被清空。
///
/// 因此把三种"同一个人的名字"都视为命中：
/// 当前配置用户名、会话解析出的 slug、会话解析出的显示昵称。
///
/// 注意 `verified` 为 `None`（未校验）时只有配置用户名可用——此时也就
/// 无从得知别名，比较退化为精确匹配，这是正确的保守行为。
fn recommendation_cache_applies(
    stored_username: &str,
    configured_username: &str,
    verified: Option<&LoginIdentity>,
) -> bool {
    if stored_username == configured_username {
        return true;
    }
    match verified {
        Some(id) => stored_username == id.slug || stored_username == id.username,
        None => false,
    }
}

/// 页面到侧边栏图标的映射。
fn page_icon(page: Page) -> crate::ui::theme::IconKind {
    use crate::ui::theme::IconKind;
    match page {
        Page::Home => IconKind::List,
        Page::Recommend => IconKind::Target,
        Page::Contest => IconKind::Chart,
        Page::Assistant => IconKind::Chat,
        Page::Settings => IconKind::Gear,
    }
}

/// 侧边栏导航项。
///
/// 选中态用"左侧强调竖条 + 实心底 + 高对比文字"三重表达。
/// 单靠底色高亮在小尺寸导航里不够醒目，加竖条后当前页一目了然。
///
/// ## 颜色为什么不用通用的 `*_overlay` 令牌
///
/// 三态色值一律取自 `Palette` 的 `sidebar_item_*` 组，而不是内容区
/// 列表行用的 `hover_overlay` / `selected_overlay`。原因是这两类元素的
/// 可辨识度要求差一个量级：列表行的悬停只需提示"鼠标在哪一行"，
/// 而导航项要回答"我在哪一页、还能去哪一页"。
///
/// 深色主题下这个差别是**致命的**：改造前选中项的文字用 `accent`
/// （`#60A5FA`），底用 16% 的 `selected_overlay`——两者同色系且明度接近，
/// 实测对比度只有约 1.3:1，当前页标题基本糊在背景里。
/// 换成 `sidebar_item_*` 后，选中项的文字/底对比度提升到约 5.7:1。
/// 该下限由 `theme` 模块的测试锁定。
///
/// 返回是否被点击。
fn nav_item(
    ui: &mut egui::Ui,
    p: &crate::ui::theme::Palette,
    label: &str,
    icon: crate::ui::theme::IconKind,
    selected: bool,
) -> bool {
    use crate::ui::theme as th;

    const ITEM_HEIGHT: f32 = 40.0;
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ITEM_HEIGHT), egui::Sense::click());

    let hovered = response.hovered();
    let painter = ui.painter();

    // 底：选中 > 悬停 > 无。
    //
    // 选中/悬停底**横向内缩 6px 并加 8px 圆角**：贴边直角块会与侧边栏
    // 边缘生硬相接，内缩后的圆角条呈"悬浮胶囊"感（按用户反馈新增）。
    // 点击热区保持整行不变——只缩小绘制范围，不影响可点面积。
    //
    // `backdrop` 记录这一层实际叠加后的颜色，供图标挖空使用（见下）。
    // 这三个色值都是**不透明**的（见 `Palette` 组 9 的说明），
    // 因此可以直接当作底板色传给 `icon`，不存在"叠加后取近似"的误差。
    const BG_INSET: f32 = 6.0;
    const BG_RADIUS: u8 = 8;
    let bg_rect = rect.shrink2(egui::vec2(BG_INSET, 0.0));
    let mut backdrop = p.glass_fill_sidebar;
    if selected {
        painter.rect_filled(bg_rect, egui::CornerRadius::same(BG_RADIUS), p.sidebar_item_selected_bg);
        backdrop = p.sidebar_item_selected_bg;
        // 左侧强调竖条（3px 宽，两端圆角）。随底色一起内缩，
        // 落在圆角条内部的直线段上（圆角半径 8 > 竖条上沿 8px 内缩，
        // 直线段从 top+8 开始，恰好避开上圆角）。
        let bar = egui::Rect::from_min_size(
            egui::pos2(bg_rect.left() + 3.0, rect.top() + 8.0),
            egui::vec2(3.0, ITEM_HEIGHT - 16.0),
        );
        painter.rect_filled(bar, egui::CornerRadius::same(2), p.sidebar_item_selected_text);
    } else if hovered {
        painter.rect_filled(bg_rect, egui::CornerRadius::same(BG_RADIUS), p.sidebar_item_hover);
        backdrop = p.sidebar_item_hover;
    }

    // 图标与文字。
    let icon_color = if selected {
        p.sidebar_item_selected_text
    } else {
        p.sidebar_item_icon
    };
    let text_color = if selected {
        p.sidebar_item_selected_text
    } else {
        p.sidebar_item_text
    };

    let icon_center = egui::pos2(
        rect.left() + th::space::LG + th::ICON_SIZE * 0.5,
        rect.center().y,
    );
    // 图标画在侧边栏玻璃上，底板色必须传玻璃底/叠加层，不能传 `panel_fill`
    // ——后者与玻璃底不同色，会让月亮图标的缺口露出色斑。
    th::icon(ui, icon_center, th::ICON_SIZE, icon, icon_color, backdrop);

    painter.text(
        egui::pos2(
            rect.left() + th::space::LG + th::ICON_SIZE + 10.0,
            rect.center().y,
        ),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(th::text::BODY),
        text_color,
    );

    response.clicked()
}

/// 侧边栏内的分隔线（左右留白）。
///
/// `color` 由调用方给出：侧边栏里这条线不止一处用途，且各处的分量要求
/// 不同——品牌区下方那条是**结构性**的（标记"品牌区到此为止"），
/// 需要 `divider`；而若将来在导航组之间再加分组线，用 `glass_stroke`
/// 这种更轻的档位才合适。把色值做成参数，避免调用方为了换个档位
/// 而在 `app.rs` 里手写颜色字面量（`ui::guards` 会直接判失败）。
fn nav_separator(ui: &mut egui::Ui, color: egui::Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    let inset = crate::ui::theme::space::LG;
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + inset, rect.center().y),
            egui::pos2(rect.right() - inset, rect.center().y),
        ],
        egui::Stroke::new(1.0, color),
    );
}

/// 玻璃文字按钮（顶栏操作用）。
///
/// 返回是否被点击。悬停时主色描边 + 主色文字，反馈明确。
fn glass_text_button(ui: &mut egui::Ui, p: &crate::ui::theme::Palette, label: &str) -> bool {
    use crate::ui::theme as th;

    let font = egui::FontId::proportional(th::text::BODY_SMALL);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), p.text_primary);
    let pad = egui::vec2(th::space::MD, 6.0);
    let (rect, response) =
        ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::click());

    let hovered = response.hovered();
    // 与全局控件圆角一致（theme.rs 的 `v.widgets.*.corner_radius` = 6）。
    let radius = egui::CornerRadius::same(6);
    let fill = if hovered {
        p.glass_fill_button_hover
    } else {
        p.glass_fill_button
    };
    // 静止态描边用 `control_stroke`：这是顶栏上的操作按钮，
    // 浅色主题下必须能看出它是可点的（`glass_stroke` 会淡到不可见）。
    let stroke = if hovered {
        egui::Stroke::new(1.0, p.accent)
    } else {
        egui::Stroke::new(1.0, p.control_stroke)
    };
    let text_color = if hovered { p.accent } else { p.text_secondary };

    ui.painter().rect_filled(rect, radius, fill);
    ui.painter()
        .rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
    ui.painter()
        .text(rect.center(), egui::Align2::CENTER_CENTER, label, font, text_color);

    response.clicked()
}

/// 应用数据目录下的数据库路径。
fn default_db_path() -> std::path::PathBuf {
    // 优先使用系统标准的应用数据目录；不可用时回退到当前目录。
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(dirs_fallback)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    base.join("LeetCodeCompass").join("compass.db")
}

/// 跨平台的家目录回退方案。
fn dirs_fallback() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

// ---------------------------------------------------------------------------
// 全局视觉样式（已迁移）
// ---------------------------------------------------------------------------
//
// 原先此处有一个 `configure_style(ctx)` 函数，只设间距与字号、不涉及颜色，
// 是主题化无法落地的直接原因。
//
// 现已迁移至 `crate::ui::theme::install`——那里同时处理明暗两套 `Visuals`、
// 语义色板、字号层级与间距栅格。
//
// ## 关键机制（勿改）
//
// egui 0.36 起样式**按主题存储**（`all_styles_mut` 同时作用于明暗两套），
// 没有 `Context::style()` / `set_style()`。切换主题走
// `ctx.set_theme(Theme::Dark | Theme::Light)`——这一步会让**所有框架级
// 组件**（面板、按钮、输入框、滚动条、工具提示、文本选区、光标）
// 统一换套，是"主题覆盖全部组件"的实现基础。

// ---------------------------------------------------------------------------
// eframe 集成
// ---------------------------------------------------------------------------

impl eframe::App for CompassApp {
    /// egui 0.36 的 `App` trait 入口是 `ui()`，不再是 `update()`。
    ///
    /// 传入的 `Ui` 位于根视口，没有内边距与背景色；面板需要显式创建。
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 保持上下文最新，供后续派发的后台任务唤醒 UI。
        let ctx = ui.ctx().clone();
        self.last_ctx = Some(ctx.clone());

        // 0. 同步主题。
        //
        // 必须在绘制之前：主题变化后本帧就要用新样式渲染，否则会出现
        // "点了一下没反应、下一帧才变"的观感。
        self.sync_theme(&ctx);

        // 1. 消费后台消息。必须在渲染之前，这样本帧就能显示新数据。
        self.drain_messages(&ctx);

        // 2. 处理提示自动消失。
        self.tick_toast();

        // 3. 首次渲染时初始化设置草稿。
        if !self.settings.is_initialized() {
            if let Ok(cfg) = self.config.try_read() {
                self.settings.sync_from(&cfg);
            }
        }

        // 4. 绘制界面。顺序重要：先侧边栏与顶栏，再内容区，最后浮动提示。
        self.draw_sidebar(ui);
        self.draw_top_bar(ui);
        self.draw_body(ui);
        self.draw_toast(&ctx);
    }
}

impl CompassApp {
    /// 把当前视觉状态同步到 egui 全局样式。
    ///
    /// **只在主题或玻璃开关变化时重装 `Visuals`**：每帧无条件重写会触发
    /// `Style` 的 `Arc::make_mut` 深拷贝，并让 egui 的样式缓存全部失效。
    /// 而 `set_theme` 调用本身是廉价的（改一个枚举），可以每帧调。
    fn sync_theme(&mut self, ctx: &Context) {
        let key = (self.ui_theme.mode, self.ui_theme.glass);
        if self.applied_theme != Some(key) {
            crate::ui::theme::install(ctx, &self.ui_theme);
            self.applied_theme = Some(key);
        }
        crate::ui::theme::set_theme(ctx, self.ui_theme.mode);
    }

    /// 切换主题（单按钮）。
    ///
    /// 立刻生效并请求重绘，无需等"保存设置"——这是需求明确要求的"即时生效"。
    /// 落盘发生在用户点「保存设置」时（见 `save_settings`）。
    pub fn toggle_theme(&mut self) {
        self.ui_theme = self.ui_theme.toggled_mode();
        if let Some(ctx) = self.last_ctx.clone() {
            ctx.request_repaint();
        }
    }

    /// 设置玻璃质感开关。
    pub fn set_glass_effect(&mut self, enabled: bool) {
        self.ui_theme = self.ui_theme.with_glass(enabled);
        if let Some(ctx) = self.last_ctx.clone() {
            ctx.request_repaint();
        }
    }

    /// 当前色板的只读副本（Copy 语义，取完即脱离 `self` 借用）。
    pub fn palette(&self) -> crate::ui::theme::Palette {
        self.ui_theme.palette
    }

    /// 左侧导航栏。
    ///
    /// 从顶部横向标签迁移而来。动机有二：
    /// 1. 用户要求"边栏采用高级玻璃质感"——侧边栏是玻璃材质最自然的载体
    ///    （大面积连续色块，半透明与高光效果最明显），而改造前项目**没有**
    ///    侧边栏，该需求无处落地；
    /// 2. 顶栏原先同时承载标题、5 个页面切换、账号状态、刷新按钮与同步进度，
    ///    信息密度过高。拆分后顶栏降级为"上下文栏"，层次更清晰。
    fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette();
        // 224 而非 216：字标 "LeetCode Compass"（18px）加 34px 磁贴后，
        // 216 宽度下右缘只剩 2~3px 余量（实测截图），换个 DPI 就可能折行。
        // 224 留出 ~10px 呼吸空间，导航项（最长"今日推荐"）不受影响。
        const SIDEBAR_WIDTH: f32 = 224.0;

        egui::Panel::left("sidebar")
            .resizable(false)
            // `Panel` 用 `exact_size` 约束唯一的自由维度——侧边栏只有宽度可变，
            // 因此这一个调用即等价于"固定宽度"。
            .exact_size(SIDEBAR_WIDTH)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let outer = ui.max_rect();
                // 玻璃底由本函数自绘而非走 `Panel::frame`：`Frame` 无法表达
                // "顶部高光线"这一层，而它是玻璃质感的关键。
                //
                // 这里不复用 `widgets::glass_bar`，因为 `Panel::show` 传入的
                // 闭包已经占用了内容区的 `Ui`，再嵌一层 `Frame` 会把内容
                // 向内挤压 1px（描边占位），导致侧边栏宽度与设计值不符。
                // 故只复用色板与高光绘制函数，底与描边自绘。
                ui.painter().rect_filled(outer, 0.0, p.glass_fill_sidebar);
                // 右侧分隔线。
                ui.painter().line_segment(
                    [
                        egui::pos2(outer.right() - 0.5, outer.top()),
                        egui::pos2(outer.right() - 0.5, outer.bottom()),
                    ],
                    egui::Stroke::new(1.0, p.glass_stroke),
                );
                // 顶部高光。
                crate::ui::theme::paint_glass_highlight(ui, outer, 0.0, &p);

                ui.add_space(crate::ui::theme::space::LG);

                // ---- 品牌标识 ----
                //
                // 形态：34px 应用图标磁贴 + 完整品牌名 "LeetCode Compass"，
                // 具体设计理由见 `widgets::brand_mark`。
                //
                // 左内边距由外层 `horizontal` 的 `add_space` 提供，
                // 因此这里把 `item_spacing.x` 归零——否则 `horizontal` 默认的
                // 12px 间距会叠加在 18px 之上，品牌会比下方的导航项多缩进 12px。
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(crate::ui::theme::space::LG);
                    crate::ui::widgets::brand_mark(ui, &p, "LeetCode Compass");
                });

                ui.add_space(crate::ui::theme::space::LG);
                // 品牌与导航之间的分隔线用 `divider` 而非 `glass_stroke`：
                // 这一条同时承担"品牌区结束"的结构含义，需要比容器轮廓更明确。
                nav_separator(ui, p.divider);
                ui.add_space(crate::ui::theme::space::SM);

                // ---- 导航项 ----
                for page in Page::all() {
                    let selected = self.page == page;
                    let icon = page_icon(page);
                    if nav_item(ui, &p, page.label_zh(), icon, selected) {
                        self.page = page;
                        // 首次进入设置页时同步草稿。
                        if page == Page::Settings && !self.settings.is_initialized() {
                            if let Ok(cfg) = self.config.try_read() {
                                self.settings.sync_from(&cfg);
                            }
                        }
                    }
                }

                // ---- 底部账号状态 ----
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(crate::ui::theme::space::MD);
                    let (username, authenticated) = self
                        .config
                        .try_read()
                        .map(|c| (c.username.clone(), c.is_authenticated()))
                        .unwrap_or_default();

                    ui.horizontal(|ui| {
                        ui.add_space(crate::ui::theme::space::LG);
                        let (dot, text, color) = if authenticated {
                            let color = p.success;
                            let text = if self.verified.is_some() {
                                format!("{username}（已校验）")
                            } else {
                                format!("{username}（未校验）")
                            };
                            ("●", text, color)
                        } else if !username.is_empty() {
                            ("○", format!("{username}（仅公开）"), p.text_tertiary)
                        } else {
                            ("○", "未绑定账号".to_string(), p.text_tertiary)
                        };
                        ui.label(
                            egui::RichText::new(dot)
                                .size(crate::ui::theme::text::CAPTION)
                                .color(color),
                        );
                        ui.label(
                            egui::RichText::new(text)
                                .size(crate::ui::theme::text::CAPTION)
                                .color(if authenticated {
                                    p.text_secondary
                                } else {
                                    p.text_tertiary
                                }),
                        );
                    });
                    ui.add_space(crate::ui::theme::space::SM);
                    ui.separator();
                    ui.add_space(crate::ui::theme::space::XS);
                });
            });
    }

    /// 顶部上下文栏。
    ///
    /// 内容：当前页标题 + 账号状态 + 刷新 + 主题切换。
    /// 页面切换已迁至侧边栏，使顶栏专注于"当前在哪、能做什么"。
    fn draw_top_bar(&mut self, ui: &mut egui::Ui) {
        let p = self.palette();
        const BAR_HEIGHT: f32 = 52.0;

        egui::Panel::top("top_bar")
            .resizable(false)
            // 顶栏只有高度可变，`exact_size` 即固定高度。
            .exact_size(BAR_HEIGHT)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let outer = ui.max_rect();
                ui.painter().rect_filled(outer, 0.0, p.glass_fill_bar);
                ui.painter().line_segment(
                    [
                        egui::pos2(outer.left(), outer.bottom() - 0.5),
                        egui::pos2(outer.right(), outer.bottom() - 0.5),
                    ],
                    egui::Stroke::new(1.0, p.glass_stroke),
                );
                crate::ui::theme::paint_glass_highlight(ui, outer, 0.0, &p);

                // 垂直居中内容。
                ui.add_space((BAR_HEIGHT - 24.0) * 0.5);
                ui.horizontal(|ui| {
                    ui.add_space(crate::ui::theme::space::XL);

                    // 当前页标题。
                    ui.label(
                        egui::RichText::new(self.page.label_zh())
                            .size(crate::ui::theme::text::HEADING)
                            .strong()
                            .color(p.text_primary),
                    );

                    // 右侧操作区。
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(crate::ui::theme::space::XL);

                        // ---- 主题切换按钮（单个按钮）----
                        //
                        // 图标语义：显示"点击后会变成什么"——
                        // 浅色主题显月亮（点它转深色），深色主题显太阳（点它转浅色）。
                        let icon = self.ui_theme.mode.toggle_icon();
                        let hint = self.ui_theme.mode.toggle_hint();
                        if crate::ui::widgets::glass_icon_button(ui, &p, 34.0, icon, hint)
                            .clicked()
                        {
                            self.toggle_theme();
                        }

                        ui.add_space(crate::ui::theme::space::SM);

                        // ---- 刷新账号 ----
                        if self.refreshing_account {
                            ui.spinner();
                        } else if glass_text_button(ui, &p, "刷新账号") {
                            self.refresh_account();
                        }

                        ui.add_space(crate::ui::theme::space::SM);
                        ui.separator();
                        ui.add_space(crate::ui::theme::space::SM);

                        // ---- 账号状态 ----
                        let (username, authenticated) = self
                            .config
                            .try_read()
                            .map(|c| (c.username.clone(), c.is_authenticated()))
                            .unwrap_or_default();

                        let (text, color) = if authenticated {
                            let verified = self.verified.is_some();
                            let text = if verified {
                                format!("{username}（凭据已校验）")
                            } else {
                                format!("{username}（凭据未校验）")
                            };
                            (text, p.success)
                        } else if !username.is_empty() {
                            (format!("{username}（仅公开数据）"), p.text_tertiary)
                        } else {
                            ("未绑定账号".to_string(), p.text_tertiary)
                        };
                        ui.label(
                            egui::RichText::new(text)
                                .size(crate::ui::theme::text::CAPTION)
                                .color(color),
                        );
                    });
                });

                // ---- 同步进度（细线形式）----
                //
                // 改造前是一整行 spinner + 文字 + 进度条，在不常发生但会持续
                // 十几秒的同步期间占据顶栏大量空间。改为底部细进度线：
                // 状态可见但不喧宾夺主。
                if self.syncing {
                    let (done, total) = self.sync_progress;
                    let ratio = if total > 0 {
                        (done as f32 / total as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let bar = egui::Rect::from_min_size(
                        egui::pos2(outer.left(), outer.bottom() - 3.0),
                        egui::vec2(outer.width() * ratio, 3.0),
                    );
                    ui.painter().rect_filled(bar, 0.0, p.accent);
                    ui.ctx().request_repaint();
                }
            });
    }

    /// 右下角浮动提示。
    ///
    /// `Area` 挂在 `Context` 上而非某个 `Ui` 上，因此只需要 `ctx`。
    fn draw_toast(&mut self, ctx: &Context) {
        let Some((kind, text, _)) = self.toast.clone() else {
            // 无提示时不重绘。
            return;
        };

        // 提示存在期间保持重绘，否则倒计时会停滞。
        ctx.request_repaint();

        let p = self.palette();
        let (bg, fg, border) = match kind {
            ToastKind::Success => (p.banner_success.bg, p.banner_success.text, p.success),
            ToastKind::Error => (p.banner_error.bg, p.banner_error.text, p.danger),
            ToastKind::Info => (p.banner_info.bg, p.banner_info.text, p.info),
        };

        let mut dismiss = false;

        egui::Area::new(egui::Id::new("toast_area"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(bg)
                    .stroke(egui::Stroke::new(1.0, border))
                    .corner_radius(egui::CornerRadius::same(10))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 14,
                        spread: 0,
                        color: p.glass_shadow,
                    })
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&text)
                                    .color(fg)
                                    .size(crate::ui::theme::text::BODY_SMALL),
                            );
                            ui.add_space(crate::ui::theme::space::SM);
                            if ui
                                .small_button(egui::RichText::new("×").color(fg))
                                .clicked()
                            {
                                dismiss = true;
                            }
                        });
                    });
            });

        // 按钮回调内不能借用 `self`，因此把结果带出来处理。
        if dismiss {
            self.toast = None;
        }
    }

    /// 主内容区。
    fn draw_body(&mut self, ui: &mut egui::Ui) {
        let p = self.palette();
        // 页面外边距：由 `default_margins()` 的紧凑值提升到 XL 档，
        // 让内容与侧边栏/顶栏之间有充分的呼吸空间。
        let margin = crate::ui::theme::space::XL as i8;
        egui::CentralPanel::default_margins()
            .frame(
                egui::Frame::NONE
                    .fill(p.bg_panel)
                    .inner_margin(egui::Margin {
                        left: margin,
                        right: margin,
                        top: crate::ui::theme::space::LG as i8,
                        bottom: margin,
                    }),
            )
            .show(ui, |ui| {
                // 持久性错误优先显示。
                //
                // "重试"按钮仅在错误可重试时出现（网络抖动、限流）。对
                // 用户名错误这类不可重试的问题，给出重试只会让用户白等。
                if let Some((err, retryable)) = self.persistent_error.clone() {
                    ui.horizontal_wrapped(|ui| {
                        crate::ui::widgets::error_banner(ui, &p, &err);
                        if retryable && ui.button("重试").clicked() {
                            self.persistent_error = None;
                            self.maybe_sync_problems(true);
                        }
                        if ui.button("忽略").clicked() {
                            self.persistent_error = None;
                        }
                    });
                    ui.add_space(crate::ui::theme::space::SM);
                }

                match self.page {
                    Page::Home => crate::ui::home::draw(ui, self),
                    Page::Recommend => crate::ui::recommend::draw(ui, self),
                    Page::Contest => crate::ui::contest::draw(ui, self),
                    Page::Assistant => crate::ui::assistant::draw(ui, self),
                    Page::Settings => crate::ui::settings::draw(ui, self),
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // 配置变更导致的缓存失效
    // -----------------------------------------------------------------------

    /// **本组测试对应一个真实缺陷。**
    ///
    /// 会话校验结论与推荐结果改为持久化后，必须能判断"还能不能用"。
    /// 漏判会让界面显示与实际身份不符的状态；误判则让用户白白重做。
    #[test]
    fn unchanged_configuration_invalidates_nothing() {
        let inv = invalidation_for(Some("fp"), Some("fp"), Some("wuhu"), Some("wuhu"));
        assert_eq!(inv, CacheInvalidation::default());
    }

    /// 凭据变化只作废校验结论，不应牵连推荐结果。
    ///
    /// 推荐由题库与账号数据推导，与 Cookie 无关——Cookie 换了但账号
    /// 没换时把推荐一起清掉，是让用户做无谓的重复劳动。
    #[test]
    fn credential_change_invalidates_session_check_only() {
        let inv = invalidation_for(Some("fp1"), Some("fp2"), Some("wuhu"), Some("wuhu"));
        assert!(inv.session_check, "凭据变了，校验结论必须作废");
        assert!(
            !inv.recommendations,
            "仅换 Cookie 不应清掉推荐缓存——推荐与凭据无关"
        );
    }

    /// 账号变化只作废推荐结果，不应牵连校验结论。
    ///
    /// 注意：实践中改账号通常也会改 Cookie（指纹随之变化），但两者是
    /// 独立维度，不能靠"反正会一起变"来偷懒——用户完全可能只改用户名
    /// 而保留 Cookie（例如把昵称改成正确的 slug）。
    #[test]
    fn account_change_invalidates_recommendations_only() {
        let inv = invalidation_for(Some("fp"), Some("fp"), Some("wuhu"), Some("alice"));
        assert!(!inv.session_check);
        assert!(inv.recommendations, "换了账号，旧推荐与新账号无关");
    }

    /// 两者同时变化时都要作废。
    #[test]
    fn both_changes_invalidate_both_caches() {
        let inv = invalidation_for(Some("fp1"), Some("fp2"), Some("wuhu"), Some("alice"));
        assert!(inv.session_check);
        assert!(inv.recommendations);
    }

    /// 读取失败（配置锁被占用）时不得作废任何缓存。
    ///
    /// 这是关键的容错方向：一次瞬时锁竞争若清掉用户的有效数据，
    /// 代价远大于"暂时沿用一份旧缓存"。
    #[test]
    fn unreadable_config_invalidates_nothing() {
        let inv = invalidation_for(None, Some("fp"), None, Some("wuhu"));
        assert_eq!(inv, CacheInvalidation::default());

        let inv = invalidation_for(Some("fp"), None, Some("wuhu"), None);
        assert_eq!(inv, CacheInvalidation::default());
    }

    // -----------------------------------------------------------------------
    // 推荐缓存的归属判定
    // -----------------------------------------------------------------------

    fn identity(slug: &str, name: &str) -> LoginIdentity {
        LoginIdentity {
            slug: slug.into(),
            username: name.into(),
            real_name: None,
        }
    }

    /// **本组测试对应一个真实缺陷的连带影响。**
    ///
    /// 中国站的配置用户名会被 `adopt_login_slug` 自动改写（昵称 → slug）。
    /// 若缓存归属判定只比较配置用户名，这个改写会让推荐缓存凭空失效——
    /// 用户既没换账号也没改数据，却看到推荐被清空。
    #[test]
    fn recommendation_cache_survives_nickname_to_slug_rewrite() {
        let id = identity("wuhu", "梧糊");

        // 缓存是在配置用户名还是昵称「梧糊」时生成的，
        // 而现在配置已被改写为 slug「wuhu」。
        assert!(
            recommendation_cache_applies("梧糊", "wuhu", Some(&id)),
            "昵称→标识的自动改写不应让推荐缓存失效"
        );

        // 反向：缓存是在 slug 时生成，现在配置是昵称。
        assert!(
            recommendation_cache_applies("wuhu", "梧糊", Some(&id)),
            "同一身份的两种写法都应命中"
        );
    }

    /// 精确匹配始终有效，且未校验时退化为精确匹配。
    ///
    /// 未校验就无从得知别名，此时**必须**保守：宁可让用户重生成一次，
    /// 也不能把别人的推荐显示成他的。
    #[test]
    fn recommendation_cache_falls_back_to_exact_match() {
        assert!(recommendation_cache_applies("wuhu", "wuhu", None));
        assert!(!recommendation_cache_applies("梧糊", "wuhu", None));
        assert!(!recommendation_cache_applies("alice", "wuhu", None));
    }

    /// 真正换了账号时必须判为不适用。
    ///
    /// 这是本判定的核心目的：把别人的推荐显示成自己的，比不显示更糟。
    #[test]
    fn recommendation_cache_rejects_different_account() {
        let id = identity("wuhu", "梧糊");
        assert!(!recommendation_cache_applies("alice", "wuhu", Some(&id)));
        assert!(!recommendation_cache_applies("bob", "wuhu", Some(&id)));
    }
}
