use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct GetToolStatsTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetToolStatsParams {}

impl Tool for GetToolStatsTool {
    type Params = GetToolStatsParams;

    fn name(&self) -> &'static str {
        "get_tool_stats"
    }

    fn description(&self) -> &'static str {
        "Return per-tool call count, error count, and cumulative/mean duration for every tool that has been invoked since the MCP server started. Sorted by call count descending."
    }

    fn run<'a>(
        &'a self,
        _p: GetToolStatsParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let snapshot = server.tool_metrics.snapshot();
            Ok(serde_json::to_string_pretty(&snapshot)?)
        })
    }
}
