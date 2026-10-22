use crate::claude_code::ClaudeCode;
use crate::db::Database;
use anyhow::{Result, anyhow, bail};
use cron::Schedule;
use std::str::FromStr;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

/// Background service that checks for due scheduled tasks and executes them via Claude.
pub struct ScheduledTaskService {
    db: Database,
    bot: Bot,
    claude: Arc<ClaudeCode>,
    check_interval: Duration,
}

impl ScheduledTaskService {
    pub fn new(
        db: Database,
        bot: Bot,
        claude: Arc<ClaudeCode>,
    ) -> Self {
        Self {
            db,
            bot,
            claude,
            check_interval: Duration::from_secs(30),
        }
    }

    /// Start the background checker loop. Call this via `tokio::spawn`.
    pub async fn run(self) {
        info!(
            "scheduled_task service started (checking every {}s)",
            self.check_interval.as_secs()
        );

        // Reset tasks stuck as "running" from a previous crash
        match self.db.reset_stale_running_tasks().await {
            Ok(n) if n > 0 => info!(count = n, "reset stale running scheduled tasks"),
            Ok(_) => {}
            Err(e) => warn!(?e, "failed to reset stale running tasks"),
        }

        let svc = Arc::new(self);
        loop {
            sleep(svc.check_interval).await;
            if let Err(e) = svc.check_and_fire().await {
                error!(?e, "scheduled_task check failed");
            }
        }
    }

    async fn check_and_fire(self: &Arc<Self>) -> Result<()> {
        let due = self.db.get_due_scheduled_tasks().await?;

        for task in due {
            // Mark as running to prevent double-fire
            self.db.set_scheduled_task_running(task.id, true).await?;

            // Execute sequentially — all tasks share the same subprocess which
            // serializes calls internally via send_lock anyway.
            self.execute_task(&task).await;
        }

        Ok(())
    }

    async fn execute_task(&self, task: &crate::db::ScheduledTask) {
        // Teach Claude the silent-marker convention for polling tasks.
        // Without this, "check gmail every 10 min" produces a "no new mail"
        // Telegram message every 10 min, which is worse than useless.
        let user_prompt = format!(
            "[Scheduled task: \"{}\"]\n\n\
             {}\n\n\
             NOTIFICATION RULES — read carefully:\n\
             • If there is nothing worth notifying about (no new emails, no \
               new jobs, no state change, polling returned identical data \
               to last time, etc.) respond with EXACTLY the single token \
               [SILENT] and nothing else. Do not explain. Do not apologize. \
               Do not include details about what you checked. Just [SILENT].\n\
             • If you have already sent a Telegram message yourself (via \
               bot_send_message) respond with EXACTLY [BOT_SENT] so the \
               runner does not duplicate it.\n\
             • Only respond with free text when there is real, actionable \
               content the user needs to see right now. The user has \
               explicitly asked that routine 'no change' poll outputs be \
               silent.",
            task.name, task.prompt,
        );

        let result = self.claude.send_message(&user_prompt, &format!("Scheduled task · {}", task.name)).await;

        match result {
            Ok(response) => {
                if is_silent_response(&response) {
                    info!(
                        task_id = task.id,
                        name = %task.name,
                        "scheduled_task_silent"
                    );
                    // Deliberately do not send anything to the user.
                } else {
                    // Truncate to Telegram's 4096 char limit, leaving room for header
                    let header = format!("[{}]\n", task.name);
                    let max_body = 4096 - header.len().min(200);
                    let body = if response.len() > max_body {
                        format!("{}...", &response[..max_body - 3])
                    } else {
                        response
                    };
                    let msg = format!("{}{}", header, body);

                    match self.bot.send_message(ChatId(task.chat_id), &msg).await {
                        Ok(_) => {
                            info!(task_id = task.id, name = %task.name, "scheduled_task_fired");
                        }
                        Err(e) => {
                            warn!(?e, task_id = task.id, "scheduled_task_send_failed");
                        }
                    }
                }
            }
            Err(e) => {
                warn!(?e, task_id = task.id, name = %task.name, "scheduled_task_execution_failed");
            }
        }

        // Compute next run and update
        let next = match compute_next_run_at(&task.cron_expr, &task.timezone) {
            Ok(ts) => ts,
            Err(e) => {
                error!(?e, task_id = task.id, "failed to compute next_run_at, disabling task");
                let _ = self
                    .db
                    .update_scheduled_task(task.id, None, None, None, None, Some(false), None)
                    .await;
                let _ = self.db.set_scheduled_task_running(task.id, false).await;
                return;
            }
        };

        if let Err(e) = self.db.mark_scheduled_task_ran(task.id, next).await {
            error!(?e, task_id = task.id, "failed to update task after run");
        }
    }
}

/// True when Claude's response indicates the runner should NOT send a
/// Telegram message:
///
/// - Completely empty / whitespace only
/// - Contains the `[SILENT]` marker (anywhere, case-insensitive)
/// - Contains the `[BOT_SENT]` marker (Claude already sent its own message)
///
/// We use `contains` rather than exact-match because Claude sometimes wraps
/// markers in other punctuation — e.g. `Last known uid still 25575. [SILENT].`
/// which is the exact pattern that triggered this fix.
fn is_silent_response(response: &str) -> bool {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.contains("[silent]") || lower.contains("[bot_sent]")
}

#[cfg(test)]
mod silent_tests {
    use super::is_silent_response;

    #[test]
    fn detects_explicit_marker() {
        assert!(is_silent_response("[SILENT]"));
        assert!(is_silent_response("[silent]"));
        assert!(is_silent_response("  [SILENT]  "));
    }

