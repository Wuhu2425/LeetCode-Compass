//! SQLite 持久化层。
//!
//! ## 为什么需要本地数据库
//!
//! 两个刚性理由：
//! 1. **题库缓存**。全量题库约 4000 题、每页 100 题即需 40+ 次 HTTP 请求。
//!    每次都重新拉取既慢又容易触发风控（参见 plan.md 风险 R6）。
//! 2. **凭据与设置持久化**。用户的会话凭据与 API Key 需要跨会话保存。
//!
//! ## 并发模型
//!
//! `rusqlite::Connection` 不是 `Sync`。本模块用 `Mutex` 串行化访问，
//! 对于"读多写少"的桌面应用场景足够。所有方法都是同步的，调用方需在
//! `tokio::task::spawn_blocking` 中执行，避免阻塞异步运行时。
//!
//! ## 表结构设计
//!
//! - `problems`：题库缓存。主键 `title_slug`（远端唯一且稳定；题号可能变）。
//! - `tag_stats`：用户标签统计。
//! - `contest_records`：竞赛历史。
//! - `settings`：键值对，存配置 JSON。
//! - `meta`：记录缓存时间等元信息。
//!
//! 缓存的时效：题库 24 小时；用户数据每次会话刷新（因为变化频繁）。
//!
//! ## 关于少量暂未调用的方法
//!
//! `update_problem_status`（单题）、`delete_setting`、`clear_problems`、
//! `path` 四个方法当前未被调用。保留它们是为了让本层的 CRUD 闭环完整：
//! 有 `upsert` 就该有单条 `update`，有 `set_setting` 就该有 `delete`。
//! 刻意删除会让下次需要时又要重写一遍，而它们各自都有测试覆盖。
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, AppResult};
use crate::models::{
    ChatMessage, ChatRole, ContestRecord, Difficulty, Problem, SolveStatus, TagProgress,
    TopicTag,
};

/// 当前数据库 schema 版本。递增时需在 [`Database::migrate`] 中补充迁移逻辑。
const SCHEMA_VERSION: i64 = 2;

/// 题库缓存有效期（秒）。24 小时。
pub const PROBLEM_CACHE_TTL_SECS: i64 = 24 * 60 * 60;

/// 数据库句柄。
///
/// 内部使用 `Mutex<Connection>` 保证线程安全。`Database` 本身实现了
/// `Send + Sync`，可放入 `Arc` 后跨线程共享。
pub struct Database {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 不输出连接内部状态；只暴露路径便于排障。
        f.debug_struct("Database").field("path", &self.path).finish()
    }
}

