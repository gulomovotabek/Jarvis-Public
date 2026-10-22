use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct RemoveMonitoredGroupTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RemoveMonitoredGroupParams {
    /// Chat id of the group to stop monitoring.
    pub chat_id: i64,
    /// Optional topic id to remove (otherwise removes the whole group).
    #[serde(default)]
    pub thread_id: Option<i64>,
}

impl Tool for RemoveMonitoredGroupTool {
    type Params = RemoveMonitoredGroupParams;

    fn name(&self) -> &'static str {
        "remove_monitored_group"
    }

    fn description(&self) -> &'static str {
        "Remove a group from proactive monitoring."
    }

    fn run<'a>(
        &'a self,
        p: RemoveMonitoredGroupParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            if server.db.remove_monitored_group(p.chat_id, p.thread_id).await? {
                Ok(format!("Group {} removed from monitoring", p.chat_id))
            } else {
                Ok(format!("Group {} was not monitored", p.chat_id))
            }
        })
    }
}