    #[test]
    fn detects_marker_mixed_with_text() {
        // The exact shape that triggered the bug report.
        assert!(is_silent_response(
            "Last known uid still 25575. No new emails.\n[SILENT]."
        ));
    }

    #[test]
    fn detects_bot_sent() {
        assert!(is_silent_response("[BOT_SENT]"));
        assert!(is_silent_response("sent.\n[BOT_SENT]"));
    }

    #[test]
    fn detects_empty() {
        assert!(is_silent_response(""));
        assert!(is_silent_response("   \n\t  "));
    }

    #[test]
    fn allows_actual_content() {
        assert!(!is_silent_response("You got an interview invitation from Google."));
        assert!(!is_silent_response("2 new jobs found matching rust backend"));
    }
}

/// Compute the next run time for a cron expression in the given timezone.
/// The `cron` crate uses 7-field expressions: sec min hour day month dow year.
/// We accept 5-field (min hour day month dow) and auto-prepend "0 " for seconds.
pub fn compute_next_run_at(cron_expr: &str, timezone: &str) -> Result<i64> {
    let fields = cron_expr.split_whitespace().count();
    let full_expr = match fields {
        5 => format!("0 {} *", cron_expr),  // prepend sec=0, append year=*
        6 => format!("0 {}", cron_expr),     // prepend sec=0
        7 => cron_expr.to_string(),          // already full
        _ => bail!("invalid cron expression: expected 5, 6, or 7 fields, got {fields}"),
    };

    let schedule = Schedule::from_str(&full_expr)
        .map_err(|e| anyhow!("invalid cron expression '{}': {}", cron_expr, e))?;

    let tz: chrono_tz::Tz = timezone
        .parse()
        .map_err(|_| anyhow!("invalid timezone: {timezone}"))?;

    let now = chrono::Utc::now().with_timezone(&tz);
    let next = schedule
        .after(&now)
        .next()
        .ok_or_else(|| anyhow!("no next occurrence for cron expression"))?;

    Ok(next.timestamp())
}

// ── CRUD functions (called from MCP handlers) ──

/// Create a scheduled task. Validates cron expression and timezone up front.
pub async fn create_scheduled_task(
    db: &Database,
    chat_id: i64,
    user_id: i64,
    name: &str,
    prompt: &str,
    cron_expr: &str,
    timezone: &str,
) -> Result<String> {
    // Validate cron + timezone by computing first run
    let next_run_at = compute_next_run_at(cron_expr, timezone)?;

    let id = db
        .create_scheduled_task(chat_id, user_id, name, prompt, cron_expr, timezone, next_run_at)
        .await?;

    let next_dt = chrono::DateTime::from_timestamp(next_run_at, 0)
        .map(|dt| {
            let tz: chrono_tz::Tz = timezone.parse().unwrap_or(chrono_tz::Asia::Tashkent);
            dt.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()
        })
        .unwrap_or_else(|| next_run_at.to_string());

    Ok(format!(
        "Scheduled task #{id} created: \"{name}\"\nCron: {cron_expr} ({timezone})\nNext run: {next_dt}"
    ))
}

/// List all scheduled tasks.
pub async fn list_scheduled_tasks(db: &Database) -> Result<String> {
    let tasks = db.list_scheduled_tasks().await?;
    if tasks.is_empty() {
        return Ok("No scheduled tasks.".to_string());
    }

    let mut out = String::new();
    for t in &tasks {
        let status = if !t.enabled {
            "disabled"
        } else if t.running {
            "running"
        } else {
            "active"
        };

        let next_dt = chrono::DateTime::from_timestamp(t.next_run_at, 0)
            .map(|dt| {
                let tz: chrono_tz::Tz = t.timezone.parse().unwrap_or(chrono_tz::Asia::Tashkent);
                dt.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()
            })
            .unwrap_or_else(|| t.next_run_at.to_string());

        out.push_str(&format!(
            "#{}: \"{}\" [{}]\n  cron: {} ({})\n  chat: {}\n  next: {}\n  prompt: {}\n\n",
            t.id,
            t.name,
            status,
            t.cron_expr,
            t.timezone,
            t.chat_id,
            next_dt,
            if t.prompt.len() > 80 {
                format!("{}...", &t.prompt[..80])
            } else {
                t.prompt.clone()
            },
        ));
    }
    Ok(out)
}

/// Update a scheduled task. Only provided fields are changed.
pub async fn update_scheduled_task(
    db: &Database,
    id: i64,
    name: Option<&str>,
    prompt: Option<&str>,
    cron_expr: Option<&str>,
    timezone: Option<&str>,
    enabled: Option<bool>,
) -> Result<String> {
    // If cron or timezone is changing, validate and recompute next_run_at
    let next_run_at = if cron_expr.is_some() || timezone.is_some() {
        // Need current values for the ones not being changed
        let tasks = db.list_scheduled_tasks().await?;
        let current = tasks
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("task #{id} not found"))?;

        let effective_cron = cron_expr.unwrap_or(&current.cron_expr);
        let effective_tz = timezone.unwrap_or(&current.timezone);
        let next = compute_next_run_at(effective_cron, effective_tz)?;
        Some(next)
    } else {
        None
    };

    if db
        .update_scheduled_task(id, name, prompt, cron_expr, timezone, enabled, next_run_at)
        .await?
    {
        Ok(format!("Scheduled task #{id} updated"))
    } else {
        Ok(format!("Scheduled task #{id} not found"))
    }
}

/// Delete a scheduled task.
pub async fn delete_scheduled_task(db: &Database, id: i64) -> Result<String> {
    if db.delete_scheduled_task(id).await? {
        Ok(format!("Scheduled task #{id} deleted"))
    } else {
        Ok(format!("Scheduled task #{id} not found"))
    }
}
