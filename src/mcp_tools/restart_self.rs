use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use crate::self_update::SelfUpdatePaths;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct RestartSelfTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RestartSelfParams {
    /// Short reason, shown in the owner's restart notice.
    #[serde(default)]
    pub reason: Option<String>,
}

impl Tool for RestartSelfTool {
    type Params = RestartSelfParams;

    fn name(&self) -> &'static str {
        "restart_self"
    }

    fn description(&self) -> &'static str {
        "Restart Jarvis's bot process on the currently installed binaries. Takes effect \
         after the current turn finishes; the owner gets a restart notice. For recovering \
         from a stuck state or reloading prompts — self_improve/self_deploy restart on their own."
    }

    fn run<'a>(
        &'a self,
        p: RestartSelfParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let reason = p
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .unwrap_or("requested by Jarvis");
            SelfUpdatePaths::from_env().request_restart(reason)?;
            Ok("Restart requested; the bot restarts as soon as the current turn finishes.".to_string())
        })
    }
}
