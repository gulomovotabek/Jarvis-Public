use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use crate::mcp_tools::calendar_list_events::calendar_client;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct CalendarDeleteEventTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CalendarDeleteEventParams {
    /// Event id (shown in calendar_list_events output).
    pub event_id: String,
    /// Calendar id. Defaults to GOOGLE_CALENDAR_ID from env.
    #[serde(default)]
    pub calendar_id: Option<String>,
}

impl Tool for CalendarDeleteEventTool {
    type Params = CalendarDeleteEventParams;

    fn name(&self) -> &'static str {
        "calendar_delete_event"
    }

    fn description(&self) -> &'static str {
        "Delete a Google Calendar event by id."
    }

    fn run<'a>(
        &'a self,
        p: CalendarDeleteEventParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let cal = calendar_client(server)?;
            let calendar_id = cal.resolve_calendar_id(p.calendar_id.as_deref())?;
            cal.delete_event(&calendar_id, &p.event_id).await
        })
    }
}
