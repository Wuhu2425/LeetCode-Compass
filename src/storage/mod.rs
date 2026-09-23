//! 持久化层。
//!
//! [`db`] 提供基于 SQLite 的题库缓存与用户数据存储。所有操作都是同步
//! 阻塞调用，调用方应在 `tokio::task::spawn_blocking` 中执行。
//!
//! 配置的序列化/反序列化辅助函数也放在此处：配置以 JSON 形式存于
//! `settings` 表，密钥与偏好共享同一套存取路径。

pub mod db;

pub use db::Database;

use serde::{Deserialize, Serialize};

/// 引入应用错误类型，供本模块的配置读写函数使用。
use crate::error::AppResult;
/// 配置在 `settings` 表中的键名。
pub const SETTINGS_KEY_CONFIG: &str = "app_config";

/// 会话校验结论在 `settings` 表中的键名。
pub const SETTINGS_KEY_SESSION_CHECK: &str = "session_check";

/// 推荐结果缓存在 `settings` 表中的键名。
pub const SETTINGS_KEY_RECOMMENDATIONS: &str = "recommendations";

/// 从数据库加载配置。
///
/// 三种降级情形都返回默认配置而非错误：
/// - 键不存在（首次运行）
/// - JSON 损坏（版本升级导致结构变更）
/// - 反序列化失败（字段类型不匹配）
///
/// 理由：配置损坏不应让应用无法启动。宁可让用户重新填写一次，
/// 也不要给一个打不开的窗口。调用方可通过返回的 `was_reset` 标志
/// 提示用户"配置已重置"。
pub fn load_config(db: &Database) -> AppResult<(crate::config::AppConfig, bool)> {
    let raw = db.get_setting(SETTINGS_KEY_CONFIG)?;
    let Some(raw) = raw else {
        return Ok((crate::config::AppConfig::default(), false));
    };

    match serde_json::from_str::<crate::config::AppConfig>(&raw) {
        Ok(cfg) => Ok((cfg, false)),
        Err(_) => {
            // 保留损坏的原始值以便排查，但不再使用它。
            let _ = db.set_setting("app_config_corrupted_backup", &raw);
            Ok((crate::config::AppConfig::default(), true))
        }
    }
}

/// 保存配置到数据库。
///
/// 序列化前会调用 `normalize()`，确保落盘的永远是规范化后的值。
pub fn save_config(
    db: &Database,
    config: &crate::config::AppConfig,
) -> AppResult<()> {
    let mut cfg = config.clone();
    cfg.normalize();
    let json = serde_json::to_string(&cfg)
        .map_err(|e| crate::error::AppError::Config(format!("配置序列化失败: {e}")))?;
    db.set_setting(SETTINGS_KEY_CONFIG, &json)
}

// ---------------------------------------------------------------------------
// 会话校验结论的持久化
// ---------------------------------------------------------------------------

/// 已持久化的会话校验结论。
///
/// ## 为什么需要持久化
///
/// 校验结论原先只存于内存，重启即丢失——用户每次打开应用都要重新点一次
/// 「校验凭据」，即便 Cookie 一直有效。校验本身是一次网络往返，把它
/// 变成"每次启动的必做动作"是纯浪费。
///
/// ## 为什么必须带指纹
///
/// 结论只对**校验时的那套凭据**成立。用户换了账号、改了 Cookie 或切了
/// 站点后沿用旧结论，会让界面显示"凭据已校验"，而实际请求用的是另一套
/// 凭据——这类"状态与事实不符"比直接要求重校验更糟。
///
/// 因此连同 [`AppConfig::credential_fingerprint`] 一起存下，启动时比对。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCheck {
    /// 校验通过时服务端认定的身份。
    ///
    /// 含 `slug`——中国站的查询标识与显示昵称可能不同，这里存下它，
    /// 重启后无需再次询问服务端即可继续用正确标识查询。
    pub identity: crate::leetcode::client::LoginIdentity,
    /// 校验时刻（Unix 秒），供界面展示。
    pub checked_at: i64,
    /// 校验时所用凭据的指纹。与当前配置不一致则本结论作废。
    pub fingerprint: String,
}

