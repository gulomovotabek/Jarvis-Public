use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsReadTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsReadParams {
    /// Google spreadsheet id (the long token in the spreadsheet URL).
    pub spreadsheet_id: String,
    /// A1-notation range, e.g. `Sheet1!A1:C10`.
    pub range: String,
    /// Optional value render option (e.g. `FORMATTED_VALUE`, `UNFORMATTED_VALUE`, `FORMULA`).
    #[serde(default)]
    pub value_render_option: Option<String>,
}

impl Tool for SheetsReadTool {
    type Params = SheetsReadParams;

    fn name(&self) -> &'static str {
        "sheets_read"
    }

    fn description(&self) -> &'static str {
        "Read a range of cells from a Google spreadsheet. Spreadsheet must be shared with the service account."
    }

    fn run<'a>(
        &'a self,
        p: SheetsReadParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets
                .read_range(&p.spreadsheet_id, &p.range, p.value_render_option.as_deref())
                .await
        })
    }
}
