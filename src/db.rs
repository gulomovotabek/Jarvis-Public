use anyhow::{Context, Result};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Column, Row, SqlitePool,
};
use std::{path::Path, str::FromStr};

#[derive(Debug, Clone)]
pub struct Database {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
pub struct UpsertChatInput {
    pub chat_id: i64,
    pub kind: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct UpsertUserInput {
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: String,
    pub last_name: Option<String>,
    pub is_bot: bool,
    pub seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct StoredMessageInput {
    pub chat_id: i64,
    pub telegram_message_id: i64,
    pub sender_id: Option<i64>,
    pub content: Option<String>,
    pub raw_json: String,
    pub created_at: i64,
    pub ingested_at: i64,
    pub reply_to_message_id: Option<i64>,
    pub message_link: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CandidateMessage {
    pub telegram_message_id: i64,
    pub chat_title: Option<String>,
    pub content: String,
    pub created_at: i64,
    pub message_link: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RecentChatMessage {
    pub sender_id: Option<i64>,
    pub content: String,
    pub created_at: i64,
    pub telegram_message_id: i64,
    pub reply_to_message_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: i64,
    pub chat_id: i64,
    pub user_id: i64,
    pub name: String,
    pub prompt: String,
    pub cron_expr: String,
    pub timezone: String,
    pub next_run_at: i64,
    pub last_run_at: Option<i64>,
    pub enabled: bool,
    pub running: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Database {
    pub async fn connect(database_url: &str) -> Result<Self> {
        ensure_sqlite_parent_exists(database_url)?;
        let options = SqliteConnectOptions::from_str(database_url)
            .context("invalid sqlite DATABASE_URL")?
            .create_if_missing(true)
            // Explicitly set the busy timeout so concurrent writers block in
            // sqlite rather than error out with SQLITE_BUSY.
            .busy_timeout(std::time::Duration::from_secs(5));
        // Explicit pool configuration — sqlx's defaults are vague ("a few
        // connections"). 25 max is comfortable headroom for the batcher +
        // MCP tools without starving the host, and an acquire timeout of 5s
        // surfaces backpressure as a real error instead of silent hangs.
        let pool = SqlitePoolOptions::new()
            .max_connections(25)
            .min_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect_with(options)
            .await
            .context("failed to connect sqlite")?;
        Ok(Self { pool })
    }

    pub async fn init(&self) -> Result<()> {
        // Enable WAL mode for concurrent reads
        sqlx::query("PRAGMA journal_mode=WAL;")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chats (
                chat_id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                title TEXT,
                username TEXT,
                first_seen_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                user_id INTEGER PRIMARY KEY,
                username TEXT,
                first_name TEXT NOT NULL,
                last_name TEXT,
                is_bot INTEGER NOT NULL,
                first_seen_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                telegram_message_id INTEGER NOT NULL,
                sender_id INTEGER,
                content TEXT,
                raw_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                ingested_at INTEGER NOT NULL,
                reply_to_message_id INTEGER,
                message_link TEXT,
                UNIQUE(chat_id, telegram_message_id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at DESC);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_content_not_null ON messages(id) WHERE content IS NOT NULL;",
        )
        .execute(&self.pool)
        .await?;

        // Focus mode tables
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS focus_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                focused_chat_id INTEGER,
                focused_at INTEGER
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS focus_chats (
                chat_id INTEGER PRIMARY KEY,
                cursor_message_id INTEGER,
                last_debounce_at INTEGER
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Memory system
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL UNIQUE,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Reminders
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS reminders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                message TEXT NOT NULL,
                remind_at INTEGER NOT NULL,
                fired INTEGER NOT NULL DEFAULT 0,
                cancelled INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_reminders_pending ON reminders(remind_at) WHERE fired = 0 AND cancelled = 0;",
        )
        .execute(&self.pool)
        .await?;

        // Security: strikes
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS strikes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                chat_id INTEGER NOT NULL,
                reason TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Security: DM rate limits
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS dm_rate_limits (
                user_id INTEGER PRIMARY KEY,
                message_count INTEGER NOT NULL DEFAULT 0,
                window_start INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Chat aliases
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chat_aliases (
                alias TEXT PRIMARY KEY,
                chat_id INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Muted chats
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS muted_chats (
                chat_id INTEGER PRIMARY KEY,
                muted_at INTEGER NOT NULL,
                mute_until INTEGER
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Monitored groups (proactive)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS monitored_groups (
                chat_id INTEGER NOT NULL,
                thread_id INTEGER,
                added_at INTEGER NOT NULL,
                UNIQUE(chat_id, thread_id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Proactive cooldowns
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS proactive_cooldowns (
                chat_id INTEGER PRIMARY KEY,
                last_response_at INTEGER NOT NULL DEFAULT 0,
                last_reaction_at INTEGER NOT NULL DEFAULT 0
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Heartbeats
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS heartbeats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                source TEXT NOT NULL,
                details TEXT
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Chat style profiles
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chat_profiles (
                chat_id INTEGER PRIMARY KEY,
                profile_json TEXT NOT NULL,
                prompt_instruction TEXT NOT NULL,
                message_count_at_analysis INTEGER NOT NULL,
                analyzed_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Scheduled tasks (cron jobs)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduled_tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                prompt TEXT NOT NULL,
                cron_expr TEXT NOT NULL,
                timezone TEXT NOT NULL DEFAULT 'Asia/Tashkent',
                next_run_at INTEGER NOT NULL,
                last_run_at INTEGER,
                enabled INTEGER NOT NULL DEFAULT 1,
                running INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_due ON scheduled_tasks(next_run_at) WHERE enabled = 1 AND running = 0;",
        )
        .execute(&self.pool)
        .await?;

        // Per-turn Claude usage/cost/latency — durable time series for the dashboard.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS claude_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                session_id TEXT,
                turn_num INTEGER,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cost_usd REAL,
                duration_ms INTEGER,
                duration_api_ms INTEGER
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_claude_turns_ts ON claude_turns(ts);")
            .execute(&self.pool)
            .await?;

        // Notable session events (context compaction, session rotation).
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS claude_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                kind TEXT NOT NULL,
                detail TEXT
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_claude_events_ts ON claude_events(ts);")
            .execute(&self.pool)
            .await?;

        // MCP tool invocations — written by the mcp-telegram process.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tool_calls (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                tool_name TEXT NOT NULL,
                success INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                error TEXT
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tool_calls_ts ON tool_calls(ts);")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn upsert_chat(&self, input: &UpsertChatInput) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO chats (chat_id, kind, title, username, first_seen_at, last_seen_at)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(chat_id) DO UPDATE SET
                kind = excluded.kind,
                title = COALESCE(excluded.title, chats.title),
                username = COALESCE(excluded.username, chats.username),
                last_seen_at = excluded.last_seen_at;
            "#,
        )
        .bind(input.chat_id)
        .bind(&input.kind)
        .bind(&input.title)
        .bind(&input.username)
        .bind(input.seen_at)
        .bind(input.seen_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn upsert_user(&self, input: &UpsertUserInput) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO users (user_id, username, first_name, last_name, is_bot, first_seen_at, last_seen_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(user_id) DO UPDATE SET
                username = COALESCE(excluded.username, users.username),
                first_name = excluded.first_name,
                last_name = COALESCE(excluded.last_name, users.last_name),
                is_bot = excluded.is_bot,
                last_seen_at = excluded.last_seen_at;
            "#,
        )
        .bind(input.user_id)
        .bind(&input.username)
        .bind(&input.first_name)
        .bind(&input.last_name)
        .bind(if input.is_bot { 1_i64 } else { 0_i64 })
        .bind(input.seen_at)
        .bind(input.seen_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn insert_or_update_message(&self, input: &StoredMessageInput) -> Result<i64> {
        sqlx::query(
            r#"
            INSERT INTO messages (
                chat_id,
                telegram_message_id,
                sender_id,
                content,
                raw_json,
                created_at,
                ingested_at,
                reply_to_message_id,
                message_link
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(chat_id, telegram_message_id) DO UPDATE SET
                sender_id = excluded.sender_id,
                content = excluded.content,
                raw_json = excluded.raw_json,
                created_at = excluded.created_at,
                ingested_at = excluded.ingested_at,
                reply_to_message_id = excluded.reply_to_message_id,
                message_link = COALESCE(excluded.message_link, messages.message_link);
            "#,
        )
        .bind(input.chat_id)
        .bind(input.telegram_message_id)
        .bind(input.sender_id)
        .bind(&input.content)
        .bind(&input.raw_json)
        .bind(input.created_at)
        .bind(input.ingested_at)
        .bind(input.reply_to_message_id)
        .bind(&input.message_link)
        .execute(&self.pool)
        .await?;

        let id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM messages WHERE chat_id = ? AND telegram_message_id = ?",
        )
        .bind(input.chat_id)
        .bind(input.telegram_message_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(id)
    }

    pub async fn fetch_recent_candidates(&self, limit: usize) -> Result<Vec<CandidateMessage>> {
        let rows = sqlx::query(
            r#"
            SELECT
                m.telegram_message_id,
                m.content,
                m.created_at,
                m.message_link,
                c.title AS chat_title
            FROM messages m
            LEFT JOIN chats c ON c.chat_id = m.chat_id
            WHERE m.content IS NOT NULL
            ORDER BY m.created_at DESC
            LIMIT ?;
            "#,
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(CandidateMessage {
                telegram_message_id: row.try_get("telegram_message_id")?,
                chat_title: row.try_get("chat_title")?,
                content: row.try_get("content")?,
                created_at: row.try_get("created_at")?,
                message_link: row.try_get("message_link")?,
            });
        }

        Ok(out)
    }

    pub async fn fetch_recent_chat_messages(
        &self,
        chat_id: i64,
        limit: usize,
    ) -> Result<Vec<RecentChatMessage>> {
        let rows = sqlx::query(
            r#"
            SELECT
                sender_id,
                content,
                created_at,
                telegram_message_id,
                reply_to_message_id
            FROM messages
            WHERE chat_id = ?
              AND content IS NOT NULL
            ORDER BY created_at DESC
            LIMIT ?;
            "#,
        )
        .bind(chat_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(RecentChatMessage {
                sender_id: row.try_get("sender_id")?,
                content: row.try_get("content")?,
                created_at: row.try_get("created_at")?,
                telegram_message_id: row.try_get("telegram_message_id")?,
                reply_to_message_id: row.try_get("reply_to_message_id")?,
            });
        }

        out.reverse();
        Ok(out)
    }

    /// Thread-aware recent messages for the response pipeline.
    /// When thread_id is Some, includes messages matching that thread.
    /// When thread_id is None, delegates to fetch_recent_chat_messages.
    pub async fn fetch_recent_chat_messages_for_thread(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<RecentChatMessage>> {
        if let Some(tid) = thread_id {
            let pattern = format!("\"message_thread_id\":{}", tid);
            let rows = sqlx::query(
                r#"
                SELECT
                    sender_id,
                    content,
                    created_at,
                    telegram_message_id,
                    reply_to_message_id
                FROM messages
                WHERE chat_id = ?
                  AND content IS NOT NULL
                  AND raw_json LIKE '%' || ? || '%'
                ORDER BY created_at DESC
                LIMIT ?;
                "#,
            )
            .bind(chat_id)
            .bind(pattern)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                out.push(RecentChatMessage {
                    sender_id: row.try_get("sender_id")?,
                    content: row.try_get("content")?,
                    created_at: row.try_get("created_at")?,
                    telegram_message_id: row.try_get("telegram_message_id")?,
                    reply_to_message_id: row.try_get("reply_to_message_id")?,
                });
            }
            out.reverse();
            Ok(out)
        } else {
            self.fetch_recent_chat_messages(chat_id, limit).await
        }
    }

    /// Get the timestamp of the bot's last message in a chat.
    pub async fn get_bot_last_message_at(
        &self,
        chat_id: i64,
        bot_user_id: i64,
    ) -> Result<Option<i64>> {
        let row = sqlx::query(
            "SELECT MAX(created_at) as last_at FROM messages WHERE chat_id = ? AND sender_id = ? AND content IS NOT NULL",
        )
        .bind(chat_id)
        .bind(bot_user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("last_at")?)
    }

    // ── Focus state ──

    pub async fn get_focus_state(&self) -> Result<Option<(i64, i64)>> {
        let row = sqlx::query(
            "SELECT focused_chat_id, focused_at FROM focus_state WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => {
                let chat_id: Option<i64> = r.try_get("focused_chat_id")?;
                let at: Option<i64> = r.try_get("focused_at")?;
                match (chat_id, at) {
                    (Some(c), Some(a)) => Ok(Some((c, a))),
                    _ => Ok(None),
                }
            }
            None => Ok(None),
        }
    }

    pub async fn set_focus_state(&self, chat_id: Option<i64>) -> Result<()> {
        let now = epoch_now();
        sqlx::query(
            r#"
            INSERT INTO focus_state (id, focused_chat_id, focused_at) VALUES (1, ?, ?)
            ON CONFLICT(id) DO UPDATE SET focused_chat_id = excluded.focused_chat_id, focused_at = excluded.focused_at;
            "#,
        )
        .bind(chat_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── Memories ──

    pub async fn create_memory(&self, path: &str, content: &str) -> Result<i64> {
        let now = epoch_now();
        sqlx::query(
            "INSERT INTO memories (path, content, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(path)
        .bind(content)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        let id = sqlx::query_scalar::<_, i64>("SELECT id FROM memories WHERE path = ?")
            .bind(path)
            .fetch_one(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn read_memory(&self, path: &str) -> Result<Option<(i64, String, String, i64, i64)>> {
        let row = sqlx::query(
            "SELECT id, path, content, created_at, updated_at FROM memories WHERE path = ?",
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some((
                r.try_get("id")?,
                r.try_get("path")?,
                r.try_get("content")?,
                r.try_get("created_at")?,
                r.try_get("updated_at")?,
            ))),
            None => Ok(None),
        }
    }

    pub async fn update_memory(&self, path: &str, content: &str) -> Result<bool> {
        let now = epoch_now();
        let result = sqlx::query(
            "UPDATE memories SET content = ?, updated_at = ? WHERE path = ?",
        )
        .bind(content)
        .bind(now)
        .bind(path)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_memory(&self, path: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM memories WHERE path = ?")
            .bind(path)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_memories(&self) -> Result<Vec<(String, i64, i64)>> {
        let rows = sqlx::query(
            "SELECT path, created_at, updated_at FROM memories ORDER BY path",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for r in rows {
            out.push((
                r.try_get("path")?,
                r.try_get("created_at")?,
                r.try_get("updated_at")?,
            ));
        }
        Ok(out)
    }

    pub async fn search_memories(&self, query: &str) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query(
            "SELECT path, content FROM memories WHERE content LIKE '%' || ? || '%' OR path LIKE '%' || ? || '%' ORDER BY updated_at DESC LIMIT 20",
        )
        .bind(query)
        .bind(query)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for r in rows {
            out.push((r.try_get("path")?, r.try_get("content")?));
        }
        Ok(out)
    }

    // ── Reminders ──

    pub async fn create_reminder(
        &self,
        chat_id: i64,
        user_id: i64,
        message: &str,
        remind_at: i64,
    ) -> Result<i64> {
        let now = epoch_now();
        sqlx::query(
            "INSERT INTO reminders (chat_id, user_id, message, remind_at, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(chat_id)
        .bind(user_id)
        .bind(message)
        .bind(remind_at)
        .bind(now)
        .execute(&self.pool)
        .await?;
        let id = sqlx::query_scalar::<_, i64>("SELECT last_insert_rowid()")
            .fetch_one(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn list_pending_reminders(&self) -> Result<Vec<(i64, i64, i64, String, i64)>> {
        let rows = sqlx::query(
            "SELECT id, chat_id, user_id, message, remind_at FROM reminders WHERE fired = 0 AND cancelled = 0 ORDER BY remind_at",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for r in rows {
            out.push((
                r.try_get("id")?,
                r.try_get("chat_id")?,
                r.try_get("user_id")?,
                r.try_get("message")?,
                r.try_get("remind_at")?,
            ));
        }
        Ok(out)
    }

    pub async fn fire_reminder(&self, id: i64) -> Result<bool> {
        let result = sqlx::query("UPDATE reminders SET fired = 1 WHERE id = ? AND fired = 0")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn cancel_reminder(&self, id: i64) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE reminders SET cancelled = 1 WHERE id = ? AND fired = 0 AND cancelled = 0",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_due_reminders(&self) -> Result<Vec<(i64, i64, i64, String, i64)>> {
        let now = epoch_now();
        let rows = sqlx::query(
            "SELECT id, chat_id, user_id, message, remind_at FROM reminders WHERE fired = 0 AND cancelled = 0 AND remind_at <= ? ORDER BY remind_at",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for r in rows {
            out.push((
                r.try_get("id")?,
                r.try_get("chat_id")?,
                r.try_get("user_id")?,
                r.try_get("message")?,
                r.try_get("remind_at")?,
            ));
        }
        Ok(out)
    }

    // ── Scheduled Tasks ──

    pub async fn create_scheduled_task(
        &self,
        chat_id: i64,
        user_id: i64,
        name: &str,
        prompt: &str,
        cron_expr: &str,
        timezone: &str,
        next_run_at: i64,
    ) -> Result<i64> {
        let now = epoch_now();
        sqlx::query(
            "INSERT INTO scheduled_tasks (chat_id, user_id, name, prompt, cron_expr, timezone, next_run_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(chat_id)
        .bind(user_id)
        .bind(name)
        .bind(prompt)
        .bind(cron_expr)
        .bind(timezone)
        .bind(next_run_at)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        let id = sqlx::query_scalar::<_, i64>("SELECT last_insert_rowid()")
            .fetch_one(&self.pool)
            .await?;
        Ok(id)
    }

    pub async fn list_scheduled_tasks(&self) -> Result<Vec<ScheduledTask>> {
        let rows = sqlx::query(
            "SELECT id, chat_id, user_id, name, prompt, cron_expr, timezone, next_run_at, last_run_at, enabled, running, created_at, updated_at FROM scheduled_tasks ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ScheduledTask {
                id: r.try_get("id")?,
                chat_id: r.try_get("chat_id")?,
                user_id: r.try_get("user_id")?,
                name: r.try_get("name")?,
                prompt: r.try_get("prompt")?,
                cron_expr: r.try_get("cron_expr")?,
                timezone: r.try_get("timezone")?,
                next_run_at: r.try_get("next_run_at")?,
                last_run_at: r.try_get("last_run_at")?,
                enabled: r.try_get::<i64, _>("enabled")? != 0,
                running: r.try_get::<i64, _>("running")? != 0,
                created_at: r.try_get("created_at")?,
                updated_at: r.try_get("updated_at")?,
            });
        }
        Ok(out)
    }

    pub async fn get_due_scheduled_tasks(&self) -> Result<Vec<ScheduledTask>> {
        let now = epoch_now();
        let rows = sqlx::query(
            "SELECT id, chat_id, user_id, name, prompt, cron_expr, timezone, next_run_at, last_run_at, enabled, running, created_at, updated_at FROM scheduled_tasks WHERE enabled = 1 AND running = 0 AND next_run_at <= ? ORDER BY next_run_at",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ScheduledTask {
                id: r.try_get("id")?,
                chat_id: r.try_get("chat_id")?,
                user_id: r.try_get("user_id")?,
                name: r.try_get("name")?,
                prompt: r.try_get("prompt")?,
                cron_expr: r.try_get("cron_expr")?,
                timezone: r.try_get("timezone")?,
                next_run_at: r.try_get("next_run_at")?,
                last_run_at: r.try_get("last_run_at")?,
                enabled: r.try_get::<i64, _>("enabled")? != 0,
                running: r.try_get::<i64, _>("running")? != 0,
                created_at: r.try_get("created_at")?,
                updated_at: r.try_get("updated_at")?,
            });
        }
        Ok(out)
    }

    pub async fn set_scheduled_task_running(&self, id: i64, running: bool) -> Result<()> {
        sqlx::query("UPDATE scheduled_tasks SET running = ?, updated_at = ? WHERE id = ?")
            .bind(running as i64)
            .bind(epoch_now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_scheduled_task_ran(&self, id: i64, next_run_at: i64) -> Result<()> {
        let now = epoch_now();
        sqlx::query(
            "UPDATE scheduled_tasks SET running = 0, last_run_at = ?, next_run_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(now)
        .bind(next_run_at)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_scheduled_task(
        &self,
        id: i64,
        name: Option<&str>,
        prompt: Option<&str>,
        cron_expr: Option<&str>,
        timezone: Option<&str>,
        enabled: Option<bool>,
        next_run_at: Option<i64>,
    ) -> Result<bool> {
        let mut sets = Vec::new();
        let mut binds: Vec<String> = Vec::new();

        if let Some(v) = name { sets.push("name = ?"); binds.push(v.to_string()); }
        if let Some(v) = prompt { sets.push("prompt = ?"); binds.push(v.to_string()); }
        if let Some(v) = cron_expr { sets.push("cron_expr = ?"); binds.push(v.to_string()); }
        if let Some(v) = timezone { sets.push("timezone = ?"); binds.push(v.to_string()); }
        if let Some(v) = enabled { sets.push("enabled = ?"); binds.push((v as i64).to_string()); }
        if let Some(v) = next_run_at { sets.push("next_run_at = ?"); binds.push(v.to_string()); }

        if sets.is_empty() {
            return Ok(false);
        }

        sets.push("updated_at = ?");
        let now = epoch_now();

        let sql = format!("UPDATE scheduled_tasks SET {} WHERE id = ?", sets.join(", "));
        let mut query = sqlx::query(&sql);
        for b in &binds {
            query = query.bind(b);
        }
        query = query.bind(now).bind(id);
        let result = query.execute(&self.pool).await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_scheduled_task(&self, id: i64) -> Result<bool> {
        let result = sqlx::query("DELETE FROM scheduled_tasks WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn reset_stale_running_tasks(&self) -> Result<u64> {
        let result = sqlx::query("UPDATE scheduled_tasks SET running = 0, updated_at = ? WHERE running = 1")
            .bind(epoch_now())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    // ── Strikes ──

    pub async fn add_strike(&self, user_id: i64, chat_id: i64, reason: &str) -> Result<i64> {
        let now = epoch_now();
        sqlx::query(
            "INSERT INTO strikes (user_id, chat_id, reason, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(chat_id)
        .bind(reason)
        .bind(now)
        .execute(&self.pool)
        .await?;
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM strikes WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    pub async fn get_strike_count(&self, user_id: i64) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM strikes WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    // ── DM rate limits ──

    pub async fn check_dm_rate(
        &self,
        user_id: i64,
        max_count: i64,
        window_secs: i64,
    ) -> Result<bool> {
        let now = epoch_now();
        let row = sqlx::query(
            "SELECT message_count, window_start FROM dm_rate_limits WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => {
                let count: i64 = r.try_get("message_count")?;
                let window_start: i64 = r.try_get("window_start")?;
                if now - window_start > window_secs {
                    sqlx::query(
                        "UPDATE dm_rate_limits SET message_count = 1, window_start = ? WHERE user_id = ?",
                    )
                    .bind(now)
                    .bind(user_id)
                    .execute(&self.pool)
                    .await?;
                    Ok(true)
                } else if count >= max_count {
                    Ok(false)
                } else {
                    sqlx::query(
                        "UPDATE dm_rate_limits SET message_count = message_count + 1 WHERE user_id = ?",
                    )
                    .bind(user_id)
                    .execute(&self.pool)
                    .await?;
                    Ok(true)
                }
            }
            None => {
                sqlx::query(
                    "INSERT INTO dm_rate_limits (user_id, message_count, window_start) VALUES (?, 1, ?)",
                )
                .bind(user_id)
                .bind(now)
                .execute(&self.pool)
                .await?;
                Ok(true)
            }
        }
    }

    // ── Muted chats ──

    pub async fn is_chat_muted(&self, chat_id: i64) -> Result<bool> {
        let now = epoch_now();
        let row = sqlx::query(
            "SELECT mute_until FROM muted_chats WHERE chat_id = ?",
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => {
                let until: Option<i64> = r.try_get("mute_until")?;
                match until {
                    Some(u) if u > 0 && now > u => {
                        sqlx::query("DELETE FROM muted_chats WHERE chat_id = ?")
                            .bind(chat_id)
                            .execute(&self.pool)
                            .await?;
                        Ok(false)
                    }
                    _ => Ok(true),
                }
            }
            None => Ok(false),
        }
    }

    pub async fn mute_chat(&self, chat_id: i64, until: Option<i64>) -> Result<()> {
        let now = epoch_now();
        sqlx::query(
            "INSERT INTO muted_chats (chat_id, muted_at, mute_until) VALUES (?, ?, ?) ON CONFLICT(chat_id) DO UPDATE SET muted_at = excluded.muted_at, mute_until = excluded.mute_until",
        )
        .bind(chat_id)
        .bind(now)
        .bind(until)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn unmute_chat(&self, chat_id: i64) -> Result<bool> {
        let result = sqlx::query("DELETE FROM muted_chats WHERE chat_id = ?")
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    // ── Heartbeats ──

    pub async fn record_heartbeat(&self, source: &str, details: Option<&str>) -> Result<()> {
        let now = epoch_now();
        sqlx::query(
            "INSERT INTO heartbeats (timestamp, source, details) VALUES (?, ?, ?)",
        )
        .bind(now)
        .bind(source)
        .bind(details)
        .execute(&self.pool)
        .await?;
        // Keep only last 1000 heartbeats
        sqlx::query(
            "DELETE FROM heartbeats WHERE id NOT IN (SELECT id FROM heartbeats ORDER BY id DESC LIMIT 1000)",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_recent_heartbeats(&self, limit: usize) -> Result<Vec<(i64, String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT timestamp, source, details FROM heartbeats ORDER BY id DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for r in rows {
            out.push((
                r.try_get("timestamp")?,
                r.try_get("source")?,
                r.try_get("details")?,
            ));
        }
        Ok(out)
    }

    // ── Dashboard: writes ──

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_claude_turn(
        &self,
        session_id: Option<&str>,
        turn_num: Option<i64>,
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
        cost_usd: Option<f64>,
        duration_ms: Option<i64>,
        duration_api_ms: Option<i64>,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO claude_turns
               (ts, session_id, turn_num, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, cost_usd, duration_ms, duration_api_ms)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(epoch_now())
        .bind(session_id)
        .bind(turn_num)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(cache_creation_tokens)
        .bind(cache_read_tokens)
        .bind(cost_usd)
        .bind(duration_ms)
        .bind(duration_api_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_claude_event(&self, kind: &str, detail: Option<&str>) -> Result<()> {
        sqlx::query("INSERT INTO claude_events (ts, kind, detail) VALUES (?, ?, ?)")
            .bind(epoch_now())
            .bind(kind)
            .bind(detail)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_tool_call(
        &self,
        tool_name: &str,
        success: bool,
        duration_ms: i64,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO tool_calls (ts, tool_name, success, duration_ms, error) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(epoch_now())
        .bind(tool_name)
        .bind(success as i64)
        .bind(duration_ms)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── Dashboard: reads (return JSON for the HTTP layer) ──

    pub async fn dash_messages(
        &self,
        chat_id: Option<i64>,
        limit: i64,
        before_id: Option<i64>,
    ) -> Result<serde_json::Value> {
        let mut sql = String::from(
            r#"SELECT m.id, m.chat_id, m.telegram_message_id, m.sender_id, m.content,
                      m.created_at, m.message_link, c.title AS chat_title,
                      u.first_name, u.last_name, u.username, u.is_bot
               FROM messages m
               LEFT JOIN chats c ON c.chat_id = m.chat_id
               LEFT JOIN users u ON u.user_id = m.sender_id"#,
        );
        let mut conds: Vec<&str> = Vec::new();
        if chat_id.is_some() {
            conds.push("m.chat_id = ?");
        }
        if before_id.is_some() {
            conds.push("m.id < ?");
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY m.id DESC LIMIT ?");

        let mut q = sqlx::query(&sql);
        if let Some(c) = chat_id {
            q = q.bind(c);
        }
        if let Some(b) = before_id {
            q = q.bind(b);
        }
        q = q.bind(limit.clamp(1, 500));

        let rows = q.fetch_all(&self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let first: Option<String> = r.try_get("first_name").ok();
            let last: Option<String> = r.try_get("last_name").ok();
            let username: Option<String> = r.try_get("username").ok();
            let is_bot: Option<i64> = r.try_get("is_bot").ok();
            let name = match (first, last) {
                (Some(f), Some(l)) => format!("{f} {l}"),
                (Some(f), None) => f,
                _ => username.clone().unwrap_or_else(|| "?".to_string()),
            };
            out.push(serde_json::json!({
                "id": r.try_get::<i64, _>("id")?,
                "chat_id": r.try_get::<i64, _>("chat_id")?,
                "telegram_message_id": r.try_get::<i64, _>("telegram_message_id")?,
                "sender_id": r.try_get::<Option<i64>, _>("sender_id")?,
                "sender_name": name,
                "username": username,
                "is_bot": is_bot.unwrap_or(0) != 0,
                "content": r.try_get::<Option<String>, _>("content")?,
                "created_at": r.try_get::<i64, _>("created_at")?,
                "message_link": r.try_get::<Option<String>, _>("message_link")?,
                "chat_title": r.try_get::<Option<String>, _>("chat_title")?,
            }));
        }
        let next_before = rows
            .last()
            .and_then(|r| r.try_get::<i64, _>("id").ok());
        Ok(serde_json::json!({ "messages": out, "next_before_id": next_before }))
    }

    pub async fn dash_chats(&self) -> Result<serde_json::Value> {
        let rows = sqlx::query(
            r#"SELECT m.chat_id, c.title, c.kind, COUNT(*) AS cnt, MAX(m.created_at) AS last_at
               FROM messages m
               LEFT JOIN chats c ON c.chat_id = m.chat_id
               GROUP BY m.chat_id
               ORDER BY last_at DESC
               LIMIT 200"#,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(serde_json::json!({
                "chat_id": r.try_get::<i64, _>("chat_id")?,
                "title": r.try_get::<Option<String>, _>("title")?,
                "kind": r.try_get::<Option<String>, _>("kind")?,
                "count": r.try_get::<i64, _>("cnt")?,
                "last_at": r.try_get::<Option<i64>, _>("last_at")?,
            }));
        }
        Ok(serde_json::json!({ "chats": out }))
    }

    pub async fn dash_context(&self) -> Result<serde_json::Value> {
        let row = sqlx::query(
            r#"SELECT ts, session_id, turn_num, input_tokens, output_tokens,
                      cache_creation_tokens, cache_read_tokens, duration_api_ms
               FROM claude_turns ORDER BY id DESC LIMIT 1"#,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else {
            return Ok(serde_json::json!({ "latest": null }));
        };
        let input: i64 = r.try_get("input_tokens")?;
        let cache_read: i64 = r.try_get("cache_read_tokens")?;
        let cache_creation: i64 = r.try_get("cache_creation_tokens")?;
        // `context_tokens` approximates the working-set size for the last turn.
        // NB: cache_read_tokens is CUMULATIVE across all internal API calls in a
        // turn (tool loops, compaction re-reads), so it routinely exceeds the
        // context window and must NOT be used as a window-fill measure. Use
        // uncached input + newly-cached tokens, which stays bounded per turn.
        let context_tokens = input + cache_creation;
        Ok(serde_json::json!({
            "latest": {
                "ts": r.try_get::<i64, _>("ts")?,
                "session_id": r.try_get::<Option<String>, _>("session_id")?,
                "turn_num": r.try_get::<Option<i64>, _>("turn_num")?,
                "input_tokens": input,
                "output_tokens": r.try_get::<i64, _>("output_tokens")?,
                "cache_creation_tokens": cache_creation,
                "cache_read_tokens": cache_read,
                "context_tokens": context_tokens,
                "duration_api_ms": r.try_get::<Option<i64>, _>("duration_api_ms")?,
            }
        }))
    }

    pub async fn dash_turns(&self, limit: i64, model: Option<&str>) -> Result<serde_json::Value> {
        let (rates, cost_basis) = model_rates(model);
        let cost = rates.cost_sql();

        let turn_sql = format!(
            "SELECT ts, turn_num, input_tokens, output_tokens, cache_creation_tokens, \
                    cache_read_tokens, {cost} AS est_cost, duration_ms, duration_api_ms \
             FROM claude_turns ORDER BY id DESC LIMIT ?"
        );
        let rows = sqlx::query(&turn_sql)
            .bind(limit.clamp(1, 2000))
            .fetch_all(&self.pool)
            .await?;
        let mut turns = Vec::with_capacity(rows.len());
        for r in &rows {
            turns.push(serde_json::json!({
                "ts": r.try_get::<i64, _>("ts")?,
                "turn_num": r.try_get::<Option<i64>, _>("turn_num")?,
                "input_tokens": r.try_get::<i64, _>("input_tokens")?,
                "output_tokens": r.try_get::<i64, _>("output_tokens")?,
                "cache_creation_tokens": r.try_get::<i64, _>("cache_creation_tokens")?,
                "cache_read_tokens": r.try_get::<i64, _>("cache_read_tokens")?,
                "cost_usd": r.try_get::<f64, _>("est_cost")?,
                "duration_ms": r.try_get::<Option<i64>, _>("duration_ms")?,
                "duration_api_ms": r.try_get::<Option<i64>, _>("duration_api_ms")?,
            }));
        }
        turns.reverse();
        let day_ago = epoch_now() - 86_400;
        let agg_sql = format!(
            "SELECT COUNT(*) AS total_turns, \
                    COALESCE(SUM(input_tokens),0) AS in_tok, \
                    COALESCE(SUM(output_tokens),0) AS out_tok, \
                    COALESCE(SUM(cache_creation_tokens),0) AS cw_tok, \
                    COALESCE(SUM(cache_read_tokens),0) AS cache_tok, \
                    COALESCE(SUM({cost}),0.0) AS cost \
             FROM claude_turns"
        );
        let agg = sqlx::query(&agg_sql).fetch_one(&self.pool).await?;
        let agg_day_sql = format!(
            "SELECT COUNT(*) AS total_turns, \
                    COALESCE(SUM(input_tokens),0) AS in_tok, \
                    COALESCE(SUM(output_tokens),0) AS out_tok, \
                    COALESCE(SUM({cost}),0.0) AS cost \
             FROM claude_turns WHERE ts >= ?"
        );
        let agg_day = sqlx::query(&agg_day_sql)
            .bind(day_ago)
            .fetch_one(&self.pool)
            .await?;
        Ok(serde_json::json!({
            "turns": turns,
            "cost_basis": cost_basis,
            "totals": {
                "turns": agg.try_get::<i64, _>("total_turns")?,
                "input_tokens": agg.try_get::<i64, _>("in_tok")?,
                "output_tokens": agg.try_get::<i64, _>("out_tok")?,
                "cache_creation_tokens": agg.try_get::<i64, _>("cw_tok")?,
                "cache_read_tokens": agg.try_get::<i64, _>("cache_tok")?,
                "cost_usd": agg.try_get::<f64, _>("cost")?,
            },
            "last_24h": {
                "turns": agg_day.try_get::<i64, _>("total_turns")?,
                "input_tokens": agg_day.try_get::<i64, _>("in_tok")?,
                "output_tokens": agg_day.try_get::<i64, _>("out_tok")?,
                "cost_usd": agg_day.try_get::<f64, _>("cost")?,
            }
        }))
    }

    pub async fn dash_tools(&self) -> Result<serde_json::Value> {
        let rows = sqlx::query(
            r#"SELECT tool_name,
                      COUNT(*) AS calls,
                      COALESCE(SUM(CASE WHEN success=0 THEN 1 ELSE 0 END),0) AS errors,
                      COALESCE(AVG(duration_ms),0) AS mean_ms,
                      MAX(ts) AS last_at
               FROM tool_calls
               GROUP BY tool_name
               ORDER BY calls DESC
               LIMIT 100"#,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(serde_json::json!({
                "name": r.try_get::<String, _>("tool_name")?,
                "calls": r.try_get::<i64, _>("calls")?,
                "errors": r.try_get::<i64, _>("errors")?,
                "mean_ms": r.try_get::<f64, _>("mean_ms")?.round() as i64,
                "last_at": r.try_get::<Option<i64>, _>("last_at")?,
            }));
        }
        Ok(serde_json::json!({ "tools": out }))
    }

    pub async fn dash_events(&self, limit: i64) -> Result<serde_json::Value> {
        let rows = sqlx::query(
            "SELECT ts, kind, detail FROM claude_events ORDER BY id DESC LIMIT ?",
        )
        .bind(limit.clamp(1, 200))
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(serde_json::json!({
                "ts": r.try_get::<i64, _>("ts")?,
                "kind": r.try_get::<String, _>("kind")?,
                "detail": r.try_get::<Option<String>, _>("detail")?,
            }));
        }
        let compactions: i64 = sqlx::query("SELECT COUNT(*) AS n FROM claude_events WHERE kind='compaction'")
            .fetch_one(&self.pool)
            .await?
            .try_get("n")?;
        Ok(serde_json::json!({ "events": out, "compaction_count": compactions }))
    }

    pub async fn dash_behavior(&self) -> Result<serde_json::Value> {
        // Messages per day (last 30 days).
        let day_rows = sqlx::query(
            r#"SELECT CAST(created_at/86400 AS INTEGER) AS day, COUNT(*) AS cnt
               FROM messages
               WHERE created_at >= ?
               GROUP BY day ORDER BY day"#,
        )
        .bind(epoch_now() - 30 * 86_400)
        .fetch_all(&self.pool)
        .await?;
        let per_day: Vec<serde_json::Value> = day_rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "day_epoch": r.try_get::<i64, _>("day").unwrap_or(0) * 86_400,
                    "count": r.try_get::<i64, _>("cnt").unwrap_or(0),
                })
            })
            .collect();

        // Active-hours heatmap (UTC hour-of-day, all-time).
        let hour_rows = sqlx::query(
            r#"SELECT CAST((created_at % 86400)/3600 AS INTEGER) AS hour, COUNT(*) AS cnt
               FROM messages GROUP BY hour ORDER BY hour"#,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut hours = vec![0i64; 24];
        for r in &hour_rows {
            let h = r.try_get::<i64, _>("hour").unwrap_or(0);
            if (0..24).contains(&h) {
                hours[h as usize] = r.try_get::<i64, _>("cnt").unwrap_or(0);
            }
        }

        // Tone distribution from chat_profiles.prompt_instruction is freeform;
        // expose the raw per-chat profile summary instead.
        let tone_rows = sqlx::query(
            r#"SELECT cp.chat_id, c.title, cp.prompt_instruction, cp.analyzed_at
               FROM chat_profiles cp LEFT JOIN chats c ON c.chat_id = cp.chat_id
               ORDER BY cp.analyzed_at DESC LIMIT 50"#,
        )
        .fetch_all(&self.pool)
        .await?;
        let profiles: Vec<serde_json::Value> = tone_rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "chat_id": r.try_get::<i64, _>("chat_id").unwrap_or(0),
                    "title": r.try_get::<Option<String>, _>("title").unwrap_or(None),
                    "instruction": r.try_get::<Option<String>, _>("prompt_instruction").unwrap_or(None),
                    "analyzed_at": r.try_get::<Option<i64>, _>("analyzed_at").unwrap_or(None),
                })
            })
            .collect();

        let memory_count: i64 = sqlx::query("SELECT COUNT(*) AS n FROM memories")
            .fetch_one(&self.pool)
            .await?
            .try_get("n")?;

        // Reminder + scheduled-task activity.
        let reminders_fired: i64 = match sqlx::query("SELECT COUNT(*) AS n FROM reminders WHERE fired=1")
            .fetch_one(&self.pool)
            .await
        {
            Ok(r) => r.try_get("n").unwrap_or(0),
            Err(_) => 0,
        };
        let sched_rows = sqlx::query(
            r#"SELECT name, cron_expr, enabled, last_run_at, next_run_at
               FROM scheduled_tasks ORDER BY COALESCE(last_run_at,0) DESC LIMIT 50"#,
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        let scheduled: Vec<serde_json::Value> = sched_rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.try_get::<String, _>("name").unwrap_or_default(),
                    "cron": r.try_get::<String, _>("cron_expr").unwrap_or_default(),
                    "enabled": r.try_get::<i64, _>("enabled").unwrap_or(0) != 0,
                    "last_run_at": r.try_get::<Option<i64>, _>("last_run_at").unwrap_or(None),
                    "next_run_at": r.try_get::<Option<i64>, _>("next_run_at").unwrap_or(None),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "messages_per_day": per_day,
            "active_hours_utc": hours,
            "profiles": profiles,
            "memory_count": memory_count,
            "reminders_fired": reminders_fired,
            "scheduled_tasks": scheduled,
        }))
    }

    // ── Monitored groups (proactive) ──

    pub async fn seed_monitored_groups(&self, entries: &[(i64, Option<i64>)]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        // Wrap all inserts in a single transaction so we pay one fsync for the
        // batch instead of one per row. Matters for startup latency when a
        // large group list is seeded from env.
        let now = epoch_now();
        let mut tx = self.pool.begin().await?;
        for &(chat_id, thread_id) in entries {
            sqlx::query(
                "INSERT OR IGNORE INTO monitored_groups (chat_id, thread_id, added_at) VALUES (?, ?, ?)",
            )
            .bind(chat_id)
            .bind(thread_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Check if a chat (and optionally a specific thread) is monitored.
    /// If the group is registered with a thread_id, only that thread matches.
    /// If registered with thread_id=NULL, all threads match.
    pub async fn is_monitored_group(&self, chat_id: i64, thread_id: Option<i64>) -> Result<bool> {
        let row = sqlx::query(
            "SELECT 1 FROM monitored_groups WHERE chat_id = ? AND (thread_id IS NULL OR thread_id = ?)",
        )
        .bind(chat_id)
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    /// Check if a chat_id has any monitored entry (regardless of thread).
    pub async fn chat_has_any_monitor(&self, chat_id: i64) -> Result<bool> {
        let row = sqlx::query("SELECT 1 FROM monitored_groups WHERE chat_id = ? LIMIT 1")
            .bind(chat_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    pub async fn get_monitored_groups(&self) -> Result<Vec<(i64, Option<i64>)>> {
        let rows = sqlx::query("SELECT chat_id, thread_id FROM monitored_groups ORDER BY added_at")
            .fetch_all(&self.pool)
            .await?;
        let mut out = Vec::new();
        for r in rows {
            out.push((r.try_get("chat_id")?, r.try_get("thread_id")?));
        }
        Ok(out)
    }

    pub async fn add_monitored_group(&self, chat_id: i64, thread_id: Option<i64>) -> Result<bool> {
        let now = epoch_now();
        let result = sqlx::query(
            "INSERT OR IGNORE INTO monitored_groups (chat_id, thread_id, added_at) VALUES (?, ?, ?)",
        )
        .bind(chat_id)
        .bind(thread_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn remove_monitored_group(&self, chat_id: i64, thread_id: Option<i64>) -> Result<bool> {
        let result = if thread_id.is_some() {
            sqlx::query("DELETE FROM monitored_groups WHERE chat_id = ? AND thread_id = ?")
                .bind(chat_id)
                .bind(thread_id)
                .execute(&self.pool)
                .await?
        } else {
            sqlx::query("DELETE FROM monitored_groups WHERE chat_id = ? AND thread_id IS NULL")
                .bind(chat_id)
                .execute(&self.pool)
                .await?
        };
        Ok(result.rows_affected() > 0)
    }

    pub async fn get_proactive_cooldown(&self, chat_id: i64) -> Result<(i64, i64)> {
        let row = sqlx::query(
            "SELECT last_response_at, last_reaction_at FROM proactive_cooldowns WHERE chat_id = ?",
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok((r.try_get("last_response_at")?, r.try_get("last_reaction_at")?)),
            None => Ok((0, 0)),
        }
    }

    pub async fn update_proactive_cooldown(
        &self,
        chat_id: i64,
        response_at: Option<i64>,
        reaction_at: Option<i64>,
    ) -> Result<()> {
        let (cur_resp, cur_react) = self.get_proactive_cooldown(chat_id).await?;
        let resp = response_at.unwrap_or(cur_resp);
        let react = reaction_at.unwrap_or(cur_react);
        sqlx::query(
            "INSERT INTO proactive_cooldowns (chat_id, last_response_at, last_reaction_at) VALUES (?, ?, ?) ON CONFLICT(chat_id) DO UPDATE SET last_response_at = excluded.last_response_at, last_reaction_at = excluded.last_reaction_at",
        )
        .bind(chat_id)
        .bind(resp)
        .bind(react)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fetch_recent_chat_messages_with_names(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<(Option<i64>, Option<String>, String, i64)>> {
        let rows = if let Some(tid) = thread_id {
            let pattern = format!("\"message_thread_id\":{}", tid);
            // Include messages matching the thread
            sqlx::query(
                r#"
                SELECT
                    m.sender_id,
                    COALESCE(u.first_name, u.username, CAST(m.sender_id AS TEXT)) AS sender_name,
                    m.content,
                    m.created_at
                FROM messages m
                LEFT JOIN users u ON u.user_id = m.sender_id
                WHERE m.chat_id = ?
                  AND m.content IS NOT NULL
                  AND m.raw_json LIKE '%' || ? || '%'
                ORDER BY m.created_at DESC
                LIMIT ?;
                "#,
            )
            .bind(chat_id)
            .bind(pattern)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT
                    m.sender_id,
                    COALESCE(u.first_name, u.username, CAST(m.sender_id AS TEXT)) AS sender_name,
                    m.content,
                    m.created_at
                FROM messages m
                LEFT JOIN users u ON u.user_id = m.sender_id
                WHERE m.chat_id = ?
                  AND m.content IS NOT NULL
                ORDER BY m.created_at DESC
                LIMIT ?;
                "#,
            )
            .bind(chat_id)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        };

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push((
                row.try_get("sender_id")?,
                row.try_get("sender_name")?,
                row.try_get("content")?,
                row.try_get("created_at")?,
            ));
        }
        out.reverse();
        Ok(out)
    }

    // ── Chat profiles ──

    pub async fn upsert_chat_profile(
        &self,
        chat_id: i64,
        profile_json: &str,
        prompt_instruction: &str,
        message_count: i64,
    ) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        sqlx::query(
            r#"
            INSERT INTO chat_profiles (chat_id, profile_json, prompt_instruction, message_count_at_analysis, analyzed_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(chat_id) DO UPDATE SET
                profile_json = excluded.profile_json,
                prompt_instruction = excluded.prompt_instruction,
                message_count_at_analysis = excluded.message_count_at_analysis,
                analyzed_at = excluded.analyzed_at;
            "#,
        )
        .bind(chat_id)
        .bind(profile_json)
        .bind(prompt_instruction)
        .bind(message_count)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns (prompt_instruction, profile_json, message_count_at_analysis)
    pub async fn get_chat_profile(&self, chat_id: i64) -> Result<Option<(String, String, i64)>> {
        let row = sqlx::query(
            "SELECT prompt_instruction, profile_json, message_count_at_analysis FROM chat_profiles WHERE chat_id = ?",
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some((
                r.try_get("prompt_instruction")?,
                r.try_get("profile_json")?,
                r.try_get("message_count_at_analysis")?,
            ))),
            None => Ok(None),
        }
    }

    pub async fn get_chat_message_count(&self, chat_id: i64) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) as cnt FROM messages WHERE chat_id = ? AND content IS NOT NULL",
        )
        .bind(chat_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("cnt")?)
    }

    /// Returns (chat_id, chat_title, current_message_count) for chats needing profile generation/update.
    pub async fn get_chats_needing_profile(
        &self,
        min_messages: i64,
        delta_threshold: i64,
    ) -> Result<Vec<(i64, String, i64)>> {
        let rows = sqlx::query(
            r#"
            SELECT c.chat_id, COALESCE(c.title, CAST(c.chat_id AS TEXT)) AS title, mc.cnt
            FROM chats c
            JOIN (
                SELECT chat_id, COUNT(*) AS cnt
                FROM messages
                WHERE content IS NOT NULL
                GROUP BY chat_id
            ) mc ON mc.chat_id = c.chat_id
            LEFT JOIN chat_profiles cp ON cp.chat_id = c.chat_id
            WHERE
                (cp.chat_id IS NULL AND mc.cnt >= ?)
                OR (cp.chat_id IS NOT NULL AND mc.cnt - cp.message_count_at_analysis >= ?)
            "#,
        )
        .bind(min_messages)
        .bind(delta_threshold)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push((
                row.try_get("chat_id")?,
                row.try_get("title")?,
                row.try_get("cnt")?,
            ));
        }
        Ok(out)
    }

    /// Look up a user's display name by user_id.
    pub async fn get_user_display_name(&self, user_id: i64) -> Result<Option<String>> {
        let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT first_name, last_name, username FROM users WHERE user_id = ?"
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(first, last, username)| {
            let mut name = first.unwrap_or_default();
            if let Some(l) = last {
                if !name.is_empty() { name.push(' '); }
                name.push_str(&l);
            }
            if name.trim().is_empty() {
                if let Some(u) = username {
                    return format!("@{u}");
                }
                return format!("id={user_id}");
            }
            name
        }))
    }

    // ── Generic query (SELECT only) ──


    pub async fn execute_select_query(&self, sql: &str, max_rows: usize) -> Result<String> {
        let trimmed = sql.trim().trim_end_matches(';').trim();
        validate_read_only_select(trimmed)?;

        // LIMIT injection via text is safe here: we've already AST-validated the
        // input as a single read-only SELECT, and `max_rows` is a server constant.
        let upper = trimmed.to_ascii_uppercase();
        let limited_sql = if upper.contains(" LIMIT ") || upper.starts_with("LIMIT ") {
            trimmed.to_string()
        } else {
            format!("{trimmed} LIMIT {max_rows}")
        };

        let rows = sqlx::query(&limited_sql)
            .fetch_all(&self.pool)
            .await?;

        let mut result = Vec::new();
        for row in &rows {
            let mut obj = serde_json::Map::new();
            for (i, col) in row.columns().iter().enumerate() {
                let val: serde_json::Value =
                    if let Ok(v) = row.try_get::<i64, _>(i) {
                        serde_json::json!(v)
                    } else if let Ok(v) = row.try_get::<f64, _>(i) {
                        serde_json::json!(v)
                    } else if let Ok(v) = row.try_get::<String, _>(i) {
                        serde_json::json!(v)
                    } else {
                        serde_json::Value::Null
                    };
                obj.insert(col.name().to_string(), val);
            }
            result.push(serde_json::Value::Object(obj));
        }

        Ok(serde_json::to_string_pretty(&result)?)
    }
}

/// Parse `sql` and reject anything that isn't a single read-only SELECT.
///
/// Why a real parser: the previous keyword blocklist was bypassable via comments,
/// CTEs with INSERT/UPDATE/DELETE, ATTACH/PRAGMA, and stacked statements. The
/// SQLite dialect AST gives us ground truth about what the statement actually is.
fn validate_read_only_select(sql: &str) -> Result<()> {
    use sqlparser::ast::Statement;
    use sqlparser::dialect::SQLiteDialect;
    use sqlparser::parser::Parser;

    if sql.is_empty() {
        anyhow::bail!("query is empty");
    }

    let statements = Parser::parse_sql(&SQLiteDialect {}, sql)
        .map_err(|e| anyhow::anyhow!("SQL parse error: {e}"))?;

    if statements.is_empty() {
        anyhow::bail!("no SQL statement provided");
    }
    if statements.len() > 1 {
        anyhow::bail!("only a single statement is allowed (got {})", statements.len());
    }

    match &statements[0] {
        Statement::Query(q) => ensure_read_only_set_expr(&q.body),
        other => {
            let dbg = format!("{other:?}");
            let kind = dbg.split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("statement");
            anyhow::bail!("only SELECT/VALUES queries are allowed (got {kind})")
        }
    }
}

/// Walk a Query body and reject Insert/Update/Delete/Merge bodies that would
/// otherwise slip through a WITH-prefixed top-level Query.
fn ensure_read_only_set_expr(body: &sqlparser::ast::SetExpr) -> Result<()> {
    use sqlparser::ast::SetExpr;
    match body {
        SetExpr::Select(_) | SetExpr::Values(_) | SetExpr::Table(_) => Ok(()),
        SetExpr::Query(inner) => ensure_read_only_set_expr(&inner.body),
        SetExpr::SetOperation { left, right, .. } => {
            ensure_read_only_set_expr(left)?;
            ensure_read_only_set_expr(right)
        }
        SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => {
            anyhow::bail!("writable statements are not allowed inside a query")
        }
    }
}

#[cfg(test)]
mod query_validator_tests {
    use super::validate_read_only_select;

    #[test]
    fn accepts_plain_select() {
        assert!(validate_read_only_select("SELECT 1").is_ok());
        assert!(validate_read_only_select("select id, content from messages where chat_id = 1").is_ok());
    }

    #[test]
    fn accepts_select_with_cte() {
        let sql = "WITH recent AS (SELECT * FROM messages ORDER BY id DESC LIMIT 10) \
                   SELECT COUNT(*) FROM recent";
        assert!(validate_read_only_select(sql).is_ok());
    }

    #[test]
    fn rejects_insert() {
        assert!(validate_read_only_select("INSERT INTO users (id) VALUES (1)").is_err());
    }

    #[test]
    fn rejects_update() {
        assert!(validate_read_only_select("UPDATE users SET name = 'x'").is_err());
    }

    #[test]
    fn rejects_delete() {
        assert!(validate_read_only_select("DELETE FROM users").is_err());
    }

    #[test]
    fn rejects_drop() {
        assert!(validate_read_only_select("DROP TABLE users").is_err());
    }

    #[test]
    fn rejects_attach() {
        assert!(validate_read_only_select("ATTACH DATABASE '/tmp/x.db' AS x").is_err());
    }

    #[test]
    fn rejects_pragma() {
        assert!(validate_read_only_select("PRAGMA writable_schema = 1").is_err());
    }

    #[test]
    fn rejects_stacked_statements() {
        assert!(validate_read_only_select("SELECT 1; DROP TABLE users").is_err());
    }

    #[test]
    fn rejects_writable_cte() {
        // CTE followed by a non-query body should be rejected.
        let sql = "WITH foo AS (SELECT 1) INSERT INTO users (id) VALUES (2)";
        assert!(validate_read_only_select(sql).is_err());
    }

    #[test]
    fn rejects_comment_hidden_injection() {
        // Comment + stacked write must still be caught.
        let sql = "SELECT 1 /* tricky */ ; UPDATE users SET name='x'";
        assert!(validate_read_only_select(sql).is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_read_only_select("").is_err());
        assert!(validate_read_only_select("   ").is_err());
    }
}

fn epoch_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

pub struct ModelRates {
    pub label: &'static str,
    pub input: f64,
    pub output: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

impl ModelRates {
    fn cost_sql(&self) -> String {
        format!(
            "(input_tokens*{:?} + output_tokens*{:?} \
             + cache_creation_tokens*{:?} + cache_read_tokens*{:?})/1000000.0",
            self.input, self.output, self.cache_write_1h, self.cache_read
        )
    }

    fn summary(&self) -> String {
        format!(
            "{} rates ($/1M: input {}, output {}, 1h cache-write {}, cache-read {})",
            self.label, self.input, self.output, self.cache_write_1h, self.cache_read
        )
    }
}

const OPUS_5_5_RATES: ModelRates = ModelRates {
    label: "Claude Opus 5.5",
    input: 4.0,
    output: 20.0,
    cache_write_1h: 8.0,
    cache_read: 0.2,
};

const MODEL_RATES: &[(&str, ModelRates)] = &[
    ("opus-5-5", OPUS_5_5_RATES),
    (
        "opus-4-8",
        ModelRates {
            label: "Claude Opus 4.8",
            input: 5.0,
            output: 25.0,
            cache_write_1h: 10.0,
            cache_read: 0.5,
        },
    ),
    (
        "sonnet-5-5",
        ModelRates {
            label: "Claude Sonnet 5.5",
            input: 2.0,
            output: 10.0,
            cache_write_1h: 4.0,
            cache_read: 0.1,
        },
    ),
    (
        "fable-5-1",
        ModelRates {
            label: "Claude Fable 5.1",
            input: 10.0,
            output: 50.0,
            cache_write_1h: 20.0,
            cache_read: 0.25,
        },
    ),
];

pub fn model_rates(model: Option<&str>) -> (&'static ModelRates, String) {
    let found = model.and_then(|m| {
        MODEL_RATES
            .iter()
            .find(|(key, _)| m.contains(key))
            .map(|(_, rates)| rates)
    });
    match (found, model) {
        (Some(rates), _) => (rates, format!("official {}", rates.summary())),
        (None, Some(m)) => (
            &OPUS_5_5_RATES,
            format!("model '{m}' not in rate table — default {}", OPUS_5_5_RATES.summary()),
        ),
        (None, None) => (
            &OPUS_5_5_RATES,
            format!("model unset (CLI default) — default {}", OPUS_5_5_RATES.summary()),
        ),
    }
}

fn ensure_sqlite_parent_exists(database_url: &str) -> Result<()> {
    if !database_url.starts_with("sqlite:") {
        return Ok(());
    }

    let mut path_part = database_url.trim_start_matches("sqlite:");
    if let Some((head, _)) = path_part.split_once("?") {
        path_part = head;
    }
    if path_part.starts_with("//") {
        path_part = path_part.trim_start_matches('/');
    }
    if path_part == ":memory:" || path_part.is_empty() {
        return Ok(());
    }

    let path = Path::new(path_part);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create sqlite parent dir: {}", parent.display())
            })?;
        }
    }

    Ok(())
}


#[cfg(test)]
mod dashboard_db_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    async fn tmp_db() -> Database {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("jarvis_dash_{}_{}.db", std::process::id(), n));
        let _ = std::fs::remove_file(&p);
        let url = format!("sqlite://{}", p.display());
        let db = Database::connect(&url).await.unwrap();
        db.init().await.unwrap();
        db
    }

    #[tokio::test]
    async fn claude_turn_roundtrip_and_aggregates() {
        let db = tmp_db().await;
        db.insert_claude_turn(Some("sess-1"), Some(1), 1000, 200, 50, 800, Some(0.12), Some(1500), Some(1400))
            .await
            .unwrap();
        db.insert_claude_turn(Some("sess-1"), Some(2), 1200, 300, 0, 900, Some(0.18), Some(1700), Some(1600))
            .await
            .unwrap();

        let ctx = db.dash_context().await.unwrap();
        let latest = &ctx["latest"];
        assert_eq!(latest["turn_num"], 2);
        // context_tokens = input + cache_creation = 1200 + 0 (cache_read is
        // cumulative-per-turn and deliberately excluded from window fill)
        assert_eq!(latest["context_tokens"], 1200);

        let turns = db.dash_turns(10, Some("claude-opus-4-8")).await.unwrap();
        assert_eq!(turns["totals"]["turns"], 2);
        assert_eq!(turns["totals"]["input_tokens"], 2200);
        assert_eq!(turns["totals"]["output_tokens"], 500);
        // chronological order after reverse: first turn first
        assert_eq!(turns["turns"][0]["turn_num"], 1);
        let cost = turns["totals"]["cost_usd"].as_f64().unwrap();
        assert!((cost - 0.02485).abs() < 1e-9, "cost was {cost}");
        assert!(turns["cost_basis"].as_str().unwrap().contains("Opus 4.8"));

        let turns = db.dash_turns(10, Some("claude-opus-5-5")).await.unwrap();
        let cost = turns["totals"]["cost_usd"].as_f64().unwrap();
        assert!((cost - 0.01954).abs() < 1e-9, "cost was {cost}");
        let first_turn = turns["turns"][0]["cost_usd"].as_f64().unwrap();
        assert!((first_turn - 0.00856).abs() < 1e-9, "turn cost was {first_turn}");
    }

    #[test]
    fn model_rates_selects_by_model_id() {
        let (rates, basis) = model_rates(Some("claude-opus-5-5"));
        assert_eq!(rates.label, "Claude Opus 5.5");
        assert_eq!(rates.cache_write_1h, 8.0);
        assert!(basis.starts_with("official Claude Opus 5.5 rates"));
        assert_eq!(model_rates(Some("claude-opus-4-8[1m]")).0.label, "Claude Opus 4.8");
        assert_eq!(model_rates(Some("claude-sonnet-5-5")).0.label, "Claude Sonnet 5.5");
        assert_eq!(model_rates(Some("claude-fable-5-1")).0.label, "Claude Fable 5.1");
    }

    #[test]
    fn model_rates_unknown_or_unset_falls_back_labelled() {
        let (rates, basis) = model_rates(Some("opus"));
        assert_eq!(rates.label, "Claude Opus 5.5");
        assert!(basis.contains("model 'opus' not in rate table"), "{basis}");
        let (rates, basis) = model_rates(None);
        assert_eq!(rates.label, "Claude Opus 5.5");
        assert!(basis.contains("model unset"), "{basis}");
    }

    #[tokio::test]
    async fn dash_turns_empty_table_ok() {
        // Regression: COALESCE(SUM(cost_usd),0) returned an INTEGER on an empty
        // table, which failed try_get::<f64>. The 0.0 literal fixes the affinity.
        let db = tmp_db().await;
        let d = db.dash_turns(10, None).await.unwrap();
        assert_eq!(d["totals"]["turns"], 0);
        assert_eq!(d["totals"]["cost_usd"], 0.0);
        assert_eq!(d["last_24h"]["cost_usd"], 0.0);
    }

    #[tokio::test]
    async fn tool_calls_aggregate() {
        let db = tmp_db().await;
        db.insert_tool_call("web_fetch", true, 120, None).await.unwrap();
        db.insert_tool_call("web_fetch", false, 80, Some("boom")).await.unwrap();
        db.insert_tool_call("send_photo", true, 50, None).await.unwrap();

        let d = db.dash_tools().await.unwrap();
        let tools = d["tools"].as_array().unwrap();
        let wf = tools.iter().find(|t| t["name"] == "web_fetch").unwrap();
        assert_eq!(wf["calls"], 2);
        assert_eq!(wf["errors"], 1);
        assert_eq!(wf["mean_ms"], 100);
    }

    #[tokio::test]
    async fn events_compaction_count() {
        let db = tmp_db().await;
        db.insert_claude_event("compaction", Some("ctx full")).await.unwrap();
        db.insert_claude_event("rotation", None).await.unwrap();
        db.insert_claude_event("compaction", None).await.unwrap();

        let d = db.dash_events(10).await.unwrap();
        assert_eq!(d["compaction_count"], 2);
        assert_eq!(d["events"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn messages_pagination() {
        let db = tmp_db().await;
        for i in 1..=5 {
            db.insert_or_update_message(&StoredMessageInput {
                chat_id: 42,
                telegram_message_id: i,
                sender_id: Some(7),
                content: Some(format!("msg {i}")),
                raw_json: "{}".to_string(),
                created_at: 1000 + i,
                ingested_at: 1000 + i,
                reply_to_message_id: None,
                message_link: None,
            })
            .await
            .unwrap();
        }
        let d = db.dash_messages(Some(42), 3, None).await.unwrap();
        let msgs = d["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        // newest first
        assert_eq!(msgs[0]["telegram_message_id"], 5);
        let next = d["next_before_id"].as_i64().unwrap();
        let d2 = db.dash_messages(Some(42), 3, Some(next)).await.unwrap();
        assert_eq!(d2["messages"].as_array().unwrap().len(), 2);
    }
}
