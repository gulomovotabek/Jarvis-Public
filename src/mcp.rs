use crate::db::Database;
use crate::focus::FocusManager;
use crate::memory::MemoryManager;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tracing::{error, info};

/// Explicit marker for errors whose message is safe to surface to the MCP client.
/// When present anywhere in the error chain, its text is returned verbatim.
/// Otherwise only the top-level `Display` of the error is shown (never the full
/// context chain, which can contain filesystem paths, SQL, or API URLs).
#[derive(Debug)]
pub struct UserError(pub String);

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UserError {}

/// Render an anyhow error for the MCP response without leaking internals.
fn render_mcp_error(err: &anyhow::Error) -> String {
    for cause in err.chain() {
        if let Some(ue) = cause.downcast_ref::<UserError>() {
            return ue.0.clone();
        }
    }
    // Top-level Display only — no `{e:#}` chain expansion.
    err.to_string()
}

pub struct McpServer {
    pub db: Database,
    pub memory: Arc<MemoryManager>,
    pub focus: Arc<FocusManager>,
    pub bot_token: Option<String>,
    pub bot_user_id: Option<i64>,
    #[cfg(feature = "faceit")]
    pub faceit: Option<crate::faceit::FaceitClient>,
    #[cfg(feature = "sheets")]
    pub google_sheets: Option<crate::google_sheets::GoogleSheetsClient>,
    #[cfg(feature = "calendar")]
    pub google_calendar: Option<crate::google_calendar::GoogleCalendarClient>,
    #[cfg(feature = "gmail")]
    pub gmail: Option<crate::gmail::GmailClient>,
    #[cfg(feature = "smtp")]
    pub smtp: Option<crate::smtp::SmtpClient>,
    /// Per-tool call/error counters + cumulative duration. Populated lazily on
    /// first call; entries are never removed.
    pub tool_metrics: Arc<ToolMetrics>,
}

/// Process-local per-tool metrics. Each tool has a small stats struct
/// (`ToolStat`) accumulated under a single `RwLock<HashMap>`.
#[derive(Default)]
pub struct ToolMetrics {
    stats: RwLock<HashMap<&'static str, ToolStat>>,
}

#[derive(Default)]
pub struct ToolStat {
    pub calls: AtomicU64,
    pub errors: AtomicU64,
    pub total_duration_ms: AtomicU64,
}

