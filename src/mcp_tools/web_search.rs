use crate::mcp::{McpServer, web_search as run_search};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct WebSearchTool;

fn default_max_results() -> u32 {
    5
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebSearchParams {
    /// Search query.
    pub query: String,
    /// Maximum number of results to return (default 5, capped at 10).
    #[serde(default = "default_max_results")]
    pub max_results: u32,
}

impl Tool for WebSearchTool {
    type Params = WebSearchParams;

    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Search the web and return snippet results. Backends in order: Brave, Tavily, DuckDuckGo (first configured/working one wins)."
    }

    fn run<'a>(
        &'a self,
        p: WebSearchParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let max = (p.max_results as usize).min(10).max(1);
            run_search(&p.query, max).await
        })
    }
}
