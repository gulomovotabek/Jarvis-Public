use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SearchMemoriesTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchMemoriesParams {
    /// Substring to search for in memory paths or contents.
    pub query: String,
}

impl Tool for SearchMemoriesTool {
    type Params = SearchMemoriesParams;

    fn name(&self) -> &'static str {
        "search_memories"
    }

    fn description(&self) -> &'static str {
        "Search memories by substring match on path or content. Returns up to 20 matches."
    }

    fn run<'a>(
        &'a self,
        params: SearchMemoriesParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { server.memory.search(&params.query).await })
    }
}
