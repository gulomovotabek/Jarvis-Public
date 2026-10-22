use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsInfoTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsInfoParams {
    /// Google spreadsheet id.
    pub spreadsheet_id: String,
}

impl Tool for SheetsInfoTool {
    type Params = SheetsInfoParams;

    fn name(&self) -> &'static str {
        "sheets_info"
    }

    fn description(&self) -> &'static str {
        "Get spreadsheet metadata (title, sheet names, row/column counts)."
    }

    fn run<'a>(
        &'a self,
        p: SheetsInfoParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets.get_spreadsheet_info(&p.spreadsheet_id).await
        })
    }
}
