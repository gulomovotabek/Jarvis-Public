use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct ReadMemoryTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadMemoryParams {
    /// Memory path to read.
    pub path: String,
}

impl Tool for ReadMemoryTool {
    type Params = ReadMemoryParams;

    fn name(&self) -> &'static str {
        "read_memory"
    }

    fn description(&self) -> &'static str {
        "Read a memory entry by its path. Returns a version header and the content. \
         To edit this memory, pass the version back as `expected_version` to `edit_memory`; \
         the edit is rejected if another writer changed the memory in between."
    }

    fn run<'a>(
        &'a self,
        params: ReadMemoryParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let (version, content) = server.memory.read(&params.path).await?;
            Ok(format!("[memory-version: {version}]\n{content}"))
        })
    }
}