impl Database {
    /// 打开（或创建）数据库。
    ///
    /// `path` 通常是应用数据目录下的 `compass.db`。为 `None` 时使用
    /// 仅存在于内存的数据库（测试用）。
    pub fn open(path: impl AsRef<Path>) -> AppResult<Self> {
        let path = path.as_ref().to_path_buf();

        // 确保父目录存在，否则 SQLite 会因路径不存在而失败。
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::Config(format!("无法创建数据目录 {}: {e}", parent.display()))
            })?;
        }

        let conn = Connection::open(&path)?;
        Self::configure(&conn)?;
        let db = Self {
            conn: Mutex::new(conn),
            path,
        };
        db.migrate()?;
        Ok(db)
    }

    /// 创建内存数据库。仅用于测试。
    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn)?;
        let db = Self {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        };
        db.migrate()?;
        Ok(db)
    }

    /// 应用连接级 PRAGMA 设置。
    fn configure(conn: &Connection) -> AppResult<()> {
        // WAL 模式下读写不互相阻塞，适合"UI 读取 + 后台写入"的模式。
        // 内存数据库不支持 WAL，故失败时忽略。
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        // 外键约束（虽然当前无外键，开启以防未来遗漏）。
        let _ = conn.pragma_update(None, "foreign_keys", "ON");
        // NORMAL 同步级别在 WAL 下已足够安全，且写入快得多。
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        // 忙等超时：避免并发写入时立即返回 SQLITE_BUSY。
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(())
    }

    /// 获取连接锁。
    ///
    /// `Mutex` 中毒意味着此前有线程在持有锁时 panic。此时数据可能处于
    /// 不一致状态，但直接 panic 会让整个应用崩溃。这里选择恢复内部值
    /// 并继续运行——对本地缓存类数据而言这比崩溃更可取。
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        match self.conn.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// 建表与版本迁移。
    fn migrate(&self) -> AppResult<()> {
        let conn = self.conn();
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

        if current < 1 {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS problems (
                    title_slug      TEXT PRIMARY KEY,
                    question_id     TEXT NOT NULL,
                    frontend_id     TEXT NOT NULL,
                    title           TEXT NOT NULL,
                    difficulty      TEXT NOT NULL,
                    ac_rate         REAL NOT NULL DEFAULT 0,
                    is_paid_only    INTEGER NOT NULL DEFAULT 0,
                    status          TEXT NOT NULL DEFAULT 'Unknown',
                    tags            TEXT NOT NULL DEFAULT '[]'
                );

                -- 排序与筛选频繁用到这三列，建立索引。
                CREATE INDEX IF NOT EXISTS idx_problems_frontend
                    ON problems(frontend_id);
                CREATE INDEX IF NOT EXISTS idx_problems_difficulty
                    ON problems(difficulty);

                CREATE TABLE IF NOT EXISTS tag_stats (
                    tag_slug    TEXT PRIMARY KEY,
                    tag_name    TEXT NOT NULL,
                    solved      INTEGER NOT NULL DEFAULT 0,
                    total       INTEGER NOT NULL DEFAULT 0,
                    username    TEXT NOT NULL DEFAULT ''
                );

                CREATE TABLE IF NOT EXISTS contest_records (
                    start_time          INTEGER NOT NULL,
                    title               TEXT NOT NULL,
                    rating              REAL NOT NULL,
                    ranking             INTEGER NOT NULL DEFAULT 0,
                    total_participants  INTEGER NOT NULL DEFAULT 0,
                    problems_solved     INTEGER NOT NULL DEFAULT 0,
                    total_problems      INTEGER NOT NULL DEFAULT 0,
                    username            TEXT NOT NULL DEFAULT '',
                    PRIMARY KEY (username, start_time)
                );

                CREATE TABLE IF NOT EXISTS settings (
                    key     TEXT PRIMARY KEY,
                    value   TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS meta (
                    key     TEXT PRIMARY KEY,
                    value   TEXT NOT NULL
                );
                "#,
            )?;
            conn.pragma_update(None, "user_version", 1)?;
        }

        // v2：学习助理的对话历史持久化。追加型表，按 id 单调递增，
        // 读取时取末尾 N 条即为"最近 N 条"。
        if current < 2 {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS chat_messages (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    role        TEXT NOT NULL CHECK (role IN ('User', 'Assistant')),
                    content     TEXT NOT NULL,
                    is_error    INTEGER NOT NULL DEFAULT 0,
                    created_at  INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_chat_messages_id
                    ON chat_messages(id);
                "#,
            )?;
            conn.pragma_update(None, "user_version", 2)?;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // 题库缓存
    // -----------------------------------------------------------------------

    /// 批量写入题库。整体在一个事务内，保证要么全成功要么全失败。
    ///
    /// 采用 `INSERT OR REPLACE`：远端题目可能被编辑（改标题、改难度），
    /// 覆盖式写入能让缓存自动跟随。
    ///
    /// 但有一个**例外**：不覆盖 `status` 的"已解决"信息需谨慎——
    /// 由于写入的是本轮拉取到的状态，直接覆盖是正确的（状态以最新为准）。
    pub fn upsert_problems(&self, problems: &[Problem]) -> AppResult<usize> {
        if problems.is_empty() {
            return Ok(0);
        }

        let mut conn = self.conn();
        let tx = conn.transaction()?;

        let mut written = 0usize;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO problems
                    (title_slug, question_id, frontend_id, title, difficulty,
                     ac_rate, is_paid_only, status, tags)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(title_slug) DO UPDATE SET
                    question_id  = excluded.question_id,
                    frontend_id  = excluded.frontend_id,
                    title        = excluded.title,
                    difficulty   = excluded.difficulty,
                    ac_rate      = excluded.ac_rate,
                    is_paid_only = excluded.is_paid_only,
                    status       = excluded.status,
                    tags         = excluded.tags
                "#,
            )?;

            for p in problems {
                // 标签序列化为 JSON 数组存储。SQLite 无原生数组类型。
                let tags_json = serde_json::to_string(&p.tags)
                    .map_err(|e| AppError::Other(format!("标签序列化失败: {e}")))?;

                stmt.execute(params![
                    p.title_slug,
                    p.question_id,
                    p.frontend_id,
                    p.title,
                    p.difficulty.as_api_str(),
                    p.ac_rate,
                    p.is_paid_only as i32,
                    status_to_db(p.status),
                    tags_json,
                ])?;
                written += 1;
            }
        }

        tx.commit()?;
        Ok(written)
    }

    /// 读取全部题目。按题号自然序返回。
    pub fn load_problems(&self) -> AppResult<Vec<Problem>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT title_slug, question_id, frontend_id, title, difficulty,
                   ac_rate, is_paid_only, status, tags
            FROM problems
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            let tags_json: String = row.get(8)?;
            Ok(ProblemRow {
                title_slug: row.get(0)?,
                question_id: row.get(1)?,
                frontend_id: row.get(2)?,
                title: row.get(3)?,
                difficulty: row.get(4)?,
                ac_rate: row.get(5)?,
                is_paid_only: row.get::<_, i32>(6)? != 0,
                status: row.get(7)?,
                tags_json,
            })
        })?;

        let mut out = Vec::new();
        for r in rows {
            let r = r?;
            // 单条记录损坏（如标签 JSON 异常）时跳过该条而非整体失败，
            // 保证用户仍能看到其余题目。
            if let Some(p) = r.into_problem() {
                out.push(p);
            }
        }

        // 排序在 Rust 侧完成：SQLite 无法正确按"题号数字"排序，
        // 因为题号是字符串（且含非数字题号如"面试题 01.01"）。
        out.sort_by(|a, b| {
            a.frontend_id_numeric()
                .cmp(&b.frontend_id_numeric())
                .then_with(|| a.frontend_id.cmp(&b.frontend_id))
        });

        Ok(out)
    }

    /// 题库缓存中的题目数量。
    pub fn problem_count(&self) -> AppResult<usize> {
        let conn = self.conn();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM problems", [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// 记录题库缓存时间。
    pub fn mark_problems_fetched(&self) -> AppResult<()> {
        self.set_meta("problems_fetched_at", &chrono::Utc::now().timestamp().to_string())
    }

    /// 题库缓存是否仍在有效期内。
    ///
    /// 无缓存记录或时间戳异常时返回 `false`（即需要重新拉取）。
    pub fn is_problem_cache_fresh(&self) -> AppResult<bool> {
        let Some(ts) = self.get_meta("problems_fetched_at")? else {
            return Ok(false);
        };
        let Ok(fetched) = ts.parse::<i64>() else {
            return Ok(false);
        };
        let now = chrono::Utc::now().timestamp();
        // now < fetched 说明系统时钟被回拨，此时视为过期更安全。
        if now < fetched {
            return Ok(false);
        }
        Ok(now - fetched < PROBLEM_CACHE_TTL_SECS)
    }

    /// 更新单题完成状态。
    ///
    /// 用途：认证模式下按题号增量刷新状态时避免整表重写。
    pub fn update_problem_status(&self, title_slug: &str, status: SolveStatus) -> AppResult<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE problems SET status = ?1 WHERE title_slug = ?2",
            params![status_to_db(status), title_slug],
        )?;
        Ok(())
    }

    /// 批量更新完成状态，单个事务。
    pub fn update_problem_statuses(
        &self,
        updates: &[(String, SolveStatus)],
    ) -> AppResult<usize> {
        if updates.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut n = 0;
        {
            let mut stmt =
                tx.prepare("UPDATE problems SET status = ?1 WHERE title_slug = ?2")?;
            for (slug, status) in updates {
                n += stmt.execute(params![status_to_db(*status), slug])?;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    // -----------------------------------------------------------------------
    // 标签统计
    // -----------------------------------------------------------------------

    /// 覆盖写入某用户的标签统计。
    pub fn save_tag_stats(&self, username: &str, stats: &[TagProgress]) -> AppResult<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            tx.execute("DELETE FROM tag_stats WHERE username = ?1", params![username])?;
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO tag_stats (tag_slug, tag_name, solved, total, username)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )?;
            for s in stats {
                stmt.execute(params![s.tag_slug, s.tag_name, s.solved, s.total, username])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 读取某用户的标签统计。
    pub fn load_tag_stats(&self, username: &str) -> AppResult<Vec<TagProgress>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT tag_slug, tag_name, solved, total
            FROM tag_stats WHERE username = ?1
            ORDER BY solved DESC, tag_slug ASC
            "#,
        )?;
        let rows = stmt.query_map(params![username], |row| {
            Ok(TagProgress {
                tag_slug: row.get(0)?,
                tag_name: row.get(1)?,
                solved: row.get::<_, i64>(2)?.max(0) as u32,
                total: row.get::<_, i64>(3)?.max(0) as u32,
            })
        })?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // 竞赛记录
    // -----------------------------------------------------------------------

    /// 覆盖写入某用户的竞赛历史。
    pub fn save_contest_records(
        &self,
        username: &str,
        records: &[ContestRecord],
    ) -> AppResult<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            tx.execute(
                "DELETE FROM contest_records WHERE username = ?1",
                params![username],
            )?;
            let mut stmt = tx.prepare(
                r#"
                INSERT OR REPLACE INTO contest_records
                    (start_time, title, rating, ranking, total_participants,
                     problems_solved, total_problems, username)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
            )?;
            for r in records {
                stmt.execute(params![
                    r.start_time,
                    r.title,
                    r.rating,
                    r.ranking,
                    r.total_participants,
                    r.problems_solved,
                    r.total_problems,
                    username,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 读取某用户的竞赛历史（按时间升序）。
    pub fn load_contest_records(&self, username: &str) -> AppResult<Vec<ContestRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            r#"
            SELECT start_time, title, rating, ranking, total_participants,
                   problems_solved, total_problems
            FROM contest_records WHERE username = ?1
            ORDER BY start_time ASC
            "#,
        )?;
        let rows = stmt.query_map(params![username], |row| {
            Ok(ContestRecord {
                start_time: row.get(0)?,
                title: row.get(1)?,
                rating: row.get(2)?,
                ranking: row.get::<_, i64>(3)?.max(0) as u32,
                total_participants: row.get::<_, i64>(4)?.max(0) as u32,
                problems_solved: row.get::<_, i64>(5)?.max(0) as u32,
                total_problems: row.get::<_, i64>(6)?.max(0) as u32,
                attended: true, // 仅存储实际参赛记录
            })
        })?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // -----------------------------------------------------------------------
    // 通用键值
    // -----------------------------------------------------------------------

    /// 写入设置项。
    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// 读取设置项。
    pub fn get_setting(&self, key: &str) -> AppResult<Option<String>> {
        let conn = self.conn();
        let v = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(v)
    }

    /// 删除设置项。
    pub fn delete_setting(&self, key: &str) -> AppResult<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 学习助理对话历史
    // -----------------------------------------------------------------------

    /// 追加一条助理对话消息。
    ///
    /// 对话历史是**追加型**数据：单条写入，绝不覆盖旧消息。
    /// 写失败不影响 UI（调用方降级为仅内存保留），因此错误只上抛不弹窗。
    pub fn append_chat_message(&self, msg: &ChatMessage) -> AppResult<()> {
        let role = match msg.role {
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
            ChatRole::System => return Ok(()), // System 角色只存在于提示词，不入库
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let conn = self.conn();
        conn.execute(
            "INSERT INTO chat_messages (role, content, is_error, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![role, msg.content, msg.is_error as i64, now],
        )?;
        Ok(())
    }

    /// 读取**最近** `limit` 条对话消息（按时间正序返回）。
    ///
    /// 用内层查询取末尾 N 条再反转：保证加载后消息顺序与发生顺序一致。
    pub fn load_chat_messages(&self, limit: usize) -> AppResult<Vec<ChatMessage>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT role, content, is_error FROM (
                 SELECT id, role, content, is_error
                 FROM chat_messages
                 ORDER BY id DESC
                 LIMIT ?1
             ) ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            let role: String = r.get(0)?;
            let content: String = r.get(1)?;
            let is_error: i64 = r.get(2)?;
            Ok((role, content, is_error))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (role, content, is_error) = row?;
            let role = match role.as_str() {
                "User" => ChatRole::User,
                _ => ChatRole::Assistant,
            };
            out.push(ChatMessage {
                role,
                content,
                is_error: is_error != 0,
            });
        }
        Ok(out)
    }

    /// 清空全部对话历史（用户主动"清空对话"时调用）。
    pub fn clear_chat_messages(&self) -> AppResult<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM chat_messages", [])?;
        Ok(())
    }

    fn set_meta(&self, key: &str, value: &str) -> AppResult<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn get_meta(&self, key: &str) -> AppResult<Option<String>> {
        let conn = self.conn();
        let v = conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        Ok(v)
    }

    /// 清空题库缓存。用户手动"刷新题库"或排查问题时使用。
    pub fn clear_problems(&self) -> AppResult<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM problems", [])?;
        conn.execute("DELETE FROM meta WHERE key = 'problems_fetched_at'", [])?;
        Ok(())
    }

    /// 保存 LeetCode 官方公布的全球题量统计。
    ///
    /// 与本地缓存中的题目数不同：这是服务端的权威口径，用于校验本地
    /// 全量拉取是否完整（例如会员题被过滤时应能察觉）。
    /// 存为 `easy/medium/hard` 三个 meta 键，简单且不必新增表。
    pub fn save_global_counts(&self, counts: &crate::models::DifficultyCounts) -> AppResult<()> {
        self.set_meta("global_easy", &counts.easy.to_string())?;
        self.set_meta("global_medium", &counts.medium.to_string())?;
        self.set_meta("global_hard", &counts.hard.to_string())?;
        Ok(())
    }

    /// 读取全球题量统计。无记录或解析失败时返回 `None`。
    pub fn load_global_counts(&self) -> AppResult<Option<crate::models::DifficultyCounts>> {
        let read = |key: &str| -> AppResult<Option<u32>> {
            Ok(self.get_meta(key)?.and_then(|v| v.parse::<u32>().ok()))
        };

        let (easy, medium, hard) = (read("global_easy")?, read("global_medium")?, read("global_hard")?);

        // 三个值必须同时存在才算有效记录——缺任一说明写入不完整。
        match (easy, medium, hard) {
            (Some(e), Some(m), Some(h)) => Ok(Some(crate::models::DifficultyCounts {
                easy: e,
                medium: m,
                hard: h,
            })),
            _ => Ok(None),
        }
    }

    /// 数据库文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// 数据库中的一行题目记录，作为解析中间态。
struct ProblemRow {
    title_slug: String,
    question_id: String,
    frontend_id: String,
    title: String,
    difficulty: String,
    ac_rate: f64,
    is_paid_only: bool,
    status: String,
    tags_json: String,
}

impl ProblemRow {
    /// 转换为领域模型。关键字段缺失或标签 JSON 损坏时返回 `None`。
    fn into_problem(self) -> Option<Problem> {
        // 难度解析失败时降级为 Easy，与 API 层保持一致的容错策略。
        let difficulty = Difficulty::parse(&self.difficulty).unwrap_or(Difficulty::Easy);
        let tags: Vec<TopicTag> = serde_json::from_str(&self.tags_json).unwrap_or_default();

        Some(Problem {
            question_id: self.question_id,
            frontend_id: self.frontend_id,
            title: self.title,
            title_slug: self.title_slug,
            difficulty,
            ac_rate: self.ac_rate,
            tags,
            status: status_from_db(&self.status),
            is_paid_only: self.is_paid_only,
        })
    }
}

/// `SolveStatus` 到数据库字符串的映射。
///
/// 存储可读字符串而非枚举序号：便于用 SQL 工具直接检查数据，
/// 也避免枚举顺序变化导致历史数据错位。
fn status_to_db(s: SolveStatus) -> &'static str {
    match s {
        SolveStatus::Solved => "Solved",
        SolveStatus::Attempted => "Attempted",
        SolveStatus::Todo => "Todo",
        SolveStatus::Unknown => "Unknown",
    }
}

