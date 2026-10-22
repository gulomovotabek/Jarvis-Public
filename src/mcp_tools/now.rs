use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct NowTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NowParams {}

impl Tool for NowTool {
    type Params = NowParams;

    fn name(&self) -> &'static str {
        "now"
    }

    fn description(&self) -> &'static str {
        "Get current Unix timestamp (seconds since epoch)."
    }

    fn run<'a>(
        &'a self,
        _params: NowParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_default();
            Ok(now.to_string())
        })
    }
}
