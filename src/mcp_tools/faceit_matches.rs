use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct FaceitMatchesTool;

fn default_limit() -> u32 {
    5
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FaceitMatchesParams {
    /// FACEIT nickname.
    pub nickname: String,
    /// Number of recent matches to return (default 5, max 20).
    #[serde(default = "default_limit")]
    pub limit: u32,
}

impl Tool for FaceitMatchesTool {
    type Params = FaceitMatchesParams;

    fn name(&self) -> &'static str {
        "faceit_matches"
    }

    fn description(&self) -> &'static str {
        "Recent CS2 match history for a FACEIT player (W/L, scores, durations, match IDs)."
    }

    fn run<'a>(
        &'a self,
        p: FaceitMatchesParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let faceit = server.faceit.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "FACEIT API key not configured (set FACEIT_API_KEY env var)".into(),
                ))
            })?;
            let limit = (p.limit as usize).clamp(1, 20);
            faceit.get_matches(&p.nickname, limit).await
        })
    }
}
