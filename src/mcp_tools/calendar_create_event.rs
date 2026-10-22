use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use crate::mcp_tools::calendar_list_events::calendar_client;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct CalendarCreateEventTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CalendarCreateEventParams {
    /// Event title.
    pub summary: String,
    /// RFC3339 start, e.g. "2026-07-15T10:00:00+05:00".
    pub start: String,
    /// RFC3339 end, e.g. "2026-07-15T11:00:00+05:00".
    pub end: String,
    /// Optional event description/notes.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional location string.
    #[serde(default)]
    pub location: Option<String>,
    /// Calendar id. Defaults to GOOGLE_CALENDAR_ID from env.
    #[serde(default)]
    pub calendar_id: Option<String>,
}

impl Tool for CalendarCreateEventTool {
    type Params = CalendarCreateEventParams;

    fn name(&self) -> &'static str {
        "calendar_create_event"
    }

    fn description(&self) -> &'static str {
        "Create a Google Calendar event (title, start/end RFC3339, optional description/location)."
    }

    fn run<'a>(
        &'a self,
        p: CalendarCreateEventParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let cal = calendar_client(server)?;
            let calendar_id = cal.resolve_calendar_id(p.calendar_id.as_deref())?;
            cal.create_event(
                &calendar_id,
                &p.summary,
                p.description.as_deref(),
                p.location.as_deref(),
                &p.start,
                &p.end,
            )
            .await
        })
    }
}
