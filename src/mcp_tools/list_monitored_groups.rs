use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct ListMonitoredGroupsTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListMonitoredGroupsParams {}

impl Tool for ListMonitoredGroupsTool {
    type Params = ListMonitoredGroupsParams;

    fn name(&self) -> &'static str {
        "list_monitored_groups"
    }

    fn description(&self) -> &'static str {
        "List all groups/topics currently under proactive monitoring."
    }

    fn run<'a>(
        &'a self,
        _p: ListMonitoredGroupsParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let groups = server.db.get_monitored_groups().await?;
            if groups.is_empty() {
                return Ok("No monitored groups.".to_string());
            }
            let out: Vec<_> = groups
                .iter()
                .map(|(cid, tid)| json!({"chat_id": cid, "thread_id": tid}))
                .collect();
            Ok(serde_json::to_string_pretty(&out)?)
        })
    }
}
