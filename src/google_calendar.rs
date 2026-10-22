use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const CALENDAR_BASE: &str = "https://www.googleapis.com/calendar/v3";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar";

#[derive(Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
}

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

/// Google Calendar client using the same service-account key file as the
/// Sheets integration. The target calendar must be shared with the service
/// account email (Settings → Share with specific people → editor role).
pub struct GoogleCalendarClient {
    client: reqwest::Client,
    service_account_email: String,
    encoding_key: jsonwebtoken::EncodingKey,
    token_cache: RwLock<Option<CachedToken>>,
    /// Default calendar id (GOOGLE_CALENDAR_ID) used when a tool call passes none.
    pub default_calendar_id: Option<String>,
}

impl GoogleCalendarClient {
    pub fn from_key_file(path: &str, default_calendar_id: Option<String>) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read service account key: {path}"))?;
        let key: ServiceAccountKey =
            serde_json::from_str(&data).context("failed to parse service account JSON key")?;
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
            default_calendar_id,
        })
    }

    async fn get_access_token(&self) -> Result<String> {
        {
            let cache = self.token_cache.read().await;
            if let Some(ref tok) = *cache {
                if Instant::now() < tok.expires_at {
                    return Ok(tok.access_token.clone());
                }
            }
        }

        let now = chrono::Utc::now().timestamp();
        let claims = json!({
            "iss": self.service_account_email,
            "scope": SCOPE,
            "aud": TOKEN_URL,
            "iat": now,
            "exp": now + 3600,
        });
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &self.encoding_key)
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
        {
            let mut cache = self.token_cache.write().await;
            *cache = Some(CachedToken { access_token: access_token.clone(), expires_at });
        }
        Ok(access_token)
    }

    pub fn resolve_calendar_id(&self, explicit: Option<&str>) -> Result<String> {
        explicit
            .map(|s| s.to_string())
            .or_else(|| self.default_calendar_id.clone())
            .context("no calendar id — pass calendar_id or set GOOGLE_CALENDAR_ID")
    }

    pub async fn list_events(
        &self,
        calendar_id: &str,
        time_min: &str,
        time_max: &str,
        max_results: usize,
    ) -> Result<String> {
        let token = self.get_access_token().await?;
        let url = format!(
            "{CALENDAR_BASE}/calendars/{}/events",
            urlencoding_encode(calendar_id)
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .query(&[
                ("timeMin", time_min),
                ("timeMax", time_max),
                ("singleEvents", "true"),
                ("orderBy", "startTime"),
                ("maxResults", &max_results.to_string()),
            ])
            .send()
            .await
            .context("calendar list request failed")?;
        let status = resp.status();
        let body: Value = resp.json().await.context("failed to parse calendar response")?;
        if !status.is_success() {
            bail!("calendar list failed ({status}): {}", api_error(&body));
        }
        let mut lines = Vec::new();
        if let Some(items) = body["items"].as_array() {
            for ev in items {
                let start = ev["start"]["dateTime"]
                    .as_str()
                    .or(ev["start"]["date"].as_str())
                    .unwrap_or("?");
                let end = ev["end"]["dateTime"]
                    .as_str()
                    .or(ev["end"]["date"].as_str())
                    .unwrap_or("?");
                lines.push(format!(
                    "• [{}] {} ({start} → {end}){}",
                    ev["id"].as_str().unwrap_or("?"),
                    ev["summary"].as_str().unwrap_or("(no title)"),
                    ev["location"]
                        .as_str()
                        .map(|l| format!(" @ {l}"))
                        .unwrap_or_default(),
                ));
            }
        }
        if lines.is_empty() {
            return Ok(format!("No events in {calendar_id} between {time_min} and {time_max}"));
        }
        Ok(lines.join("\n"))
    }

    pub async fn create_event(
        &self,
        calendar_id: &str,
        summary: &str,
        description: Option<&str>,
        location: Option<&str>,
        start_rfc3339: &str,
        end_rfc3339: &str,
    ) -> Result<String> {
        let token = self.get_access_token().await?;
        let url = format!(
            "{CALENDAR_BASE}/calendars/{}/events",
            urlencoding_encode(calendar_id)
        );
        let mut event = json!({
            "summary": summary,
            "start": {"dateTime": start_rfc3339},
            "end": {"dateTime": end_rfc3339},
        });
        if let Some(d) = description {
            event["description"] = json!(d);
        }
        if let Some(l) = location {
            event["location"] = json!(l);
        }
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(&event)
            .send()
            .await
            .context("calendar create request failed")?;
        let status = resp.status();
        let body: Value = resp.json().await.context("failed to parse calendar response")?;
        if !status.is_success() {
            bail!("calendar create failed ({status}): {}", api_error(&body));
        }
        Ok(format!(
            "created event [{}] {} — {}",
            body["id"].as_str().unwrap_or("?"),
            summary,
            body["htmlLink"].as_str().unwrap_or(""),
        ))
    }

    pub async fn delete_event(&self, calendar_id: &str, event_id: &str) -> Result<String> {
        let token = self.get_access_token().await?;
        let url = format!(
            "{CALENDAR_BASE}/calendars/{}/events/{}",
            urlencoding_encode(calendar_id),
            urlencoding_encode(event_id)
        );
        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .context("calendar delete request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            bail!("calendar delete failed ({status}): {}", api_error(&body));
        }
        Ok(format!("deleted event {event_id} from {calendar_id}"))
    }
}

fn api_error(body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .unwrap_or("unknown error")
        .to_string()
}

fn urlencoding_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
