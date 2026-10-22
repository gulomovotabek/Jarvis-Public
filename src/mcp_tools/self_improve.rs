use crate::config::parse_bool;
use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use crate::self_update::{new_job_id, self_improve_script, SelfUpdatePaths};
use anyhow::{anyhow, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

const MAX_TASK_CHARS: usize = 8000;

pub struct SelfImproveTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SelfImproveParams {
    /// The change to make in Jarvis's own source code, written as a concrete
    /// task for a coding agent: what behavior to add or fix, where, and how to
    /// verify it. The agent sees only this text and the repository.
    pub task: String,
}

pub(crate) fn start_job(mode: &str, task: &str, git_ref: Option<&str>) -> Result<String> {
    if !parse_bool("SELF_IMPROVE_ENABLED", false) {
        return Err(anyhow::Error::new(UserError(
            "self-improve is disabled (set SELF_IMPROVE_ENABLED=true in .env)".into(),
        )));
    }
    let paths = SelfUpdatePaths::from_env();
    let id = new_job_id();
    paths.write_job_state(&id, "queued")?;
    std::fs::write(paths.job_dir(&id).join("task.md"), task)?;
    let mut args = vec![mode, id.as_str()];
    args.extend(git_ref);
    if let Err(e) = paths.spawn_job(&self_improve_script(), &args) {
        let _ = paths.write_job_state(&id, &format!("failed: {e:#}"));
        return Err(e);
    }
    Ok(id)
}

impl Tool for SelfImproveTool {
    type Params = SelfImproveParams;

    fn name(&self) -> &'static str {
        "self_improve"
    }

    fn description(&self) -> &'static str {
        "Change Jarvis's own code. Starts a background job: a sandboxed coding agent \
         implements the task on a fresh jarvis/<job-id> branch, release build + tests + \
         MCP smoke test must pass, the branch is pushed to GitHub, the new binaries are \
         installed and Jarvis restarts into them. If the new build fails its health \
         probation after restart it is rolled back automatically. Returns the job id \
         immediately; progress arrives as owner DMs, details via self_improve_status. \
         Only call when the owner asked for a change to Jarvis itself."
    }

    fn run<'a>(
        &'a self,
        p: SelfImproveParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let task = p.task.trim();
            if task.is_empty() {
                return Err(anyhow!("task is empty"));
            }
            if task.chars().count() > MAX_TASK_CHARS {
                return Err(anyhow!("task too long (max {MAX_TASK_CHARS} chars)"));
            }
            let id = start_job("improve", task, None)?;
            Ok(format!(
                "Self-improve job {id} started on branch jarvis/{id}. \
                 It runs in the background (typically 5-30 min); the owner gets a DM on \
                 each milestone. Check progress with self_improve_status."
            ))
        })
    }
}
