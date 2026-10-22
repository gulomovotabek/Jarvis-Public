use crate::mcp::{McpServer, web_fetch as run_fetch};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct WebFetchTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebFetchParams {
    /// Absolute URL to fetch.
    pub url: String,
    /// Optional map of extra HTTP headers to send with the request.
    #[serde(default)]
    pub headers: Option<Value>,
}

impl Tool for WebFetchTool {
    type Params = WebFetchParams;

    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "Fetch the contents of a URL (up to ~100KB, 30s timeout). Returns body + content-type."
    }

    fn run<'a>(
        &'a self,
        p: WebFetchParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { run_fetch(&p.url, p.headers.as_ref()).await })
    }
}