/// 保存会话校验结论。
pub fn save_session_check(db: &Database, check: &SessionCheck) -> AppResult<()> {
    let json = serde_json::to_string(check)
        .map_err(|e| crate::error::AppError::Config(format!("会话校验结论序列化失败: {e}")))?;
    db.set_setting(SETTINGS_KEY_SESSION_CHECK, &json)
}

/// 读取会话校验结论。
///
/// 这是**缓存**：解析失败一律按 `None`（未校验）处理，最坏结果是让用户
/// 多点一次「校验凭据」，不会造成错误状态。数据库层面的错误仍会上报。
pub fn load_session_check(db: &Database) -> AppResult<Option<SessionCheck>> {
    let Some(raw) = db.get_setting(SETTINGS_KEY_SESSION_CHECK)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&raw).ok())
}

/// 清除会话校验结论（凭据失效或用户主动登出时调用）。
pub fn clear_session_check(db: &Database) -> AppResult<()> {
    db.delete_setting(SETTINGS_KEY_SESSION_CHECK)
}

// ---------------------------------------------------------------------------
// 推荐结果缓存
// ---------------------------------------------------------------------------

/// 已缓存的推荐结果。
///
/// ## 为什么需要缓存
///
/// 推荐结果原先只存于内存，重启后丢失，用户每次打开都要重新点一次
/// 「生成推荐」。但推荐是**由本地数据推导**的——题库、标签统计、画像
/// 都没变时，重算结果必然相同，纯属重复劳动。
///
/// ## 为什么要记录来源
///
/// 缓存必须能判断"还能不能用"。两个维度：
/// - `username`：换了账号，旧结果与新账号无关；
/// - `generated_at`：数据可能已更新，结果会过时。
///
/// 这里不做自动失效（无法预知用户何时刷新了题库），而是把生成时间
/// 交给界面展示——让用户自己判断要不要重算，比程序擅自清空更可控。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRecommendations {
    /// 生成时刻（Unix 秒）。
    pub generated_at: i64,
    /// 生成时绑定的账号。空字符串表示当时未绑定账号。
    pub username: String,
    /// 推荐结果本体。
    pub set: crate::logic::recommend::RecommendationSet,
}

/// 保存推荐结果。
pub fn save_recommendations(db: &Database, rec: &StoredRecommendations) -> AppResult<()> {
    let json = serde_json::to_string(rec)
        .map_err(|e| crate::error::AppError::Config(format!("推荐结果序列化失败: {e}")))?;
    db.set_setting(SETTINGS_KEY_RECOMMENDATIONS, &json)
}

