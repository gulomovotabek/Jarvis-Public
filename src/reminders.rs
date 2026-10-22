use crate::db::Database;
use anyhow::Result;
use teloxide::prelude::*;
use teloxide::types::ChatId;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

/// Background service that checks for due reminders and fires them.
pub struct ReminderService {
    db: Database,
    bot: Bot,
    check_interval: Duration,
}

impl ReminderService {
    pub fn new(db: Database, bot: Bot) -> Self {
        Self {
            db,
            bot,
            check_interval: Duration::from_secs(30),
        }
    }

    /// Start the background checker loop. Call this via `tokio::spawn`.
    pub async fn run(self) {
        info!("reminder service started (checking every {}s)", self.check_interval.as_secs());
        loop {
            sleep(self.check_interval).await;
            if let Err(e) = self.check_and_fire().await {
                error!(?e, "reminder check failed");
            }
        }
    }

    async fn check_and_fire(&self) -> Result<()> {
        let due = self.db.get_due_reminders().await?;

        for (id, chat_id, _user_id, message, _remind_at) in due {
            if self.db.fire_reminder(id).await? {
                let text = format!("Eslatma: {message}");
                match self.bot.send_message(ChatId(chat_id), &text).await {
                    Ok(_) => {
                        info!(reminder_id = id, chat_id, "reminder fired");
                    }
                    Err(e) => {
                        warn!(?e, reminder_id = id, chat_id, "failed to send reminder");
                    }
                }
            }
        }

        Ok(())
    }
}

/// Set a reminder. `remind_at` is a Unix timestamp.
pub async fn set_reminder(
    db: &Database,
    chat_id: i64,
    user_id: i64,
    message: &str,
    remind_at: i64,
) -> Result<String> {
    let id = db.create_reminder(chat_id, user_id, message, remind_at).await?;
    Ok(format!("Reminder #{id} set for timestamp {remind_at}"))
}

/// List pending reminders.
pub async fn list_reminders(db: &Database) -> Result<String> {
    let reminders = db.list_pending_reminders().await?;
    if reminders.is_empty() {
        return Ok("No pending reminders.".to_string());
    }

    let mut out = String::new();
    for (id, chat_id, _user_id, message, remind_at) in &reminders {
        out.push_str(&format!(
            "#{id}: chat={chat_id} at={remind_at} — {message}\n"
        ));
    }
    Ok(out)
}

/// Cancel a reminder by ID.
pub async fn cancel_reminder(db: &Database, id: i64) -> Result<String> {
    if db.cancel_reminder(id).await? {
        Ok(format!("Reminder #{id} cancelled"))
    } else {
        Ok(format!("Reminder #{id} not found or already fired/cancelled"))
    }
}
