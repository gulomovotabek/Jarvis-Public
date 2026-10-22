use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct ListScheduledTasksTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListScheduledTasksParams {}

impl Tool for ListScheduledTasksTool {
    type Params = ListScheduledTasksParams;

    fn name(&self) -> &'static str {
        "list_scheduled_tasks"
    }

    fn description(&self) -> &'static str {
        "List all scheduled tasks with status, cron expression, and next run time."
    }

    fn run<'a>(
        &'a self,
        _p: ListScheduledTasksParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { crate::scheduled_tasks::list_scheduled_tasks(&server.db).await })
    }
}
