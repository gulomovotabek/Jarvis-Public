use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct DeleteScheduledTaskTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteScheduledTaskParams {
    /// Task id to delete.
    pub id: i64,
}

impl Tool for DeleteScheduledTaskTool {
    type Params = DeleteScheduledTaskParams;

    fn name(&self) -> &'static str {
        "delete_scheduled_task"
    }

    fn description(&self) -> &'static str {
        "Permanently delete a scheduled task by id. Irreversible."
    }

    fn run<'a>(
        &'a self,
        p: DeleteScheduledTaskParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { crate::scheduled_tasks::delete_scheduled_task(&server.db, p.id).await })
    }
}
