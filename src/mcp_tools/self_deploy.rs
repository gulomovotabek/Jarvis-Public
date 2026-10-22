use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::self_improve::start_job;
use crate::mcp_tools::Tool;
use crate::self_update::is_safe_git_ref;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SelfDeployTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SelfDeployParams {
    /// Branch on GitHub (origin) to build and deploy, e.g. "main" or
    /// "jarvis/20261018-101500-00aa".
    pub branch: String,
}

impl Tool for SelfDeployTool {
    type Params = SelfDeployParams;

    fn name(&self) -> &'static str {
        "self_deploy"
    }

    fn description(&self) -> &'static str {
        "Build and deploy an existing GitHub branch of Jarvis (e.g. \"main\" after the \
         owner merged, or a jarvis/* branch to roll back/forward). Same gates as \
         self_improve (release build, tests, MCP smoke test), then installs and restarts \
         with health probation + automatic rollback. Returns the job id immediately."
    }

    fn run<'a>(
        &'a self,
        p: SelfDeployParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let branch = p.branch.trim();
            if !is_safe_git_ref(branch) {
                return Err(anyhow::Error::new(UserError(format!("invalid branch name: {branch:?}"))));
            }
            let id = start_job("deploy", &format!("Deploy branch {branch}"), Some(branch))?;
            Ok(format!(
                "Deploy job {id} started for origin/{branch}. The owner gets a DM when it \
                 restarts or fails. Check progress with self_improve_status."
            ))
        })
    }
}
