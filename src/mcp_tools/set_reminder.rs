use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SetReminderTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetReminderParams {
    /// Target chat id where the reminder will be sent when it fires.
    pub chat_id: i64,
    /// User id who created the reminder (for audit).
    pub user_id: i64,
    /// Reminder text.
    pub message: String,
    /// Unix timestamp at which to fire the reminder.
    pub remind_at: i64,
}

impl Tool for SetReminderTool {
    type Params = SetReminderParams;

    fn name(&self) -> &'static str {
        "set_reminder"
    }

    fn description(&self) -> &'static str {
        "Set a timed reminder. The bot will send the message at remind_at (Unix timestamp)."
    }

    fn run<'a>(
        &'a self,
        p: SetReminderParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            crate::reminders::set_reminder(&server.db, p.chat_id, p.user_id, &p.message, p.remind_at).await
        })
    }
}
