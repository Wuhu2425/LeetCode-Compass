//! 统一错误类型定义。
//!
//! 应用层使用 `anyhow::Result` 获得上下文链，库层/边界层使用本模块的
//! `AppError` 保留可分类处理的错误语义（例如 UI 需要区分"鉴权失败"与
//! "网络超时"以给出不同提示）。

use thiserror::Error;

/// 应用级错误类型。
#[derive(Debug, Error)]
pub enum AppError {
    /// 网络传输层失败（DNS、连接、TLS、超时）。
    #[error("网络请求失败: {0}")]
    Network(#[from] reqwest::Error),

    /// GraphQL 响应无法解析为预期的 DTO 结构。
    ///
    /// 通常意味着 LeetCode 调整了 schema。携带原始响应片段以便排查。
    #[error("响应解析失败: {message}")]
    Parse {
        message: String,
        /// 原始响应片段（截断后），仅用于诊断。
        raw_snippet: Option<String>,
    },

    /// GraphQL 响应中包含 `errors` 字段。
    #[error("LeetCode 返回错误: {0}")]
    GraphQl(String),

    /// 缺少有效的登录凭据，或凭据已失效。
    #[error("登录凭据无效或缺失: {0}")]
    Unauthorized(String),

    /// 请求的资源不存在（例如用户名拼写错误）。
    #[error("未找到资源: {0}")]
    NotFound(String),

    /// 触发服务端限流。
    #[error("请求过于频繁，请稍后再试")]
    RateLimited,

    /// 数据库操作失败。
    #[error("数据库错误: {0}")]
    Database(#[from] rusqlite::Error),

    /// 配置文件读写失败。
    #[error("配置读写失败: {0}")]
    Config(String),

    /// 大模型接口返回错误。
    #[error("大模型接口错误: {0}")]
    Llm(String),

    /// 输入参数不合法。
    #[error("参数不合法: {0}")]
    InvalidInput(String),

    /// 该功能在用户选定的站点上不提供。
    ///
    /// **这刻意不是一个"错误"**，而是能力边界的事实陈述。国际站与中国站的
    /// GraphQL schema 不同，部分功能（标签统计、提交日历、竞赛复盘、
    /// 最近提交）在中国站没有对应的查询接口。
    ///
    /// 之所以单列一个变体而非复用 `Other`/`GraphQl`：UI 层需要把它渲染为
    /// 信息性提示（"此功能暂不支持中国站"）而不是错误告警。若混入通用错误，
    /// 用户会误以为是自己配置有误或程序有 bug，进而做无效的排查。
    #[error("{feature} 暂不支持{site}")]
    UnsupportedOnSite {
        /// 功能名，例如"竞赛复盘"。
        feature: String,
        /// 站点名，例如"中国站 leetcode.cn"。
        site: String,
    },

    /// 兜底：其他未分类错误。
    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// 构造解析错误，自动截断原始响应片段。
    pub fn parse(message: impl Into<String>, raw: Option<&str>) -> Self {
        Self::Parse {
            message: message.into(),
            raw_snippet: raw.map(|s| s.chars().take(500).collect()),
        }
    }

    /// 构造"该功能在当前站点不可用"错误。
    pub fn unsupported_on_site(feature: impl Into<String>, site: impl Into<String>) -> Self {
        Self::UnsupportedOnSite {
            feature: feature.into(),
            site: site.into(),
        }
    }

    /// 供 UI 层判断是否需要引导用户重新配置凭据。
    pub fn is_auth_related(&self) -> bool {
        matches!(self, Self::Unauthorized(_))
    }

    /// 供 UI 层判断是否适合自动重试。
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Network(_) | Self::RateLimited)
    }

    /// 是否为"站点能力边界"而非真正的故障。
    ///
    /// UI 据此决定用信息提示还是错误告警展示。
    pub fn is_unsupported_on_site(&self) -> bool {
        matches!(self, Self::UnsupportedOnSite { .. })
    }
}

/// 应用统一返回类型。
pub type AppResult<T> = Result<T, AppError>;

/// 安装 rustls 的进程级加密后端。
///
/// ## 为什么必须显式安装
///
/// 本项目为规避 `aws-lc-sys` 的编译依赖（CMake / NASM / MSVC），
/// 在 `Cargo.toml` 中对 reqwest 使用了 `rustls-no-provider` feature。
/// 该 feature 的含义是"不要替我选加密后端"——于是 reqwest 在构造
/// `Client` 时会去读取进程级的 rustls 默认 provider，若无人安装就
/// **直接 panic**：
///
/// ```text
/// No rustls crypto provider is configured. When using the
/// `rustls-no-provider` feature you must install a crypto provider
/// before building a Client.
/// ```
///
/// 这个 panic 发生在第一次发 HTTP 请求时，即用户点击"刷新题库"的瞬间——
/// 从用户视角看是"点了按钮程序就崩了"，排查成本很高。因此必须在
/// `main()` 的最开始就装好。
///
/// ## 幂等性
///
/// `install_default()` 在已有 provider 时返回 `Err`。测试二进制与
/// 主程序可能各自调用一次，因此这里忽略该错误——只要最终存在
/// provider 即可，重复安装并不是问题。
pub fn install_crypto_provider() {
    // 忽略返回值：可能已被安装（例如测试并行运行时）。
    let _ = rustls::crypto::ring::default_provider().install_default();
}
