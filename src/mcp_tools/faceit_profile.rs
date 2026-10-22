use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct FaceitProfileTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FaceitProfileParams {
    /// FACEIT nickname to look up.
    pub nickname: String,
}

impl Tool for FaceitProfileTool {
    type Params = FaceitProfileParams;

    fn name(&self) -> &'static str {
        "faceit_profile"
    }

    fn description(&self) -> &'static str {
        "Look up a FACEIT CS2 player profile by nickname (ELO, level, lifetime K/D, win rate, ADR)."
    }

    fn run<'a>(
        &'a self,
        p: FaceitProfileParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let faceit = server.faceit.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "FACEIT API key not configured (set FACEIT_API_KEY env var)".into(),
                ))
            })?;
            faceit.get_profile(&p.nickname).await
        })
    }
}