impl ToolMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a completed tool invocation. `is_error` is true if the tool
    /// returned Err. Duration is wall-clock time spent inside `ErasedTool::call`.
    pub fn record(&self, tool_name: &'static str, duration_ms: u64, is_error: bool) {
        // Fast path: read lock, stat already exists.
        {
            let guard = self.stats.read().unwrap_or_else(|e| e.into_inner());
            if let Some(stat) = guard.get(tool_name) {
                stat.calls.fetch_add(1, Ordering::Relaxed);
                stat.total_duration_ms
                    .fetch_add(duration_ms, Ordering::Relaxed);
                if is_error {
                    stat.errors.fetch_add(1, Ordering::Relaxed);
                }
                return;
            }
        }
        // Slow path: first call for this tool — insert under write lock.
        let mut guard = self.stats.write().unwrap_or_else(|e| e.into_inner());
        let stat = guard.entry(tool_name).or_default();
        stat.calls.fetch_add(1, Ordering::Relaxed);
        stat.total_duration_ms
            .fetch_add(duration_ms, Ordering::Relaxed);
        if is_error {
            stat.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Snapshot all recorded metrics as a sorted JSON array for the
    /// `get_tool_stats` tool.
    pub fn snapshot(&self) -> Value {
        let guard = self.stats.read().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<(&'static str, u64, u64, u64)> = guard
            .iter()
            .map(|(name, stat)| {
                (
                    *name,
                    stat.calls.load(Ordering::Relaxed),
                    stat.errors.load(Ordering::Relaxed),
                    stat.total_duration_ms.load(Ordering::Relaxed),
                )
            })
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let arr: Vec<Value> = rows
            .into_iter()
            .map(|(name, calls, errors, total_ms)| {
                let mean_ms = if calls == 0 { 0 } else { total_ms / calls };
                json!({
                    "name": name,
                    "calls": calls,
                    "errors": errors,
                    "total_duration_ms": total_ms,
                    "mean_duration_ms": mean_ms,
                })
            })
            .collect();
        json!({ "tools": arr })
    }
}

impl McpServer {
    /// Construct a teloxide Bot from the configured token. Returns a UserError
    /// when no token is set so the message reaches Claude verbatim.
    pub(crate) fn bot(&self) -> anyhow::Result<teloxide::Bot> {
        let token = self.bot_token.as_ref().ok_or_else(|| {
            anyhow::Error::new(UserError(
                "bot token not configured — set TELOXIDE_TOKEN".into(),
            ))
        })?;
        Ok(teloxide::Bot::new(token))
    }
}

impl McpServer {
    pub async fn run(self) -> Result<()> {
        use std::io::{BufRead, Write};

        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();

        for line_result in stdin.lock().lines() {
            let line = line_result.context("failed to read stdin")?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let request: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    error!("invalid JSON-RPC: {e}");
                    continue;
                }
            };

            let response = self.handle_request(&request).await;

            if let Some(resp) = response {
                let mut out = serde_json::to_string(&resp).unwrap();
                out.push('\n');
                stdout
                    .write_all(out.as_bytes())
                    .context("failed to write stdout")?;
                stdout.flush().context("failed to flush stdout")?;
            }
        }

        Ok(())
    }

    async fn handle_request(&self, request: &Value) -> Option<Value> {
        let method = request.get("method")?.as_str()?;
        let id = request.get("id");

        match method {
            "initialize" => {
                let result = json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "mcp-telegram",
                        "version": "0.2.0"
                    }
                });
                Some(jsonrpc_response(id, result))
            }
            "notifications/initialized" | "initialized" => None,
            "tools/list" => {
                let tools = crate::mcp_tools::list_definitions(crate::mcp_tools::global());
                Some(jsonrpc_response(id, json!({ "tools": tools })))
            }
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or(json!({}));
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));

                let result = self.call_tool(tool_name, &args).await;

                let content = match result {
                    Ok(text) => json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    }),
                    Err(e) => {
                        // Full chain goes to server logs; client sees only the
                        // top-level or an explicit UserError message.
                        error!(tool = tool_name, error = ?e, "mcp tool error");
                        let safe = render_mcp_error(&e);
                        json!({
                            "content": [{"type": "text", "text": format!("Error: {safe}")}],
                            "isError": true
                        })
                    }
                };

                Some(jsonrpc_response(id, content))
            }
            "ping" => Some(jsonrpc_response(id, json!({}))),
            _ => {
                info!("unhandled MCP method: {method}");
                Some(jsonrpc_error(id, -32601, &format!("Method not found: {method}")))
            }
        }
    }

    async fn call_tool(&self, name: &str, args: &Value) -> Result<String> {
        let Some(tool) = crate::mcp_tools::global().get(name) else {
            return Err(anyhow::Error::new(UserError(format!("unknown tool: {name}"))));
        };
        // Use the tool's own `&'static str` name so the HashMap key is stable
        // across calls and `ToolMetrics` doesn't leak allocations.
        let static_name: &'static str = tool.name();
        let start = std::time::Instant::now();
        let result = tool.call(args, self).await;
        let duration_ms = start.elapsed().as_millis() as u64;
        let is_err = result.is_err();
        self.tool_metrics.record(static_name, duration_ms, is_err);
        // Durable per-call record for the dashboard (best-effort).
        let err_str = result.as_ref().err().map(|e| e.to_string());
        let _ = self
            .db
            .insert_tool_call(static_name, !is_err, duration_ms as i64, err_str.as_deref())
            .await;
        result
    }
}


fn jsonrpc_response(id: Option<&Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "result": result
    })
}

