use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct CreateScheduledTaskTool;

fn default_timezone() -> String {
    "Asia/Tashkent".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateScheduledTaskParams {
    /// Target chat id where the task's output will be sent when it runs.
    pub chat_id: i64,
    /// User id that owns this task (for audit).
    pub user_id: i64,
    /// Human-readable task name.
    pub name: String,
    /// Prompt sent to Claude at each scheduled run.
    pub prompt: String,
    /// 5-field cron expression: `minute hour day month weekday` (e.g. `0 13 * * *` = daily at 13:00).
    pub cron_expr: String,
    /// IANA timezone for the cron schedule. Defaults to `Asia/Tashkent`.
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

impl Tool for CreateScheduledTaskTool {
    type Params = CreateScheduledTaskParams;

    fn name(&self) -> &'static str {
        "create_scheduled_task"
    }

    fn description(&self) -> &'static str {
        "Create a recurring scheduled task (cron job). The bot runs the prompt via Claude at each scheduled time and sends the result to the target chat."
    }

    fn run<'a>(
        &'a self,
        p: CreateScheduledTaskParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            crate::scheduled_tasks::create_scheduled_task(
                &server.db,
                p.chat_id,
                p.user_id,
                &p.name,
                &p.prompt,
                &p.cron_expr,
                &p.timezone,
            )
            .await
        })
    }
}
