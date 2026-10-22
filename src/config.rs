use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::env;
use std::net::SocketAddr;
use url::Url;

#[derive(Debug, Clone)]
pub struct Config {
    pub telegram_bot_token: Option<String>,
    pub claude_code_bin: String,
    pub claude_code_model: Option<String>,
    pub claude_code_timeout_secs: u64,
    pub claude_code_session_file: String,
    pub database_url: String,
    pub allowed_users: HashSet<i64>,
    pub owner_user_id: Option<i64>,
    pub bot_aliases: Vec<String>,
    pub rag_top_k: usize,
    pub rag_candidate_limit: usize,
    pub max_context_chars: usize,
    pub system_prompt_template: String,
    pub user_rules_template: String,
    pub webhook_url: Option<Url>,
    pub webhook_bind: SocketAddr,
    pub claude_code_allowed_tools: Vec<String>,
    /// Monitored groups: Vec of (chat_id, optional thread_id).
    pub monitored_groups: Vec<(i64, Option<i64>)>,
    pub proactive_cooldown_secs: u64,
    pub proactive_bot_cooldown_secs: u64,
    pub proactive_decision_timeout_secs: u64,
    pub proactive_active_conversation_secs: u64,
    pub proactive_batch_collect_ms: u64,
    pub active_followup_secs: u64,
    pub proactive_cold_trigger_enabled: bool,
    pub claude_session_max_turns: u32,
    pub claude_session_max_duration_ms: u64,
    pub claude_session_duration_window: usize,
    /// Shared secret the dashboard routes require in the `X-Dash-Auth` header.
    /// Injected by the nginx vhost that fronts the dashboard subdomain. When
    /// unset, dashboard routes are disabled (return 404).
    pub dashboard_auth_secret: Option<String>,
    /// Model context-window size in tokens, used to compute the live
    /// context-fill gauge. 200k for standard Claude; 1_000_000 for the 1M variant.
    pub context_window_tokens: u64,
}

pub struct McpConfig {
    pub database_url: String,
    pub bot_token: Option<String>,
    pub faceit_api_key: Option<String>,
    pub google_service_account_key: Option<String>,
    /// Default Google Calendar id for calendar_* tools (the calendar must be
    /// shared with the service account email).
    pub google_calendar_id: Option<String>,
    /// Gmail address for the account Jarvis will read. App-password auth.
    pub gmail_imap_user: Option<String>,
    /// App password generated at https://myaccount.google.com/apppasswords.
    /// NOT the normal Gmail login — that won't work with IMAP.
    pub gmail_app_password: Option<String>,
}

