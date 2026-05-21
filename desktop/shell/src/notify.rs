//! OS-notification bridge.
//!
//! Subscribes to the daemon's EventBus over `ws://127.0.0.1:18790/ws` and
//! turns "the AI needs you" events into native desktop notifications — the
//! last mile of R5 ("AI comes to you"). Pure I/O: it forwards events, it
//! never interprets or acts on them.
//!
//! Best-effort by design. If the Gateway requires authentication and no
//! token is available, the bridge logs a hint and the rest of the shell is
//! entirely unaffected.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;
use tokio_tungstenite::tungstenite::Message;

const WS_URL: &str = "ws://127.0.0.1:18790/ws";
const INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// EventBus topics worth interrupting the user for. These already flow on
/// the daemon's EventBus — the shell adds no new core topics (R1).
const NOTIFY_TOPICS: &[&str] = &["approval.requested", "agent.ask.user"];

/// Run the bridge forever, reconnecting with exponential backoff.
pub async fn run_notification_bridge(app: AppHandle) {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match session(&app).await {
            Ok(()) => backoff = INITIAL_BACKOFF,
            Err(e) => tracing::debug!("notification bridge disconnected: {e}"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// One connection: connect, handshake, subscribe, then forward events
/// until the socket closes or errors.
async fn session(app: &AppHandle) -> Result<(), String> {
    let (mut ws, _) = tokio_tungstenite::connect_async(WS_URL)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;

    // The Gateway rejects any first frame that is not `connect`.
    ws.send(Message::Text(connect_request()))
        .await
        .map_err(|e| format!("connect send failed: {e}"))?;
    ws.send(Message::Text(subscribe_request()))
        .await
        .map_err(|e| format!("subscribe send failed: {e}"))?;

    while let Some(frame) = ws.next().await {
        match frame.map_err(|e| format!("stream error: {e}"))? {
            Message::Text(text) => {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    handle_message(app, &value);
                }
            }
            Message::Close(_) => return Ok(()),
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }
    Ok(())
}

/// Build the `connect` handshake request, including a token if one is
/// supplied via `ALEPH_GATEWAY_TOKEN`.
fn connect_request() -> String {
    let mut params = json!({
        "device_name": "Aleph Desktop",
        "device_type": "desktop",
        "device_id": "aleph-desktop-shell",
    });
    if let Ok(token) = std::env::var("ALEPH_GATEWAY_TOKEN") {
        if !token.is_empty() {
            params["shared_token"] = json!(token);
        }
    }
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "connect",
        "params": params,
    })
    .to_string()
}

/// Build the `events.subscribe` request for the notification topics.
fn subscribe_request() -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "events.subscribe",
        "params": { "topics": NOTIFY_TOPICS },
    })
    .to_string()
}

/// Route one parsed JSON-RPC frame: surface auth failures, forward events.
fn handle_message(app: &AppHandle, msg: &Value) {
    // An error on the `connect` request (id 1) almost always means the
    // Gateway requires authentication — log one helpful line.
    if let Some(error) = msg.get("error") {
        if msg.get("id").and_then(Value::as_i64) == Some(1) {
            tracing::warn!(
                "Gateway rejected the desktop shell connection ({}). \
                 Set ALEPH_GATEWAY_TOKEN to enable OS notifications.",
                // A closure, not the `Value::as_str` path: inside the
                // `tracing::warn!` expansion the bare `Value` name resolves
                // to `tracing::Value` (a trait), not `serde_json::Value`.
                error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("authentication error")
            );
        }
        return;
    }

    // Event notifications arrive as {"method":"event","params":{topic,data}}.
    if msg.get("method").and_then(Value::as_str) != Some("event") {
        return;
    }
    let Some(params) = msg.get("params") else {
        return;
    };
    if let Some(topic) = params.get("topic").and_then(Value::as_str) {
        let data = params.get("data").unwrap_or(&Value::Null);
        emit_notification(app, topic, data);
    }
}

/// Map an event to a native notification and show it.
fn emit_notification(app: &AppHandle, topic: &str, data: &Value) {
    let (title, body) = match topic {
        "approval.requested" => (
            "Aleph needs your approval",
            extract_text(data).unwrap_or_else(|| "A tool call is waiting for you.".to_string()),
        ),
        "agent.ask.user" => (
            "Aleph has a question",
            extract_text(data).unwrap_or_else(|| "Aleph is waiting for your reply.".to_string()),
        ),
        other => (
            "Aleph",
            extract_text(data).unwrap_or_else(|| other.to_string()),
        ),
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!("failed to show notification: {e}");
    }
}

/// Pull a human-readable line out of an arbitrary event payload.
fn extract_text(data: &Value) -> Option<String> {
    for key in [
        "message",
        "question",
        "text",
        "summary",
        "description",
        "prompt",
    ] {
        if let Some(s) = data.get(key).and_then(Value::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(truncate(trimmed, 180));
            }
        }
    }
    None
}

/// Truncate to `max` characters on a char boundary, appending an ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_request_is_well_formed() {
        let v: Value = serde_json::from_str(&connect_request()).unwrap();
        assert_eq!(v["method"], "connect");
        assert_eq!(v["params"]["device_type"], "desktop");
    }

    #[test]
    fn subscribe_request_carries_notify_topics() {
        let v: Value = serde_json::from_str(&subscribe_request()).unwrap();
        assert_eq!(v["method"], "events.subscribe");
        let topics = v["params"]["topics"].as_array().unwrap();
        assert_eq!(topics.len(), NOTIFY_TOPICS.len());
    }

    #[test]
    fn extract_text_prefers_message_then_falls_back() {
        let data = json!({ "message": "needs approval", "text": "ignored" });
        assert_eq!(extract_text(&data).as_deref(), Some("needs approval"));
        assert_eq!(extract_text(&json!({})), None);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 3), "hel…");
        // Multi-byte characters must not be split mid-codepoint.
        assert_eq!(truncate("日本語テスト", 3), "日本語…");
    }
}
