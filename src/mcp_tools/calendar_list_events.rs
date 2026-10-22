use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct CalendarListEventsTool;

fn default_max_results() -> u32 {
    20
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CalendarListEventsParams {
    /// RFC3339 lower bound, e.g. "2026-07-14T00:00:00+05:00".
    pub time_min: String,
    /// RFC3339 upper bound, e.g. "2026-07-21T00:00:00+05:00".
    pub time_max: String,
    /// Calendar id (email-style). Defaults to GOOGLE_CALENDAR_ID from env.
    #[serde(default)]
    pub calendar_id: Option<String>,
    /// Maximum events to return (default 20, capped at 100).
    #[serde(default = "default_max_results")]
    pub max_results: u32,
}

pub(crate) fn calendar_client(server: &McpServer) -> Result<&crate::google_calendar::GoogleCalendarClient> {
    server.google_calendar.as_ref().ok_or_else(|| {
        anyhow::Error::new(UserError(
            "Google Calendar not configured (set GOOGLE_SERVICE_ACCOUNT_KEY and share the calendar with the service account)".into(),
        ))
    })
}

impl Tool for CalendarListEventsTool {
    type Params = CalendarListEventsParams;

    fn name(&self) -> &'static str {
        "calendar_list_events"
    }

    fn description(&self) -> &'static str {
        "List Google Calendar events in a time window. Calendar must be shared with the service account."
    }

    fn run<'a>(
        &'a self,
        p: CalendarListEventsParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let cal = calendar_client(server)?;
            let calendar_id = cal.resolve_calendar_id(p.calendar_id.as_deref())?;
            cal.list_events(
                &calendar_id,
                &p.time_min,
                &p.time_max,
                (p.max_results as usize).clamp(1, 100),
            )
            .await
        })
    }
}