/// 读取缓存的推荐结果。解析失败按"无缓存"处理（见 [`load_session_check`]）。
pub fn load_recommendations(db: &Database) -> AppResult<Option<StoredRecommendations>> {
    let Some(raw) = db.get_setting(SETTINGS_KEY_RECOMMENDATIONS)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&raw).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LeetCodeSession, LlmConfig, LlmProvider};

    #[test]
    fn load_config_returns_default_on_first_run() {
        let db = Database::open_in_memory().unwrap();
        let (cfg, reset) = load_config(&db).unwrap();
        assert!(!reset);
        assert!(!cfg.has_account());
    }

    #[test]
    fn config_roundtrip_preserves_credentials_and_llm_settings() {
        let db = Database::open_in_memory().unwrap();
        let mut cfg = crate::config::AppConfig {
            username: "alice".into(),
            session: LeetCodeSession {
                session_cookie: "sess-value".into(),
                csrf_token: "csrf-value".into(),
                site: crate::config::LeetCodeSite::Cn,
            },
            llm: LlmConfig {
                provider: LlmProvider::Anthropic,
                base_url: "https://api.anthropic.com/v1".into(),
                model: "claude-3-5-sonnet".into(),
                api_key: "sk-ant-test".into(),
                temperature: 0.7,
                timeout_secs: 90,
                max_history_turns: 20,
            },
            page_size: 50,
            theme: crate::ui::theme::ThemeMode::Light,
            glass_effect: false,
        };
        cfg.normalize();
        save_config(&db, &cfg).unwrap();

        let (loaded, reset) = load_config(&db).unwrap();
        assert!(!reset);
        assert_eq!(loaded.username, "alice");
        assert_eq!(loaded.session.session_cookie, "sess-value");
        assert_eq!(loaded.session.csrf_token, "csrf-value");
        assert_eq!(loaded.llm.provider, LlmProvider::Anthropic);
        assert_eq!(loaded.llm.model, "claude-3-5-sonnet");
        assert_eq!(loaded.llm.api_key, "sk-ant-test");
        assert_eq!(loaded.llm.temperature, 0.7);
        assert_eq!(loaded.page_size, 50);
        // 外观设置也必须经过同一条持久化路径往返。
        assert_eq!(loaded.theme, crate::ui::theme::ThemeMode::Light);
        assert!(!loaded.glass_effect);
    }

    /// **本组测试对应一个真实的升级数据场景。**
    ///
    /// 新增 `theme` / `glass_effect` 两个字段时，用户机器上已经存在一份
    /// **不含这两个字段**的配置。若 `load_config` 把"缺字段"判定为
    /// "配置损坏"，它会走降级分支**重置全部配置**——用户名、Cookie、
    /// API Key 一起丢失。这是最不可接受的升级事故。
    ///
    /// 这里直接在存储层写入一段老版本的 JSON（模拟真实磁盘状态），
    /// 断言：读到的新字段取默认值，而原有凭据一字不改。
    #[test]
    fn legacy_config_on_disk_migrates_without_resetting_credentials() {
        let db = Database::open_in_memory().unwrap();

        // 与真实用户机器上的结构一致：只有 4 个键。
        let legacy = r#"{
            "username": "sweet-eulerzsj",
            "session": {
                "session_cookie": "legacy-session-value",
                "csrf_token": "legacy-csrf-value",
                "site": "Cn"
            },
            "llm": {
                "provider": "OpenAiCompatible",
                "base_url": "https://api.openai.com/v1",
                "model": "gpt-4o-mini",
                "api_key": "sk-legacy-key",
                "temperature": 0.2,
                "timeout_secs": 60,
                "max_history_turns": 10
            },
            "page_size": 100
        }"#;
        db.set_setting(SETTINGS_KEY_CONFIG, legacy).unwrap();

        let (cfg, reset) = load_config(&db).unwrap();

        // 关键断言一：不得被判为损坏。`reset == true` 会弹"配置已重置"
        // 提示并把用户的一切设置清空。
        assert!(
            !reset,
            "缺主题字段的老配置不得被判定为损坏——否则会重置用户全部设置"
        );

        // 关键断言二：原有数据一字不改。
        assert_eq!(cfg.username, "sweet-eulerzsj");
        assert_eq!(cfg.session.session_cookie, "legacy-session-value");
        assert_eq!(cfg.session.csrf_token, "legacy-csrf-value");
        assert_eq!(cfg.llm.api_key, "sk-legacy-key");
        assert_eq!(cfg.llm.temperature, 0.2);
        assert_eq!(cfg.page_size, 100);

        // 关键断言三：新字段取默认值（深色主题 + 启用玻璃）。
        assert_eq!(
            cfg.theme,
            crate::ui::theme::ThemeMode::Dark,
            "老配置升级后应默认深色主题"
        );
        assert!(cfg.glass_effect, "老配置升级后应默认启用玻璃质感");

        // 关键断言四：回存之后，新字段与老数据共存于同一份 JSON。
        save_config(&db, &cfg).unwrap();
        let (reloaded, reset2) = load_config(&db).unwrap();
        assert!(!reset2);
        assert_eq!(reloaded.username, "sweet-eulerzsj");
        assert_eq!(reloaded.session.session_cookie, "legacy-session-value");
        assert_eq!(reloaded.theme, crate::ui::theme::ThemeMode::Dark);
        assert!(reloaded.glass_effect);
    }

    #[test]
    fn save_config_normalizes_before_persisting() {
        let db = Database::open_in_memory().unwrap();
        let cfg = crate::config::AppConfig {
            username: "  bob  ".into(),
            llm: LlmConfig {
                base_url: " https://api.openai.com/v1/ ".into(),
                ..Default::default()
            },
            page_size: 0,
            ..Default::default()
        };
        save_config(&db, &cfg).unwrap();

        let (loaded, _) = load_config(&db).unwrap();
        assert_eq!(loaded.username, "bob", "落盘值应已去除空白");
        assert_eq!(loaded.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(loaded.page_size, 100, "非法分页值应被规范化");
    }

    #[test]
    fn corrupted_config_falls_back_to_default_and_flags_reset() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting(SETTINGS_KEY_CONFIG, "{ this is not valid json")
            .unwrap();

        let (cfg, reset) = load_config(&db).unwrap();
        assert!(reset, "应标记配置已重置");
        assert!(!cfg.has_account());
        assert_eq!(cfg.page_size, 100, "应回退到默认值");

        // 原始损坏内容应被备份，便于排查。
        assert!(db
            .get_setting("app_config_corrupted_backup")
            .unwrap()
            .is_some());
    }

    #[test]
    fn config_with_wrong_types_falls_back_to_default() {
        let db = Database::open_in_memory().unwrap();
        // 合法 JSON 但字段类型不符（page_size 是字符串）。
        db.set_setting(SETTINGS_KEY_CONFIG, r#"{"username":123,"page_size":"many"}"#)
            .unwrap();
        let (cfg, reset) = load_config(&db).unwrap();
        assert!(reset);
        assert!(!cfg.has_account());
    }

    // -----------------------------------------------------------------------
    // 会话校验结论持久化
    // -----------------------------------------------------------------------

    fn sample_identity() -> crate::leetcode::client::LoginIdentity {
        crate::leetcode::client::LoginIdentity {
            slug: "wuhu".into(),
            username: "梧糊".into(),
            real_name: Some("吴湖".into()),
        }
    }

    /// **本组测试对应一个真实缺陷。**
    ///
    /// 校验结论原先只存于内存，重启即丢失，用户每次打开应用都要重新点
    /// 一次「校验凭据」——即便 Cookie 一直有效。这里锁定"能存能取"，
    /// 且**中文昵称与 ASCII slug 都要完整保留**：slug 是中国站的查询
    /// 标识，丢了就得重新向服务端解析一遍。
    #[test]
    fn session_check_roundtrip_preserves_slug_and_display_name() {
        let db = Database::open_in_memory().unwrap();
        let check = SessionCheck {
            identity: sample_identity(),
            checked_at: 1_700_000_000,
            fingerprint: "abc123".into(),
        };
        save_session_check(&db, &check).unwrap();

        let loaded = load_session_check(&db).unwrap().expect("应能读回");
        assert_eq!(loaded.identity.slug, "wuhu");
        assert_eq!(loaded.identity.username, "梧糊");
        assert_eq!(loaded.identity.real_name.as_deref(), Some("吴湖"));
        assert_eq!(loaded.checked_at, 1_700_000_000);
        assert_eq!(loaded.fingerprint, "abc123");
    }

    /// 首次运行（从未校验过）必须返回 `None` 而不是报错。
    #[test]
    fn session_check_absent_returns_none() {
        let db = Database::open_in_memory().unwrap();
        assert!(load_session_check(&db).unwrap().is_none());
    }

    /// 缓存损坏时按"未校验"处理，不得让应用启动失败。
    ///
    /// 这是缓存而非权威数据：最坏结果是让用户多点一次「校验凭据」。
    /// 若为此返回错误并中断启动，代价与收益完全不成比例。
    #[test]
    fn corrupted_session_check_degrades_to_none() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting(SETTINGS_KEY_SESSION_CHECK, "{ not json")
            .unwrap();
        assert!(load_session_check(&db).unwrap().is_none());
    }

    /// 凭据失效后必须能清除结论，否则界面会一直显示"已校验"。
    #[test]
    fn clear_session_check_removes_persisted_conclusion() {
        let db = Database::open_in_memory().unwrap();
        save_session_check(
            &db,
            &SessionCheck {
                identity: sample_identity(),
                checked_at: 1,
                fingerprint: "f".into(),
            },
        )
        .unwrap();
        assert!(load_session_check(&db).unwrap().is_some());

        clear_session_check(&db).unwrap();
        assert!(load_session_check(&db).unwrap().is_none());
    }

    // -----------------------------------------------------------------------
    // 推荐结果缓存
    // -----------------------------------------------------------------------

    fn sample_recommendations() -> StoredRecommendations {
        use crate::logic::recommend::{RecommendMode, RecommendationSet};

        StoredRecommendations {
            generated_at: 1_700_000_000,
            username: "wuhu".into(),
            set: RecommendationSet {
                items: Vec::new(),
                mode: RecommendMode::ProgressAware,
                weak_tags: Vec::new(),
                note: Some("测试用".into()),
            },
        }
    }

    /// **本组测试对应一个真实缺陷。**
    ///
    /// 推荐结果原先只存于内存，重启后丢失，用户每次打开都要重新生成。
    /// 这里锁定生成时间、所属账号与模式都能完整往返——三者缺一不可：
    /// 没有账号就无从判断缓存是否还属于当前用户，没有时间就无从判断
    /// 新鲜度，没有模式则界面会显示错误的模式说明条。
    #[test]
    fn recommendations_roundtrip_preserves_metadata() {
        let db = Database::open_in_memory().unwrap();
        save_recommendations(&db, &sample_recommendations()).unwrap();

        let loaded = load_recommendations(&db).unwrap().expect("应能读回");
        assert_eq!(loaded.generated_at, 1_700_000_000);
        assert_eq!(loaded.username, "wuhu");
        assert_eq!(
            loaded.set.mode,
            crate::logic::recommend::RecommendMode::ProgressAware
        );
        assert_eq!(loaded.set.note.as_deref(), Some("测试用"));
    }

    /// 首次运行无缓存，返回 `None`。
    #[test]
    fn recommendations_absent_returns_none() {
        let db = Database::open_in_memory().unwrap();
        assert!(load_recommendations(&db).unwrap().is_none());
    }

    /// 缓存损坏时按"无缓存"处理（同 [`corrupted_session_check_degrades_to_none`]）。
    #[test]
    fn corrupted_recommendations_degrade_to_none() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting(SETTINGS_KEY_RECOMMENDATIONS, "[[[").unwrap();
        assert!(load_recommendations(&db).unwrap().is_none());
    }

    /// 重新生成应覆盖旧缓存，而不是堆积多份。
    #[test]
    fn saving_recommendations_replaces_previous() {
        let db = Database::open_in_memory().unwrap();
        save_recommendations(&db, &sample_recommendations()).unwrap();

        let mut newer = sample_recommendations();
        newer.generated_at = 1_800_000_000;
        newer.username = "alice".into();
        save_recommendations(&db, &newer).unwrap();

        let loaded = load_recommendations(&db).unwrap().unwrap();
        assert_eq!(loaded.generated_at, 1_800_000_000);
        assert_eq!(loaded.username, "alice", "应读到最新一份");
    }
}
