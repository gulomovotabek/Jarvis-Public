use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct EditMemoryTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditMemoryParams {
    /// Memory path to edit.
    pub path: String,
    /// Version token from the most recent `read_memory` call for this path.
    /// Acts as a compare-and-swap guard: if the stored content has changed
    /// since that read, the edit is rejected and you should re-read first.
    pub expected_version: String,
    /// New content — replaces the existing content entirely.
    pub content: String,
}

impl Tool for EditMemoryTool {
    type Params = EditMemoryParams;

    fn name(&self) -> &'static str {
        "edit_memory"
    }

    fn description(&self) -> &'static str {
        "Edit an existing memory using compare-and-swap. You must first call read_memory \
         to obtain the current version token, then pass it as `expected_version`. \
         If another writer has changed the memory in the meantime, the edit fails and \
         you should re-read before retrying."
    }

    fn run<'a>(
        &'a self,
        params: EditMemoryParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            server
                .memory
                .edit(&params.path, &params.expected_version, &params.content)
                .await
        })
    }
}
