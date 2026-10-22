use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

pub struct GetHeartbeatsTool;

fn default_limit() -> u32 {
    10
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetHeartbeatsParams {
    /// Max number of heartbeats to return (most recent first).
    #[serde(default = "default_limit")]
    pub limit: u32,
}

impl Tool for GetHeartbeatsTool {
    type Params = GetHeartbeatsParams;

    fn name(&self) -> &'static str {
        "get_heartbeats"
    }

    fn description(&self) -> &'static str {
        "Get recent heartbeat records for bot liveness monitoring."
    }

    fn run<'a>(
        &'a self,
        p: GetHeartbeatsParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let beats = server.db.get_recent_heartbeats(p.limit as usize).await?;
            if beats.is_empty() {
                return Ok("No heartbeats recorded.".to_string());
            }
            let out: Vec<_> = beats
                .iter()
                .map(|(ts, src, det)| json!({"timestamp": ts, "source": src, "details": det}))
                .collect();
            Ok(serde_json::to_string_pretty(&out)?)
        })
    }
}
