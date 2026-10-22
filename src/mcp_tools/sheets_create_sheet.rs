use crate::mcp::{McpServer, UserError};
use crate::mcp_tools::Tool;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;

pub struct SheetsCreateSheetTool;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SheetsCreateSheetParams {
    /// Spreadsheet id to add the sheet to.
    pub spreadsheet_id: String,
    /// Title of the new sheet/tab.
    pub title: String,
}

impl Tool for SheetsCreateSheetTool {
    type Params = SheetsCreateSheetParams;

    fn name(&self) -> &'static str {
        "sheets_create_sheet"
    }

    fn description(&self) -> &'static str {
        "Add a new sheet/tab to an existing spreadsheet."
    }

    fn run<'a>(
        &'a self,
        p: SheetsCreateSheetParams,
        server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let sheets = server.google_sheets.as_ref().ok_or_else(|| {
                anyhow::Error::new(UserError(
                    "Google Sheets not configured (set GOOGLE_SERVICE_ACCOUNT_KEY env var)".into(),
                ))
            })?;
            sheets.create_sheet(&p.spreadsheet_id, &p.title).await
        })
    }
}