impl McpConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://data/jarvis.db".to_string()),
            bot_token: optional_string("TELOXIDE_TOKEN"),
            faceit_api_key: optional_string("FACEIT_API_KEY"),
            google_service_account_key: optional_string("GOOGLE_SERVICE_ACCOUNT_KEY"),
            google_calendar_id: optional_string("GOOGLE_CALENDAR_ID"),
            gmail_imap_user: optional_string("GMAIL_IMAP_USER"),
            gmail_app_password: optional_string("GMAIL_APP_PASSWORD"),
        })
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let telegram_bot_token = optional_string("TELOXIDE_TOKEN");
        if telegram_bot_token.is_none() {
            bail!("Missing required env var: TELOXIDE_TOKEN");
        }

        let claude_code_bin = env::var("CLAUDE_CODE_BIN")
            .or_else(|_| env::var("CODEX_CLI_BIN"))
            .unwrap_or_else(|_| "claude".to_string());
        let claude_code_model = env::var("CLAUDE_CODE_MODEL")
            .or_else(|_| env::var("CODEX_MODEL"))
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let claude_code_timeout_secs = match env::var("CLAUDE_CODE_TIMEOUT_SECS") {
            Ok(v) if !v.trim().is_empty() => v.trim().parse::<u64>().context("CLAUDE_CODE_TIMEOUT_SECS must be u64")?,
            _ => parse_u64("CODEX_TIMEOUT_SECS", 180)?,
        };
        let claude_code_session_file = env::var("CLAUDE_CODE_SESSION_FILE")
            .unwrap_or_else(|_| "data/claude_session_id".to_string());
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://data/jarvis.db".to_string());

        let allowed_users = parse_i64_list(env::var("ALLOWED_USERS").unwrap_or_default());
        let owner_user_id = env::var("OWNER_USER_ID")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .or_else(|| allowed_users.iter().copied().next());

        let bot_aliases =
            parse_string_list(env::var("BOT_ALIASES").unwrap_or_else(|_| "jarvis,bot".to_string()));

        let rag_top_k = parse_usize("RAG_TOP_K", 6)?;
        let rag_candidate_limit = parse_usize("RAG_CANDIDATE_LIMIT", 2000)?;
        let max_context_chars = parse_usize("MAX_CONTEXT_CHARS", 4000)?;
        let system_prompt_file = env::var("SYSTEM_PROMPT_FILE")
            .unwrap_or_else(|_| "prompts/system_prompt.txt".to_string());
        let user_rules_file =
            env::var("USER_RULES_FILE").unwrap_or_else(|_| "prompts/user_rules.txt".to_string());
        let system_prompt_template = std::fs::read_to_string(&system_prompt_file)
            .with_context(|| format!("failed to read prompt file: {system_prompt_file}"))?;
        let user_rules_template = std::fs::read_to_string(&user_rules_file)
            .with_context(|| format!("failed to read prompt file: {user_rules_file}"))?;

        let webhook_url = env::var("WEBHOOK_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|v| Url::parse(v.trim()).context("WEBHOOK_URL is invalid"))
            .transpose()?;

        let webhook_bind = env::var("WEBHOOK_BIND")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
            .parse::<SocketAddr>()
            .context("WEBHOOK_BIND must be host:port")?;

        let claude_code_allowed_tools = parse_string_list(
            env::var("CLAUDE_CODE_ALLOWED_TOOLS")
                .or_else(|_| env::var("CODEX_ALLOWED_TOOLS"))
                .unwrap_or_else(|_| "mcp__telegram__*,WebSearch,WebFetch".to_string()),
        );

        let monitored_groups = parse_monitored_groups(env::var("MONITORED_GROUPS").unwrap_or_default());
        let proactive_cooldown_secs = parse_u64("PROACTIVE_COOLDOWN_SECS", 300)?;
        let proactive_bot_cooldown_secs = parse_u64("PROACTIVE_BOT_COOLDOWN_SECS", 60)?;
        let proactive_decision_timeout_secs = parse_u64("PROACTIVE_DECISION_TIMEOUT_SECS", 30)?;
        let proactive_active_conversation_secs = parse_u64("PROACTIVE_ACTIVE_CONVERSATION_SECS", 90)?;
        let proactive_batch_collect_ms = parse_u64("PROACTIVE_BATCH_COLLECT_MS", 8000)?;
        let active_followup_secs = parse_u64("ACTIVE_FOLLOWUP_SECS", 180)?;
        let proactive_cold_trigger_enabled = parse_bool("PROACTIVE_COLD_TRIGGER_ENABLED", true);

        let claude_session_max_turns = parse_u64("CLAUDE_SESSION_MAX_TURNS", 0)? as u32;
        let claude_session_max_duration_ms = parse_u64("CLAUDE_SESSION_MAX_DURATION_MS", 0)?;
        let claude_session_duration_window = parse_usize("CLAUDE_SESSION_DURATION_WINDOW", 3)?;

        let dashboard_auth_secret = optional_string("DASHBOARD_AUTH_SECRET");
        let context_window_tokens = parse_u64("CONTEXT_WINDOW_TOKENS", 200_000)?;

        Ok(Self {
            telegram_bot_token,
            claude_code_bin,
            claude_code_model,
            claude_code_timeout_secs,
            claude_code_session_file,
            database_url,
            allowed_users,
            owner_user_id,
            bot_aliases,
            rag_top_k,
            rag_candidate_limit,
            max_context_chars,
            system_prompt_template,
            user_rules_template,
            webhook_url,
            webhook_bind,
            claude_code_allowed_tools,
            monitored_groups,
            proactive_cooldown_secs,
            proactive_bot_cooldown_secs,
            proactive_decision_timeout_secs,
            proactive_active_conversation_secs,
            proactive_batch_collect_ms,
            active_followup_secs,
            proactive_cold_trigger_enabled,
            claude_session_max_turns,
            claude_session_max_duration_ms,
            claude_session_duration_window,
            dashboard_auth_secret,
            context_window_tokens,
        })
    }

    pub fn is_allowed_private_user(&self, user_id: i64) -> bool {
        self.allowed_users.is_empty() || self.allowed_users.contains(&user_id)
    }
}

fn optional_string(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn parse_i64_list(input: String) -> HashSet<i64> {
    input
        .split(',')
        .filter_map(|piece| {
            let trimmed = piece.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse::<i64>().ok()
        })
        .collect()
}

fn parse_string_list(input: String) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn parse_usize(key: &str, default: usize) -> Result<usize> {
    match env::var(key) {
        Ok(v) => v
            .trim()
            .parse::<usize>()
            .with_context(|| format!("{key} must be usize")),
        Err(_) => Ok(default),
    }
}

fn parse_u64(key: &str, default: u64) -> Result<u64> {
    match env::var(key) {
        Ok(v) => v
            .trim()
            .parse::<u64>()
            .with_context(|| format!("{key} must be u64")),
        Err(_) => Ok(default),
    }
}

fn parse_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(v) => {
            let t = v.trim().to_ascii_lowercase();
            matches!(t.as_str(), "1" | "true" | "yes" | "on")
                || (default && t.is_empty())
        }
        Err(_) => default,
    }
}

/// Parse MONITORED_GROUPS env var. Format: "chat_id:thread_id,chat_id,..."
/// Each entry is either "chat_id" (monitors all threads) or "chat_id:thread_id" (specific topic).
fn parse_monitored_groups(input: String) -> Vec<(i64, Option<i64>)> {
    input
        .split(',')
        .filter_map(|piece| {
            let trimmed = piece.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Some((chat_str, thread_str)) = trimmed.split_once(':') {
                let chat_id = chat_str.trim().parse::<i64>().ok()?;
                let thread_id = thread_str.trim().parse::<i64>().ok()?;
                Some((chat_id, Some(thread_id)))
            } else {
                let chat_id = trimmed.parse::<i64>().ok()?;
                Some((chat_id, None))
            }
        })
        .collect()
}
