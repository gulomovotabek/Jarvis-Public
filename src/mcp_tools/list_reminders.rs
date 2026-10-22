use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct ListRemindersTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListRemindersParams {}

impl Tool for ListRemindersTool {
    type Params = ListRemindersParams;

    fn name(&self) -> &'static str {
        "list_reminders"
    }

    fn description(&self) -> &'static str {
        "List all pending (unfired, uncancelled) reminders."
    }

    fn run<'a>(
        &'a self,
        _p: ListRemindersParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { crate::reminders::list_reminders(&server.db).await })
    }
}
