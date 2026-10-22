use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct DeleteMemoryTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteMemoryParams {
    /// Memory path to delete.
    pub path: String,
}

impl Tool for DeleteMemoryTool {
    type Params = DeleteMemoryParams;

    fn name(&self) -> &'static str {
        "delete_memory"
    }

    fn description(&self) -> &'static str {
        "Delete a memory entry by path. Irreversible."
    }

    fn run<'a>(
        &'a self,
        params: DeleteMemoryParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { server.memory.delete(&params.path).await })
    }
}