fn jsonrpc_error(id: Option<&Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": {
            "code": code,
            "message": message
        }
    })
}

// ── Web tool implementations ──

const WEB_FETCH_MAX_BYTES: usize = 100 * 1024; // 100KB
const WEB_FETCH_TIMEOUT_SECS: u64 = 30;

pub(crate) async fn web_fetch(url: &str, headers: Option<&Value>) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .build()?;

    let mut request = client.get(url);

    if let Some(Value::Object(hdrs)) = headers {
        for (key, val) in hdrs {
            if let Some(v) = val.as_str() {
                request = request.header(key.as_str(), v);
            }
        }
    }

    let response = request.send().await?;
    let status = response.status();

    if !status.is_success() {
        return Ok(format!("HTTP {} {}", status.as_u16(), status.canonical_reason().unwrap_or("")));
    }

    let bytes = response.bytes().await?;
    let truncated = bytes.len() > WEB_FETCH_MAX_BYTES;
    let slice = if truncated { &bytes[..WEB_FETCH_MAX_BYTES] } else { &bytes[..] };
    let mut body = String::from_utf8_lossy(slice).to_string();
    if truncated {
        body.push_str("\n\n[...truncated at 100KB]");
    }
    Ok(body)
}

/// Web search with a backend chain: Brave (BRAVE_SEARCH_API_KEY) → Tavily
/// (TAVILY_API_KEY) → DuckDuckGo scrape. DDG anomaly-detection blocks
/// datacenter IPs, so on a VPS one of the keyed backends is effectively
/// required for this tool to work.
pub(crate) async fn web_search(query: &str, max_results: usize) -> anyhow::Result<String> {
    let mut errors: Vec<String> = Vec::new();

    if let Some(key) = non_empty_env("BRAVE_SEARCH_API_KEY") {
        match brave_search(query, max_results, &key).await {
            Ok(out) => return Ok(out),
            Err(e) => errors.push(format!("brave: {e}")),
        }
    }
    if let Some(key) = non_empty_env("TAVILY_API_KEY") {
        match tavily_search(query, max_results, &key).await {
            Ok(out) => return Ok(out),
            Err(e) => errors.push(format!("tavily: {e}")),
        }
    }
    match ddg_search(query, max_results).await {
        Ok(out) => Ok(out),
        Err(e) => {
            errors.push(format!("duckduckgo: {e}"));
            anyhow::bail!("all search backends failed: {}", errors.join("; "))
        }
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn format_search_results(results: &[(String, String, String)]) -> String {
    results
        .iter()
        .enumerate()
        .map(|(i, (title, url, snippet))| format!("{}. {}\n   {}\n   {}", i + 1, title, url, snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn brave_search(query: &str, max_results: usize, api_key: &str) -> anyhow::Result<String> {
    use anyhow::bail;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .build()?;
    let resp = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[("q", query), ("count", &max_results.to_string())])
        .header("X-Subscription-Token", api_key)
        .header("accept", "application/json")
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }
    let body: Value = resp.json().await?;
    let results: Vec<(String, String, String)> = body["web"]["results"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .take(max_results)
                .map(|item| {
                    (
                        strip_html_tags(item["title"].as_str().unwrap_or("")),
                        item["url"].as_str().unwrap_or("").to_string(),
                        strip_html_tags(item["description"].as_str().unwrap_or("")),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if results.is_empty() {
        return Ok(format!("No results found for: {query}"));
    }
    Ok(format_search_results(&results))
}

async fn tavily_search(query: &str, max_results: usize, api_key: &str) -> anyhow::Result<String> {
    use anyhow::bail;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .build()?;
    let resp = client
        .post("https://api.tavily.com/search")
        .json(&serde_json::json!({
            "api_key": api_key,
            "query": query,
            "max_results": max_results,
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }
    let body: Value = resp.json().await?;
    let results: Vec<(String, String, String)> = body["results"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .take(max_results)
                .map(|item| {
                    (
                        item["title"].as_str().unwrap_or("").to_string(),
                        item["url"].as_str().unwrap_or("").to_string(),
                        item["content"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if results.is_empty() {
        return Ok(format!("No results found for: {query}"));
    }
    Ok(format_search_results(&results))
}

async fn ddg_search(query: &str, max_results: usize) -> anyhow::Result<String> {
    use anyhow::bail;

    let browser_ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .user_agent(browser_ua)
        .cookie_store(true)
        .build()?;

    // Step 1: Get vqd token from DuckDuckGo
    let resp = client
        .get("https://duckduckgo.com/")
        .query(&[("q", query)])
        .send()
        .await?;
    let html = resp.text().await?;

    let vqd = extract_vqd(&html)
        .ok_or_else(|| anyhow::anyhow!("failed to get DuckDuckGo vqd token"))?;

    // Step 2: Fetch search results via internal JSON API
    let params: Vec<(&str, &str)> = vec![
        ("q", query),
        ("vqd", &vqd),
        ("kl", "wt-wt"),
        ("dl", "en"),
        ("ct", "US"),
        ("ss_mkt", "us"),
        ("df", ""),
        ("ex", "-1"),
        ("sp", "1"),
        ("bpa", "1"),
        ("biaexp", "b"),
        ("msvrtexp", "a"),
    ];
    let resp = client
        .get("https://links.duckduckgo.com/d.js")
        .query(&params)
        .header("referer", "https://duckduckgo.com/")
        .send()
        .await?;

    if !resp.status().is_success() {
        bail!("DuckDuckGo search failed: HTTP {}", resp.status());
    }

    let body = resp.text().await?;

    // Parse the JSONP-like response: extract JSON array from d.js response
    let json_str = if let Some(start) = body.find("DDG.pageLayout.load('d',") {
        let after = &body[start + 24..];
        if let Some(end) = after.find(");") {
            &after[..end]
        } else {
            &body
        }
    } else {
        // Try direct JSON parse
        &body
    };

    let parsed: Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => {
            // Fallback: try to find JSON array in the response
            let start = body.find('[').unwrap_or(0);
            let end = body.rfind(']').map(|i| i + 1).unwrap_or(body.len());
            match serde_json::from_str(&body[start..end]) {
                Ok(v) => v,
                Err(_) if body.contains("anomalyDetectionBlock") => {
                    bail!("DuckDuckGo bot-detection block (set BRAVE_SEARCH_API_KEY or TAVILY_API_KEY)")
                }
                Err(_) => bail!("failed to parse DuckDuckGo search results"),
            }
        }
    };

    let mut results = Vec::new();
    if let Some(items) = parsed.as_array() {
        for item in items {
            if results.len() >= max_results {
                break;
            }
            let url = item.get("u").and_then(|v| v.as_str()).unwrap_or("");
            let title = item.get("t").and_then(|v| v.as_str()).unwrap_or("");
            let snippet = item.get("a").and_then(|v| v.as_str()).unwrap_or("");

            // Skip ad/navigation entries
            if url.is_empty() || title.is_empty() {
                continue;
            }

            let clean_title = strip_html_tags(title);
            let clean_snippet = strip_html_tags(snippet);

            results.push(format!(
                "{}. {}\n   {}\n   {}",
                results.len() + 1,
                clean_title,
                url,
                clean_snippet,
            ));
        }
    }

    if results.is_empty() {
        Ok(format!("No results found for: {query}"))
    } else {
        Ok(results.join("\n\n"))
    }
}

/// Extract DuckDuckGo vqd token from page HTML.
fn extract_vqd(html: &str) -> Option<String> {
    // Pattern 1: vqd="TOKEN" or vqd='TOKEN'
    for prefix in ["vqd=\"", "vqd='", "vqd="] {
        if let Some(start) = html.find(prefix) {
            let after = &html[start + prefix.len()..];
            let end_char = if prefix.ends_with('"') {
                '"'
            } else if prefix.ends_with('\'') {
                '\''
            } else {
                '&'
            };
            if let Some(end) = after.find(end_char) {
                let token = after[..end].trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
            // If no end delimiter found, try newline/semicolon
            if let Some(end) = after.find(|c: char| c == '\n' || c == ';' || c == '&' || c == '"' || c == '\'') {
                let token = after[..end].trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

fn extract_between(html: &str, start_marker: &str, end_marker: &str) -> Option<String> {
    let start = html.find(start_marker)?;
    let after_marker = &html[start + start_marker.len()..];
    // Skip to after the next '>'
    let content_start = after_marker.find('>')? + 1;
    let content = &after_marker[content_start..];
    let end = content.find(end_marker)?;
    Some(content[..end].to_string())
}

pub(crate) fn strip_html_tags(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

// ── File download implementation ──

const DOWNLOAD_DIR: &str = "/tmp/jarvis_downloads";
const DOWNLOAD_MAX_BYTES: u64 = 50 * 1024 * 1024; // 50MB
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

pub(crate) async fn download_file(url: &str, filename: Option<&str>) -> anyhow::Result<String> {
    use tokio::io::AsyncWriteExt;

    // Ensure download directory exists
    tokio::fs::create_dir_all(DOWNLOAD_DIR).await
        .context("failed to create download directory")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?;

    let response = client.get(url).send().await
        .context("failed to send download request")?;

    if !response.status().is_success() {
        anyhow::bail!("HTTP {} {}", response.status().as_u16(),
            response.status().canonical_reason().unwrap_or(""));
    }

    let content_type = response.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let content_length = response.content_length().unwrap_or(0);
    if content_length > DOWNLOAD_MAX_BYTES {
        anyhow::bail!("file too large: {} bytes (max {} MB)",
            content_length, DOWNLOAD_MAX_BYTES / 1024 / 1024);
    }

    // Determine filename: explicit > Content-Disposition > URL path > fallback.
    // Any caller-supplied or header-supplied name is forced to a safe basename below.
    let raw_name = match filename {
        Some(name) => name.to_string(),
        None => extract_filename_from_response(&response, url),
    };
    let resolved_name = sanitize_filename(&raw_name);
    if resolved_name.is_empty() {
        anyhow::bail!("invalid filename after sanitization");
    }

    // Belt-and-suspenders: ensure the final path stays inside DOWNLOAD_DIR.
    let file_path = format!("{}/{}", DOWNLOAD_DIR, resolved_name);
    let canonical_dir = std::path::Path::new(DOWNLOAD_DIR);
    if !std::path::Path::new(&file_path).starts_with(canonical_dir) {
        anyhow::bail!("resolved path escaped download directory");
    }

    // Stream download to file with a running byte counter so a lying
    // Content-Length can't force us to buffer the whole body in memory before
    // the size check fires. `Response::chunk()` reads one chunk at a time
    // without requiring a `Stream` adapter.
    let mut file = tokio::fs::File::create(&file_path).await
        .context("failed to create output file")?;

    let mut response = response;
    let mut written: u64 = 0;
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(anyhow::anyhow!("failed to read response chunk: {e}")),
        };
        let next_total = written.saturating_add(chunk.len() as u64);
        if next_total > DOWNLOAD_MAX_BYTES {
            // Abort before committing the oversize bytes to disk. Remove the
            // partial file so we don't leave garbage under the download dir.
            drop(file);
            let _ = tokio::fs::remove_file(&file_path).await;
            anyhow::bail!(
                "file exceeded max size {} MB mid-stream",
                DOWNLOAD_MAX_BYTES / 1024 / 1024
            );
        }
        file.write_all(&chunk).await.context("failed to write chunk")?;
        written = next_total;
    }
    file.flush().await?;

    Ok(serde_json::to_string(&json!({
        "path": file_path,
        "size": written,
        "content_type": content_type,
        "filename": resolved_name
    }))?)
}

fn extract_filename_from_response(response: &reqwest::Response, url: &str) -> String {
    // Try Content-Disposition header
    if let Some(cd) = response.headers().get(reqwest::header::CONTENT_DISPOSITION) {
        if let Ok(cd_str) = cd.to_str() {
            // Look for filename="..." or filename*=UTF-8''...
            if let Some(start) = cd_str.find("filename=") {
                let after = &cd_str[start + 9..];
                let name = after.trim_start_matches('"').split('"').next()
                    .or_else(|| after.split(';').next())
                    .unwrap_or("")
                    .trim();
                if !name.is_empty() {
                    return sanitize_filename(name);
                }
            }
        }
    }

    // Try URL path
    if let Ok(parsed) = url::Url::parse(url) {
        if let Some(segments) = parsed.path_segments() {
            if let Some(last) = segments.last() {
                let decoded = percent_decode_str(last);
                if !decoded.is_empty() && decoded.contains('.') {
                    return sanitize_filename(&decoded);
                }
            }
        }
    }

    // Fallback: generate name from content type
    let ext = response.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| match ct.split(';').next().unwrap_or("").trim() {
            "application/pdf" => Some("pdf"),
            "application/epub+zip" => Some("epub"),
            "application/zip" => Some("zip"),
            "audio/mpeg" => Some("mp3"),
            "audio/ogg" => Some("ogg"),
            "video/mp4" => Some("mp4"),
            "image/jpeg" => Some("jpg"),
            "image/png" => Some("png"),
            "text/html" => Some("html"),
            "text/plain" => Some("txt"),
            _ => None,
        })
        .unwrap_or("bin");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("download_{ts}.{ext}")
}

fn percent_decode_str(s: &str) -> String {
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i+1..i+3]).unwrap_or(""),
                16,
            ) {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

fn sanitize_filename(name: &str) -> String {
    // Strip any directory components supplied by caller or headers.
    let basename = std::path::Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Replace shell/path-unsafe characters with '_' and drop leading dots to
    // avoid hidden files or `..` traversal. Also cap length to a sane bound.
    let cleaned: String = basename
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_start_matches('.').trim();
    let bounded: String = trimmed.chars().take(200).collect();
    if bounded.is_empty() || bounded == "." || bounded == ".." {
        return String::new();
    }
    bounded
}

#[cfg(test)]
mod sanitize_filename_tests {
    use super::sanitize_filename;

    #[test]
    fn strips_parent_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
    }

    #[test]
    fn strips_absolute_path() {
        assert_eq!(sanitize_filename("/etc/shadow"), "shadow");
    }

    #[test]
    fn rejects_dot_only() {
        assert_eq!(sanitize_filename("."), "");
        assert_eq!(sanitize_filename(".."), "");
    }

    #[test]
    fn replaces_unsafe_chars() {
        assert_eq!(sanitize_filename("a:b*c?.txt"), "a_b_c_.txt");
    }

    #[test]
    fn drops_leading_dots() {
        assert_eq!(sanitize_filename(".hidden"), "hidden");
    }

    #[test]
    fn strips_null_and_control() {
        assert_eq!(sanitize_filename("a\0b\nc"), "a_b_c");
    }

    #[test]
    fn preserves_normal() {
        assert_eq!(sanitize_filename("report-2026.pdf"), "report-2026.pdf");
    }
}

/// Create a teloxide InputFile from a string that can be a URL, local file path, or Telegram file_id.
pub(crate) fn make_input_file(value: &str) -> anyhow::Result<teloxide::types::InputFile> {
    use teloxide::types::InputFile;

    if value.starts_with("http://") || value.starts_with("https://") {
        Ok(InputFile::url(url::Url::parse(value).context("invalid URL")?))
    } else if value.starts_with('/') || value.starts_with("./") {
        // Local file path
        let path = std::path::Path::new(value);
        anyhow::ensure!(path.exists(), "local file not found: {value}");
        Ok(InputFile::file(path))
    } else {
        // Assume Telegram file_id
        Ok(InputFile::file_id(value))
    }
}
