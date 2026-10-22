use std::env;
use telegram_codex_jarvis::claude_code::{ClaudeCode, ClaudeCodeConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let model = env::var("CLAUDE_CODE_MODEL").ok().filter(|v| !v.trim().is_empty());
    println!("model = {:?}", model);

    let claude = ClaudeCode::new(ClaudeCodeConfig {
        binary: env::var("CLAUDE_CODE_BIN").unwrap_or_else(|_| "claude".into()),
        model,
        timeout_secs: 120,
        allowed_tools: vec!["mcp__telegram__*".into()],
        session_file: "data/claude_session_id".into(),
        default_system_prompt: "You are a test harness.".into(),
        session_max_turns: 0,
        session_max_duration_ms: 0,
        session_duration_window: 20,
    });

    claude.start("You are a test harness. Answer briefly.").await?;
    println!("alive = {}", claude.is_alive());

    let prompt = env::args().nth(1).unwrap_or_else(|| "Which model are you? Reply in one line.".into());
    let reply = claude.send_message("You are a test harness. Answer briefly.", &prompt, "example run").await?;
    println!("--- reply ---\n{reply}");

    claude.stop().await;
    Ok(())
}
