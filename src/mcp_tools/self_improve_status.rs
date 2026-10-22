use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use crate::rag::truncate;
use crate::self_update::{is_job_id, SelfUpdatePaths, GIT_SHA};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

const RECENT_JOBS: usize = 10;
const LOG_TAIL_LINES: usize = 20;

pub struct SelfImproveStatusTool;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SelfImproveStatusParams {
    /// Job id returned by self_improve or self_deploy. Omit to list the most
    /// recent jobs.
    #[serde(default)]
    pub job_id: Option<String>,
}

fn first_line(text: Option<String>) -> String {
    text.as_deref().and_then(|t| t.lines().next()).map(|l| truncate(l.trim(), 120)).unwrap_or_default()
}

impl Tool for SelfImproveStatusTool {
    type Params = SelfImproveStatusParams;

    fn name(&self) -> &'static str {
        "self_improve_status"
    }

    fn description(&self) -> &'static str {
        "Status of self_improve / self_deploy jobs. With job_id: state, task, agent \
         summary and the log tail. Without: the running version and the 10 most recent jobs."
    }

    fn run<'a>(
        &'a self,
        p: SelfImproveStatusParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let paths = SelfUpdatePaths::from_env();
            let Some(id) = p.job_id.as_deref().map(str::trim).filter(|id| !id.is_empty()) else {
                let jobs = paths.recent_jobs(RECENT_JOBS);
                let mut out = format!("Running version: {GIT_SHA}\n");
                if jobs.is_empty() {
                    out.push_str("No self-improve jobs yet.");
                }
                for id in jobs {
                    let state = paths.read_job_file(&id, "state").unwrap_or_default();
                    let task = first_line(paths.read_job_file(&id, "task.md"));
                    out.push_str(&format!("{id} | {} | {task}\n", state.trim()));
                }
                return Ok(out);
            };
            if !is_job_id(id) || !paths.job_dir(id).is_dir() {
                return Err(anyhow::Error::new(UserError(format!("unknown job id: {id:?}"))));
            }
            let state = paths.read_job_file(id, "state").unwrap_or_default();
            let task = paths.read_job_file(id, "task.md").unwrap_or_default();
            let mut out = format!("Job {id}\nState: {}\nTask: {}\n", state.trim(), truncate(task.trim(), 1000));
            if let Some(summary) = paths.read_job_file(id, "summary.md").filter(|s| !s.trim().is_empty()) {
                out.push_str(&format!("\nAgent summary:\n{}\n", truncate(summary.trim(), 3000)));
            }
            if let Some(log) = paths.read_job_file(id, "log") {
                let lines: Vec<&str> = log.lines().collect();
                let tail = lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n");
                out.push_str(&format!("\nLog (last {LOG_TAIL_LINES} lines):\n{}\n", truncate(&tail, 4000)));
            }
            Ok(out)
        })
    }
}
