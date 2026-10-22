use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct CancelReminderTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelReminderParams {
    /// Reminder id to cancel.
    pub id: i64,
}

impl Tool for CancelReminderTool {
    type Params = CancelReminderParams;

    fn name(&self) -> &'static str {
        "cancel_reminder"
    }

    fn description(&self) -> &'static str {
        "Cancel a pending reminder by ID."
    }

    fn run<'a>(
        &'a self,
        p: CancelReminderParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { crate::reminders::cancel_reminder(&server.db, p.id).await })
    }
}
