use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct ListMemoriesTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListMemoriesParams {}

impl Tool for ListMemoriesTool {
    type Params = ListMemoriesParams;

    fn name(&self) -> &'static str {
        "list_memories"
    }

    fn description(&self) -> &'static str {
        "List all stored memory paths with their created/updated timestamps."
    }

    fn run<'a>(
        &'a self,
        _params: ListMemoriesParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { server.memory.list().await })
    }
}
