use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::debug;

const SHEETS_BASE: &str = "https://sheets.googleapis.com/v4/spreadsheets";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets";

#[derive(Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
}

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

pub struct GoogleSheetsClient {
    client: reqwest::Client,
    service_account_email: String,
    encoding_key: jsonwebtoken::EncodingKey,
    token_cache: RwLock<Option<CachedToken>>,
}

impl GoogleSheetsClient {
    pub fn from_key_file(path: &str) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read service account key: {path}"))?;
        let key: ServiceAccountKey = serde_json::from_str(&data)
            .context("failed to parse service account JSON key")?;
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(key.private_key.as_bytes())
            .context("failed to parse RSA private key from service account")?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            client,
            service_account_email: key.client_email,
            encoding_key,
            token_cache: RwLock::new(None),
        })
    }

    async fn get_access_token(&self) -> Result<String> {
        // Check cache
        {
            let cache = self.token_cache.read().await;
            if let Some(ref tok) = *cache {
                if Instant::now() < tok.expires_at {
                    return Ok(tok.access_token.clone());
                }
            }
        }

        // Mint new token
        let now = chrono::Utc::now().timestamp();
        let claims = json!({
            "iss": self.service_account_email,
            "scope": SCOPE,
            "aud": TOKEN_URL,
            "iat": now,
            "exp": now + 3600,
        });

        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(
            &header,
            &claims,
            &self.encoding_key,
        )
        .context("failed to sign JWT")?;

        let resp = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .context("token exchange request failed")?;

        let status = resp.status();
        let body: Value = resp.json().await.context("failed to parse token response")?;

        if !status.is_success() {
            let err_msg = body["error_description"]
                .as_str()
                .or(body["error"].as_str())
                .unwrap_or("unknown error");
            bail!("Google OAuth token exchange failed ({status}): {err_msg}");
        }

        let access_token = body["access_token"]
            .as_str()
            .context("no access_token in response")?
            .to_string();

        let expires_in = body["expires_in"].as_u64().unwrap_or(3600);
        let expires_at = Instant::now() + Duration::from_secs(expires_in.saturating_sub(60));

        // Update cache
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(CachedToken {
                access_token: access_token.clone(),
                expires_at,
            });
        }

        Ok(access_token)
    }

    async fn get(&self, url: &str) -> Result<Value> {
        let token = self.get_access_token().await?;
        debug!("Google Sheets GET {url}");
        let resp = self
            .client
            .get(url)
            .bearer_auth(&token)
            .send()
            .await
            .context("Google Sheets API request failed")?;
        self.parse_response(resp).await
    }

    async fn put_json(&self, url: &str, body: &Value) -> Result<Value> {
        let token = self.get_access_token().await?;
        debug!("Google Sheets PUT {url}");
        let resp = self
            .client
            .put(url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .context("Google Sheets API request failed")?;
        self.parse_response(resp).await
    }

    async fn post_json(&self, url: &str, body: &Value) -> Result<Value> {
        let token = self.get_access_token().await?;
        debug!("Google Sheets POST {url}");
        let resp = self
            .client
            .post(url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .context("Google Sheets API request failed")?;
        self.parse_response(resp).await
    }

    async fn parse_response(&self, resp: reqwest::Response) -> Result<Value> {
        let status = resp.status();
        let body: Value = resp.json().await.context("failed to parse response JSON")?;
        if !status.is_success() {
            let msg = body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            let code = body["error"]["status"]
                .as_str()
                .unwrap_or("");
            bail!("Google Sheets API error ({status} {code}): {msg}");
        }
        Ok(body)
    }

    /// Read cells from a range. Returns formatted text with the values.
    pub async fn read_range(
        &self,
        spreadsheet_id: &str,
        range: &str,
        render: Option<&str>,
    ) -> Result<String> {
        let encoded_range = url::form_urlencoded::byte_serialize(range.as_bytes()).collect::<String>();
        let render_opt = render.unwrap_or("FORMATTED_VALUE");
        let url = format!(
            "{SHEETS_BASE}/{spreadsheet_id}/values/{encoded_range}?valueRenderOption={render_opt}"
        );
        let data = self.get(&url).await?;

        let range_str = data["range"].as_str().unwrap_or(range);
        let values = data["values"].as_array();

        let mut out = format!("Range: {range_str}\n");
        match values {
            Some(rows) if !rows.is_empty() => {
                out.push_str(&format!("Rows: {}\n\n", rows.len()));
                for (i, row) in rows.iter().enumerate() {
                    let cells: Vec<&str> = row
                        .as_array()
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                        .unwrap_or_default();
                    out.push_str(&format!("Row {}: {}\n", i + 1, cells.join(" | ")));
                }
            }
            _ => {
                out.push_str("(empty — no data in range)\n");
            }
        }
        Ok(out)
    }

    /// Write values to a range (overwrites existing data).
    pub async fn write_range(
        &self,
        spreadsheet_id: &str,
        range: &str,
        values: &Value,
        input_option: Option<&str>,
    ) -> Result<String> {
        let encoded_range = url::form_urlencoded::byte_serialize(range.as_bytes()).collect::<String>();
        let input_opt = input_option.unwrap_or("USER_ENTERED");
        let url = format!(
            "{SHEETS_BASE}/{spreadsheet_id}/values/{encoded_range}?valueInputOption={input_opt}"
        );
        let body = json!({
            "range": range,
            "values": values,
        });
        let data = self.put_json(&url, &body).await?;

        let updated_range = data["updatedRange"].as_str().unwrap_or(range);
        let updated_rows = data["updatedRows"].as_u64().unwrap_or(0);
        let updated_cols = data["updatedColumns"].as_u64().unwrap_or(0);
        let updated_cells = data["updatedCells"].as_u64().unwrap_or(0);

        Ok(format!(
            "Updated {updated_cells} cells ({updated_rows} rows x {updated_cols} cols) in {updated_range}"
        ))
    }

    /// Append rows after the last row of data in a range.
    pub async fn append_rows(
        &self,
        spreadsheet_id: &str,
        range: &str,
        values: &Value,
        input_option: Option<&str>,
    ) -> Result<String> {
        let encoded_range = url::form_urlencoded::byte_serialize(range.as_bytes()).collect::<String>();
        let input_opt = input_option.unwrap_or("USER_ENTERED");
        let url = format!(
            "{SHEETS_BASE}/{spreadsheet_id}/values/{encoded_range}:append?valueInputOption={input_opt}&insertDataOption=INSERT_ROWS"
        );
        let body = json!({
            "range": range,
            "values": values,
        });
        let data = self.post_json(&url, &body).await?;

        let updates = &data["updates"];
        let updated_range = updates["updatedRange"].as_str().unwrap_or(range);
        let updated_rows = updates["updatedRows"].as_u64().unwrap_or(0);
        let updated_cells = updates["updatedCells"].as_u64().unwrap_or(0);

        Ok(format!(
            "Appended {updated_rows} rows ({updated_cells} cells) at {updated_range}"
        ))
    }

    /// Clear all values in a range (preserves formatting).
    pub async fn clear_range(
        &self,
        spreadsheet_id: &str,
        range: &str,
    ) -> Result<String> {
        let encoded_range = url::form_urlencoded::byte_serialize(range.as_bytes()).collect::<String>();
        let url = format!(
            "{SHEETS_BASE}/{spreadsheet_id}/values/{encoded_range}:clear"
        );
        let data = self.post_json(&url, &json!({})).await?;

        let cleared = data["clearedRange"].as_str().unwrap_or(range);
        Ok(format!("Cleared range: {cleared}"))
    }

    /// Get spreadsheet metadata: title, sheet names, row/column counts.
    pub async fn get_spreadsheet_info(&self, spreadsheet_id: &str) -> Result<String> {
        let url = format!(
            "{SHEETS_BASE}/{spreadsheet_id}?fields=properties,sheets.properties"
        );
        let data = self.get(&url).await?;

        let title = data["properties"]["title"].as_str().unwrap_or("(untitled)");
        let mut out = format!("Spreadsheet: {title}\nID: {spreadsheet_id}\n\nSheets:\n");

        if let Some(sheets) = data["sheets"].as_array() {
            for s in sheets {
                let props = &s["properties"];
                let name = props["title"].as_str().unwrap_or("?");
                let sheet_id = props["sheetId"].as_u64().unwrap_or(0);
                let rows = props["gridProperties"]["rowCount"].as_u64().unwrap_or(0);
                let cols = props["gridProperties"]["columnCount"].as_u64().unwrap_or(0);
                out.push_str(&format!(
                    "  - \"{name}\" (id: {sheet_id}, {rows} rows x {cols} cols)\n"
                ));
            }
        }
        Ok(out)
    }

    /// Add a new sheet/tab to the spreadsheet.
    pub async fn create_sheet(
        &self,
        spreadsheet_id: &str,
        title: &str,
    ) -> Result<String> {
        let url = format!("{SHEETS_BASE}/{spreadsheet_id}:batchUpdate");
        let body = json!({
            "requests": [{
                "addSheet": {
                    "properties": {
                        "title": title
                    }
                }
            }]
        });
        let data = self.post_json(&url, &body).await?;

        let reply = &data["replies"][0]["addSheet"]["properties"];
        let new_title = reply["title"].as_str().unwrap_or(title);
        let sheet_id = reply["sheetId"].as_u64().unwrap_or(0);

        Ok(format!("Created sheet \"{new_title}\" (id: {sheet_id})"))
    }

    /// Batch read multiple ranges in one API call.
    pub async fn batch_read(
        &self,
        spreadsheet_id: &str,
        ranges: &[String],
        render: Option<&str>,
    ) -> Result<String> {
        let render_opt = render.unwrap_or("FORMATTED_VALUE");
        let ranges_param: String = ranges
            .iter()
            .map(|r| {
                format!(
                    "ranges={}",
                    url::form_urlencoded::byte_serialize(r.as_bytes()).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        let url = format!(
            "{SHEETS_BASE}/{spreadsheet_id}/values:batchGet?{ranges_param}&valueRenderOption={render_opt}"
        );
        let data = self.get(&url).await?;

        let mut out = String::new();
        if let Some(value_ranges) = data["valueRanges"].as_array() {
            for vr in value_ranges {
                let range_str = vr["range"].as_str().unwrap_or("?");
                out.push_str(&format!("--- {range_str} ---\n"));
                if let Some(rows) = vr["values"].as_array() {
                    for (i, row) in rows.iter().enumerate() {
                        let cells: Vec<&str> = row
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();
                        out.push_str(&format!("Row {}: {}\n", i + 1, cells.join(" | ")));
                    }
                } else {
                    out.push_str("(empty)\n");
                }
                out.push('\n');
            }
        }
        Ok(out)
    }

    /// Batch write to multiple ranges in one API call.
    pub async fn batch_write(
        &self,
        spreadsheet_id: &str,
        data_ranges: &Value,
        input_option: Option<&str>,
    ) -> Result<String> {
        let input_opt = input_option.unwrap_or("USER_ENTERED");
        let url = format!("{SHEETS_BASE}/{spreadsheet_id}/values:batchUpdate");
        let body = json!({
            "valueInputOption": input_opt,
            "data": data_ranges,
        });
        let data = self.post_json(&url, &body).await?;

        let total_updated = data["totalUpdatedCells"].as_u64().unwrap_or(0);
        let total_rows = data["totalUpdatedRows"].as_u64().unwrap_or(0);
        let total_sheets = data["totalUpdatedSheets"].as_u64().unwrap_or(0);

        Ok(format!(
            "Batch updated {total_updated} cells across {total_rows} rows in {total_sheets} sheet(s)"
        ))
    }

    /// Delete a sheet/tab from the spreadsheet by sheet ID.
    pub async fn delete_sheet(
        &self,
        spreadsheet_id: &str,
        sheet_id: u64,
    ) -> Result<String> {
        let url = format!("{SHEETS_BASE}/{spreadsheet_id}:batchUpdate");
        let body = json!({
            "requests": [{
                "deleteSheet": {
                    "sheetId": sheet_id
                }
            }]
        });
        self.post_json(&url, &body).await?;
        Ok(format!("Deleted sheet with ID {sheet_id}"))
    }

    /// Create a brand new spreadsheet.
    pub async fn create_spreadsheet(&self, title: &str) -> Result<String> {
        let url = SHEETS_BASE.to_string();
        let body = json!({
            "properties": {
                "title": title
            }
        });
        let data = self.post_json(&url, &body).await?;

        let id = data["spreadsheetId"].as_str().unwrap_or("?");
        let url = data["spreadsheetUrl"].as_str().unwrap_or("?");
        Ok(format!("Created spreadsheet \"{title}\"\nID: {id}\nURL: {url}"))
    }
}
