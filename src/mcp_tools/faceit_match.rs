use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct FaceitMatchTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FaceitMatchParams {
    /// FACEIT match id.
    pub match_id: String,
}

impl Tool for FaceitMatchTool {
    type Params = FaceitMatchParams;

    fn name(&self) -> &'static str {
        "faceit_match"
    }

    fn description(&self) -> &'static str {
        "Detailed per-player scoreboard for a specific FACEIT CS2 match (K/D/A, HS%)."
    }

    fn run<'a>(
        &'a self,
        p: FaceitMatchParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let faceit = server.faceit.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "FACEIT API key not configured (set FACEIT_API_KEY env var)".into(),
                ))
            })?;
            faceit.get_match_details(&p.match_id).await
        })
    }
}
