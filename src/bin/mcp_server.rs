use anyhow::{Result};
use std::collections::HashSet;
use std::sync::Arc;
use telegram_codex_jarvis::config::McpConfig;
use telegram_codex_jarvis::db::Database;
#[cfg(feature = "faceit")]
use telegram_codex_jarvis::faceit::FaceitClient;
#[cfg(feature = "gmail")]
use telegram_codex_jarvis::gmail::GmailClient;
#[cfg(feature = "sheets")]
use telegram_codex_jarvis::google_sheets::GoogleSheetsClient;
#[cfg(feature = "smtp")]
use telegram_codex_jarvis::smtp::SmtpClient;
use telegram_codex_jarvis::focus::FocusManager;
use telegram_codex_jarvis::mcp::McpServer;
use telegram_codex_jarvis::memory::MemoryManager;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    // Log to stderr so stdout stays clean for JSON-RPC. Default to `info`
    // so MCP tool calls leave a trail in journald.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .compact()
        .init();

    let config = McpConfig::from_env()?;
    let db = Database::connect(&config.database_url).await?;
    db.init().await?;

    let memory = Arc::new(MemoryManager::new(db.clone()));
    let focus = Arc::new(FocusManager::new(db.clone(), None, HashSet::new()));

    #[cfg(feature = "faceit")]
    let faceit = config
        .faceit_api_key
        .clone()
        .map(FaceitClient::new)
        .transpose()?;

    #[cfg(feature = "sheets")]
    let google_sheets = config
        .google_service_account_key
        .clone()
        .map(|path| GoogleSheetsClient::from_key_file(&path))
        .transpose()?;

    #[cfg(feature = "calendar")]
    let google_calendar = config
        .google_service_account_key
        .clone()
        .map(|path| {
            telegram_codex_jarvis::google_calendar::GoogleCalendarClient::from_key_file(
                &path,
                config.google_calendar_id.clone(),
            )
        })
        .transpose()?;

    #[cfg(feature = "gmail")]
    let gmail = match (config.gmail_imap_user.clone(), config.gmail_app_password.clone()) {
        (Some(user), Some(pass)) => Some(GmailClient::new(user, pass)),
        _ => None,
    };

    // SMTP reuses the Gmail app password by default. Overrideable via
    // SMTP_USER / SMTP_PASSWORD / SMTP_HOST / SMTP_PORT for non-Gmail setups.
    #[cfg(feature = "smtp")]
    let smtp = {
        let user = std::env::var("SMTP_USER").ok().or_else(|| config.gmail_imap_user.clone());
        let pass = std::env::var("SMTP_PASSWORD").ok().or_else(|| config.gmail_app_password.clone());
        let host = std::env::var("SMTP_HOST").unwrap_or_else(|_| "smtp.gmail.com".to_string());
        let port: u16 = std::env::var("SMTP_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(587);
        match (user, pass) {
            (Some(u), Some(p)) => SmtpClient::new(u, p, host, port).ok(),
            _ => None,
        }
    };

    // Resolve bot user ID for storing sent messages in DB
    let bot_user_id = if let Some(ref token) = config.bot_token {
        use teloxide::prelude::*;
        match Bot::new(token).get_me().await {
            Ok(me) => Some(i64::try_from(me.id.0).unwrap_or_default()),
            Err(e) => {
                eprintln!("warning: could not get bot user ID: {e}");
                None
            }
        }
    } else {
        None
    };

    let server = McpServer {
        db,
        memory,
        focus,
        bot_token: config.bot_token,
        bot_user_id,
        #[cfg(feature = "faceit")]
        faceit,
        #[cfg(feature = "sheets")]
        google_sheets,
        #[cfg(feature = "calendar")]
        google_calendar,
        #[cfg(feature = "gmail")]
        gmail,
        #[cfg(feature = "smtp")]
        smtp,
        tool_metrics: std::sync::Arc::new(telegram_codex_jarvis::mcp::ToolMetrics::new()),
    };
    server.run().await
}
