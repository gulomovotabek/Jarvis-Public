use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct AddMonitoredGroupTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddMonitoredGroupParams {
    /// Chat id of the group to monitor.
    pub chat_id: i64,
    /// Optional forum topic thread id. If set, only that topic is monitored.
    #[serde(default)]
    pub thread_id: Option<i64>,
}

impl Tool for AddMonitoredGroupTool {
    type Params = AddMonitoredGroupParams;

    fn name(&self) -> &'static str {
        "add_monitored_group"
    }

    fn description(&self) -> &'static str {
        "Add a group (optionally a specific forum topic) to proactive monitoring."
    }

    fn run<'a>(
        &'a self,
        p: AddMonitoredGroupParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            if server.db.add_monitored_group(p.chat_id, p.thread_id).await? {
                let suffix = p.thread_id.map(|t| format!(" (topic {t})")).unwrap_or_default();
                Ok(format!("Group {}{suffix} added to monitoring", p.chat_id))
            } else {
                Ok(format!("Group {} is already monitored", p.chat_id))
            }
        })
    }
}
