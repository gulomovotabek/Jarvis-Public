use crate::mcp::{McpServer, download_file as run_download};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct DownloadFileTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DownloadFileParams {
    /// Absolute URL to download.
    pub url: String,
    /// Optional filename to save as. Always sanitized to a basename — directory
    /// components are stripped. Auto-detected from Content-Disposition / URL
    /// path if omitted.
    #[serde(default)]
    pub filename: Option<String>,
}

impl Tool for DownloadFileTool {
    type Params = DownloadFileParams;

    fn name(&self) -> &'static str {
        "download_file"
    }

    fn description(&self) -> &'static str {
        "Download a file from a URL to local storage (max 50MB, 2min timeout). Returns local path for use with send_* tools."
    }

    fn run<'a>(
        &'a self,
        p: DownloadFileParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move { run_download(&p.url, p.filename.as_deref()).await })
    }
}