/// 数据库字符串到 `SolveStatus` 的映射。无法识别时返回 `Unknown`
/// （保守选择：宁可不显示完成，也不要错误地标记为已完成）。
fn status_from_db(s: &str) -> SolveStatus {
    match s {
        "Solved" => SolveStatus::Solved,
        "Attempted" => SolveStatus::Attempted,
        "Todo" => SolveStatus::Todo,
        _ => SolveStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对话历史：追加 → 按 limit 读回 → 清空。锁定的性质：
    /// ① 顺序与发生顺序一致；② limit 取的是**最近** N 条而非最先 N 条；
    /// ③ is_error 与角色正确往返；④ 清空后为空。
    #[test]
    fn chat_history_roundtrip() {
        let db = Database::open_in_memory().unwrap();

        for i in 0..5 {
            db.append_chat_message(&ChatMessage::user(format!("问题 {i}")))
                .unwrap();
            db.append_chat_message(&ChatMessage::assistant(format!("回答 {i}")))
                .unwrap();
        }
        db.append_chat_message(&ChatMessage::error("请求失败：超时"))
            .unwrap();

        // 只取最近 4 条：回答 4、问题 4 之前的部分按时间正序。
        let last = db.load_chat_messages(4).unwrap();
        assert_eq!(last.len(), 4);
        assert_eq!(last[0].content, "回答 3");
        assert_eq!(last[1].content, "问题 4");
        assert_eq!(last[1].role, ChatRole::User);
        assert_eq!(last[2].content, "回答 4");
        assert_eq!(last[2].role, ChatRole::Assistant);
        assert_eq!(last[3].content, "请求失败：超时");
        assert!(last[3].is_error);

        // 全量读取包含首条。
        let all = db.load_chat_messages(100).unwrap();
        assert_eq!(all.len(), 11);
        assert_eq!(all[0].content, "问题 0");

        db.clear_chat_messages().unwrap();
        assert!(db.load_chat_messages(100).unwrap().is_empty());
    }

    fn mk_problem(slug: &str, id: &str, difficulty: Difficulty, status: SolveStatus) -> Problem {
        Problem {
            question_id: id.to_string(),
            frontend_id: id.to_string(),
            title: format!("Title {slug}"),
            title_slug: slug.to_string(),
            difficulty,
            ac_rate: 50.0,
            tags: vec![TopicTag {
                name: "Array".into(),
                slug: "array".into(),
            }],
            status,
            is_paid_only: false,
        }
    }

    #[test]
    fn migrate_creates_schema_and_sets_version() {
        let db = Database::open_in_memory().unwrap();
        let conn = db.conn();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // migrate 可重复调用而不报错（幂等）。
        drop(conn);
        assert!(db.migrate().is_ok());
    }

    #[test]
    fn upsert_and_load_roundtrip_preserves_all_fields() {
        let db = Database::open_in_memory().unwrap();
        let p = Problem {
            question_id: "1".into(),
            frontend_id: "1".into(),
            title: "Two Sum".into(),
            title_slug: "two-sum".into(),
            difficulty: Difficulty::Easy,
            ac_rate: 57.9,
            tags: vec![
                TopicTag {
                    name: "Array".into(),
                    slug: "array".into(),
                },
                TopicTag {
                    name: "Hash Table".into(),
                    slug: "hash-table".into(),
                },
            ],
            status: SolveStatus::Solved,
            is_paid_only: true,
        };
        assert_eq!(db.upsert_problems(std::slice::from_ref(&p)).unwrap(), 1);

        let loaded = db.load_problems().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], p, "往返后必须完全一致");
    }

    #[test]
    fn upsert_is_idempotent_and_updates_existing() {
        let db = Database::open_in_memory().unwrap();
        let mut p = mk_problem("two-sum", "1", Difficulty::Easy, SolveStatus::Unknown);
        db.upsert_problems(&[p.clone()]).unwrap();

        // 同一 slug 再次写入应更新而非插入重复行。
        p.title = "Two Sum (Renamed)".into();
        p.status = SolveStatus::Solved;
        db.upsert_problems(&[p.clone()]).unwrap();

        let loaded = db.load_problems().unwrap();
        assert_eq!(loaded.len(), 1, "不应产生重复行");
        assert_eq!(loaded[0].title, "Two Sum (Renamed)");
        assert_eq!(loaded[0].status, SolveStatus::Solved);
    }

    #[test]
    fn upsert_empty_slice_is_noop() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.upsert_problems(&[]).unwrap(), 0);
        assert_eq!(db.problem_count().unwrap(), 0);
    }

    #[test]
    fn load_problems_sorts_numerically_not_lexicographically() {
        let db = Database::open_in_memory().unwrap();
        // 字符串排序会把 "10" 排在 "2" 之前，必须避免。
        let items = vec![
            mk_problem("c", "100", Difficulty::Easy, SolveStatus::Todo),
            mk_problem("a", "2", Difficulty::Easy, SolveStatus::Todo),
            mk_problem("b", "10", Difficulty::Easy, SolveStatus::Todo),
        ];
        db.upsert_problems(&items).unwrap();

        let loaded = db.load_problems().unwrap();
        let ids: Vec<&str> = loaded.iter().map(|p| p.frontend_id.as_str()).collect();
        assert_eq!(ids, vec!["2", "10", "100"], "应按数值升序而非字典序");
    }

    #[test]
    fn non_numeric_frontend_id_sorts_last() {
        let db = Database::open_in_memory().unwrap();
        let items = vec![
            mk_problem("interview", "面试题 01.01", Difficulty::Easy, SolveStatus::Todo),
            mk_problem("a", "5", Difficulty::Easy, SolveStatus::Todo),
        ];
        db.upsert_problems(&items).unwrap();
        let loaded = db.load_problems().unwrap();
        assert_eq!(loaded[0].frontend_id, "5");
        assert_eq!(loaded[1].frontend_id, "面试题 01.01");
    }

    #[test]
    fn all_solve_statuses_roundtrip_through_db() {
        let db = Database::open_in_memory().unwrap();
        for (i, s) in [
            SolveStatus::Solved,
            SolveStatus::Attempted,
            SolveStatus::Todo,
            SolveStatus::Unknown,
        ]
        .iter()
        .enumerate()
        {
            let slug = format!("p{i}");
            db.upsert_problems(&[mk_problem(&slug, &i.to_string(), Difficulty::Easy, *s)])
                .unwrap();
        }
        let loaded = db.load_problems().unwrap();
        let statuses: Vec<SolveStatus> = loaded.iter().map(|p| p.status).collect();
        assert!(statuses.contains(&SolveStatus::Solved));
        assert!(statuses.contains(&SolveStatus::Attempted));
        assert!(statuses.contains(&SolveStatus::Todo));
        assert!(statuses.contains(&SolveStatus::Unknown));
    }

    #[test]
    fn unknown_status_string_in_db_degrades_to_unknown() {
        // 防御性：数据库中若出现无法识别的状态文本（如手工改过或版本降级），
        // 必须保守降级为 Unknown，绝不能误判为 Solved。
        assert_eq!(status_from_db("Garbage"), SolveStatus::Unknown);
        assert_eq!(status_from_db(""), SolveStatus::Unknown);
        assert_eq!(status_from_db("solved"), SolveStatus::Unknown, "大小写敏感");
    }

    #[test]
    fn problem_count_reflects_writes() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.problem_count().unwrap(), 0);
        db.upsert_problems(&[mk_problem("a", "1", Difficulty::Easy, SolveStatus::Todo)])
            .unwrap();
        assert_eq!(db.problem_count().unwrap(), 1);
    }

    #[test]
    fn cache_freshness_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        // 从未拉取过 -> 不新鲜
        assert!(!db.is_problem_cache_fresh().unwrap());

        db.mark_problems_fetched().unwrap();
        assert!(db.is_problem_cache_fresh().unwrap(), "刚写入应视为新鲜");
    }

    #[test]
    fn cache_freshness_false_for_corrupted_timestamp() {
        let db = Database::open_in_memory().unwrap();
        db.set_meta("problems_fetched_at", "not-a-number").unwrap();
        assert!(!db.is_problem_cache_fresh().unwrap());
    }

    #[test]
    fn cache_freshness_false_when_clock_moved_backwards() {
        let db = Database::open_in_memory().unwrap();
        // 未来的时间戳说明系统时钟被回拨，应视为过期以确保数据正确。
        let future = chrono::Utc::now().timestamp() + 100_000;
        db.set_meta("problems_fetched_at", &future.to_string())
            .unwrap();
        assert!(!db.is_problem_cache_fresh().unwrap());
    }

    #[test]
    fn update_single_problem_status() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_problems(&[mk_problem("two-sum", "1", Difficulty::Easy, SolveStatus::Unknown)])
            .unwrap();
        db.update_problem_status("two-sum", SolveStatus::Solved).unwrap();
        let loaded = db.load_problems().unwrap();
        assert_eq!(loaded[0].status, SolveStatus::Solved);
    }

    #[test]
    fn update_statuses_batch_is_atomic() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_problems(&[
            mk_problem("a", "1", Difficulty::Easy, SolveStatus::Unknown),
            mk_problem("b", "2", Difficulty::Easy, SolveStatus::Unknown),
        ])
        .unwrap();

        let updates = vec![
            ("a".to_string(), SolveStatus::Solved),
            ("b".to_string(), SolveStatus::Attempted),
        ];
        assert_eq!(db.update_problem_statuses(&updates).unwrap(), 2);

        let loaded = db.load_problems().unwrap();
        assert_eq!(loaded[0].status, SolveStatus::Solved);
        assert_eq!(loaded[1].status, SolveStatus::Attempted);
    }

    #[test]
    fn update_statuses_empty_is_noop() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.update_problem_statuses(&[]).unwrap(), 0);
    }

    #[test]
    fn tag_stats_save_replaces_previous_set() {
        let db = Database::open_in_memory().unwrap();
        let v1 = vec![TagProgress {
            tag_name: "Array".into(),
            tag_slug: "array".into(),
            solved: 10,
            total: 100,
        }];
        db.save_tag_stats("alice", &v1).unwrap();

        // 第二次保存应完全替换，而非累加。
        let v2 = vec![TagProgress {
            tag_name: "DP".into(),
            tag_slug: "dynamic-programming".into(),
            solved: 3,
            total: 50,
        }];
        db.save_tag_stats("alice", &v2).unwrap();

        let loaded = db.load_tag_stats("alice").unwrap();
        assert_eq!(loaded.len(), 1, "旧数据应被清除");
        assert_eq!(loaded[0].tag_slug, "dynamic-programming");
    }

    #[test]
    fn tag_stats_are_isolated_per_username() {
        let db = Database::open_in_memory().unwrap();
        db.save_tag_stats(
            "alice",
            &[TagProgress {
                tag_name: "Array".into(),
                tag_slug: "array".into(),
                solved: 10,
                total: 100,
            }],
        )
        .unwrap();
        db.save_tag_stats(
            "bob",
            &[TagProgress {
                tag_name: "Tree".into(),
                tag_slug: "tree".into(),
                solved: 5,
                total: 80,
            }],
        )
        .unwrap();

        assert_eq!(db.load_tag_stats("alice").unwrap()[0].tag_slug, "array");
        assert_eq!(db.load_tag_stats("bob").unwrap()[0].tag_slug, "tree");
        assert!(db.load_tag_stats("carol").unwrap().is_empty());
    }

    #[test]
    fn contest_records_roundtrip_and_replace() {
        let db = Database::open_in_memory().unwrap();
        let recs = vec![
            ContestRecord {
                title: "Weekly 380".into(),
                start_time: 1_700_000_000,
                rating: 1500.0,
                ranking: 500,
                total_participants: 20000,
                problems_solved: 3,
                total_problems: 4,
                attended: true,
            },
            ContestRecord {
                title: "Weekly 381".into(),
                start_time: 1_700_500_000,
                rating: 1550.0,
                ranking: 300,
                total_participants: 21000,
                problems_solved: 4,
                total_problems: 4,
                attended: true,
            },
        ];
        db.save_contest_records("alice", &recs).unwrap();
        let loaded = db.load_contest_records("alice").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].title, "Weekly 380", "应按时间升序");
        assert_eq!(loaded[1].rating, 1550.0);

        // 替换写入。
        db.save_contest_records("alice", &recs[..1]).unwrap();
        assert_eq!(db.load_contest_records("alice").unwrap().len(), 1);
    }

    #[test]
    fn settings_kv_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_setting("theme").unwrap(), None);

        db.set_setting("theme", "light").unwrap();
        assert_eq!(db.get_setting("theme").unwrap().as_deref(), Some("light"));

        // 覆盖写。
        db.set_setting("theme", "dark").unwrap();
        assert_eq!(db.get_setting("theme").unwrap().as_deref(), Some("dark"));

        db.delete_setting("theme").unwrap();
        assert_eq!(db.get_setting("theme").unwrap(), None);
    }

    #[test]
    fn clear_problems_resets_cache_state() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_problems(&[mk_problem("a", "1", Difficulty::Easy, SolveStatus::Todo)])
            .unwrap();
        db.mark_problems_fetched().unwrap();
        assert!(db.is_problem_cache_fresh().unwrap());

        db.clear_problems().unwrap();
        assert_eq!(db.problem_count().unwrap(), 0);
        assert!(
            !db.is_problem_cache_fresh().unwrap(),
            "清空后应重新拉取"
        );
    }

    #[test]
    fn clear_problems_does_not_touch_other_tables() {
        // 刷新题库不应误删用户数据。
        let db = Database::open_in_memory().unwrap();
        db.upsert_problems(&[mk_problem("a", "1", Difficulty::Easy, SolveStatus::Todo)])
            .unwrap();
        db.save_tag_stats(
            "alice",
            &[TagProgress {
                tag_name: "Array".into(),
                tag_slug: "array".into(),
                solved: 1,
                total: 10,
            }],
        )
        .unwrap();
        db.set_setting("username", "alice").unwrap();

        db.clear_problems().unwrap();

        assert_eq!(db.load_tag_stats("alice").unwrap().len(), 1);
        assert_eq!(db.get_setting("username").unwrap().as_deref(), Some("alice"));
    }

    #[test]
    fn corrupt_tags_json_does_not_break_loading() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_problems(&[
            mk_problem("good", "1", Difficulty::Easy, SolveStatus::Todo),
            mk_problem("bad", "2", Difficulty::Easy, SolveStatus::Todo),
        ])
        .unwrap();

        // 直接破坏其中一行的标签数据。
        {
            let conn = db.conn();
            conn.execute(
                "UPDATE problems SET tags = 'not json' WHERE title_slug = 'bad'",
                [],
            )
            .unwrap();
        }

        let loaded = db.load_problems().unwrap();
        assert_eq!(loaded.len(), 2, "损坏标签不应导致整表加载失败");
        let bad = loaded.iter().find(|p| p.title_slug == "bad").unwrap();
        assert!(bad.tags.is_empty(), "无法解析的标签应降级为空列表");
    }

    #[test]
    fn opening_file_database_creates_parent_dirs() {
        let dir = std::env::temp_dir().join(format!(
            "compass_test_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let db_path = dir.join("nested").join("compass.db");
        let db = Database::open(&db_path).unwrap();
        assert!(db_path.exists(), "应自动创建父目录与数据库文件");
        db.upsert_problems(&[mk_problem("a", "1", Difficulty::Easy, SolveStatus::Todo)])
            .unwrap();
        assert_eq!(db.problem_count().unwrap(), 1);
        drop(db);
        // 清理测试产物。
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopening_file_database_preserves_data() {
        let dir = std::env::temp_dir().join(format!(
            "compass_reopen_{}_{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let db_path = dir.join("compass.db");

        {
            let db = Database::open(&db_path).unwrap();
            db.upsert_problems(&[mk_problem("persist", "1", Difficulty::Easy, SolveStatus::Solved)])
                .unwrap();
        }

        let db2 = Database::open(&db_path).unwrap();
        assert_eq!(db2.problem_count().unwrap(), 1, "数据应跨连接持久化");
        let loaded = db2.load_problems().unwrap();
        assert_eq!(loaded[0].status, SolveStatus::Solved);

        drop(db2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
