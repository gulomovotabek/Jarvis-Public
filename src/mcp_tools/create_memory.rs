use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct CreateMemoryTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateMemoryParams {
    /// Memory path (e.g. "notes/meeting.md"). Must be relative, no `..`.
    pub path: String,
    /// Memory content to store.
    pub content: String,
}

impl Tool for CreateMemoryTool {
    type Params = CreateMemoryParams;

    fn name(&self) -> &'static str {
        "create_memory"
    }

    fn description(&self) -> &'static str {
        "Create a new persistent memory entry at the given path. Fails if the path already exists."
    }

    fn run<'a>(
        &'a self,
        params: CreateMemoryParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { server.memory.create(&params.path, &params.content).await })
    }
}
