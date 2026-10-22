use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct UpdateScheduledTaskTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateScheduledTaskParams {
    /// Task id to update.
    pub id: i64,
    /// New task name.
    #[serde(default)]
    pub name: Option<String>,
    /// New prompt body.
    #[serde(default)]
    pub prompt: Option<String>,
    /// New cron expression (5-field).
    #[serde(default)]
    pub cron_expr: Option<String>,
    /// New IANA timezone.
    #[serde(default)]
    pub timezone: Option<String>,
    /// Enable or disable the task.
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl Tool for UpdateScheduledTaskTool {
    type Params = UpdateScheduledTaskParams;

    fn name(&self) -> &'static str {
        "update_scheduled_task"
    }

    fn description(&self) -> &'static str {
        "Update one or more fields of an existing scheduled task. Only provided fields change."
    }

    fn run<'a>(
        &'a self,
        p: UpdateScheduledTaskParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            crate::scheduled_tasks::update_scheduled_task(
                &server.db,
                p.id,
                p.name.as_deref(),
                p.prompt.as_deref(),
                p.cron_expr.as_deref(),
                p.timezone.as_deref(),
                p.enabled,
            )
            .await
        })
    }
}
