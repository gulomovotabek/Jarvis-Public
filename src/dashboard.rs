//! Operational dashboard served on the bot's existing axum server.
//!
//! Routes are fronted by an nginx vhost (status subdomain) that performs HTTP
//! basic auth and injects an `X-Dash-Auth: <secret>` header. Every route here
//! re-checks that header against `DASHBOARD_AUTH_SECRET`, so the routes stay
//! protected even if reached directly via the webhook host. When the secret is
//! unset the routes report as disabled (404).

use crate::bot::AppState;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json, Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/dash") }))
        .route("/dash", get(serve_dash))
        .route("/api/metrics", get(api_metrics))
        .route("/api/messages", get(api_messages))
        .route("/api/chats", get(api_chats))
        .route("/api/turns", get(api_turns))
        .route("/api/context", get(api_context))
        .route("/api/tools", get(api_tools))
        .route("/api/events", get(api_events))
        .route("/api/behavior", get(api_behavior))
        .with_state(state)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Gate on the injected `X-Dash-Auth` header. Returns an error response when the
/// dashboard is disabled (no secret) or the header is missing/wrong.
fn auth(headers: &HeaderMap, state: &AppState) -> Result<(), Response> {
    let Some(secret) = state.config.dashboard_auth_secret.as_deref() else {
        return Err((StatusCode::NOT_FOUND, "dashboard disabled").into_response());
    };
    let provided = headers
        .get("x-dash-auth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if constant_time_eq(provided.as_bytes(), secret.as_bytes()) {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "forbidden").into_response())
    }
}

fn err_json(e: anyhow::Error) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))).into_response()
}

async fn serve_dash(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    Html(include_str!("dashboard.html")).into_response()
}

async fn api_metrics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    let now = now_secs();
    let uptime = now.saturating_sub(state.started_at);
    let hb_epoch = state.claude.heartbeat_epoch() as i64;
    let hb_ago = if hb_epoch > 0 { now - hb_epoch } else { 0 };
    let alive = state.claude.is_alive();
    let busy_since = state.claude.busy_since();
    let busy_for = if busy_since > 0 { now.saturating_sub(busy_since as i64) } else { 0 };
    let activity_label = state.claude.activity_label();

    let heartbeats = state
        .db
        .get_recent_heartbeats(120)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(ts, source, details)| {
            json!({
                "ts": ts,
                "source": source,
                "details": details.and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok()),
            })
        })
        .collect::<Vec<_>>();

    Json(json!({
        "status": if alive { "ok" } else { "degraded" },
        "uptime_secs": uptime,
        "now": now,
        "claude": {
            "alive": alive,
            "last_heartbeat_secs_ago": hb_ago,
            "session_turns": state.claude.session_turn_count(),
            "session_max_turns": state.config.claude_session_max_turns,
            "busy": busy_since > 0,
            "busy_since": busy_since,
            "busy_for_secs": busy_for,
            "activity": activity_label,
        },
        "model": state.config.claude_code_model.clone().unwrap_or_else(|| "(cli default)".to_string()),
        "counters": state.metrics.to_json(),
        "context_window_tokens": state.config.context_window_tokens,
        "heartbeats": heartbeats,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct MessagesQuery {
    chat_id: Option<i64>,
    limit: Option<i64>,
    before_id: Option<i64>,
}

async fn api_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<MessagesQuery>,
) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    match state
        .db
        .dash_messages(q.chat_id, q.limit.unwrap_or(50), q.before_id)
        .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => err_json(e),
    }
}

async fn api_chats(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    match state.db.dash_chats().await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err_json(e),
    }
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<i64>,
}

async fn api_turns(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<LimitQuery>,
) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    let model = state.config.claude_code_model.as_deref();
    match state.db.dash_turns(q.limit.unwrap_or(200), model).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err_json(e),
    }
}

async fn api_context(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    match state.db.dash_context().await {
        Ok(mut v) => {
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "window_tokens".into(),
                    json!(state.config.context_window_tokens),
                );
            }
            Json(v).into_response()
        }
        Err(e) => err_json(e),
    }
}

async fn api_tools(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    match state.db.dash_tools().await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err_json(e),
    }
}

async fn api_events(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<LimitQuery>,
) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    match state.db.dash_events(q.limit.unwrap_or(50)).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err_json(e),
    }
}

async fn api_behavior(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = auth(&headers, &state) {
        return r;
    }
    match state.db.dash_behavior().await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err_json(e),
    }
}
