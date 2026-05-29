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
//!
//! R5 ("不打扰用户 / 不抢焦点") is enforced here: a notification only fires
//! when the Panel window is **not** focused. If the user is already looking
//! at the Panel an OS banner is pure noise — the in-Panel UI already shows
//! the prompt. The focus-gating and the long-turn-completion threshold live
//! in the pure [`decide_notification`] function so they are unit-testable
//! without a running window or daemon.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio_tungstenite::tungstenite::Message;

const WS_URL: &str = "ws://127.0.0.1:18790/ws";
const INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// A tool call is waiting for the user's approval. A real topic event
/// (`GatewayEventFrame::ApprovalRequested`, no `stream_method`), delivered
/// as `{"method":"event","params":{"topic":…,"data":…}}`.
const TOPIC_APPROVAL: &str = "approval.requested";
/// The agent asked the user a question. Published as a *streaming* frame
/// (`GatewayEventFrame::AskUser` → `stream.ask_user`), so the wire shape is
/// `{"method":"stream.ask_user","params":{…frame…}}` — NOT a topic event.
const TOPIC_ASK_USER: &str = "stream.ask_user";
/// A run finished. Streaming frame `stream.run_complete`, carrying
/// `total_duration_ms` so the completion notice can be gated on turn length
/// without the bridge tracking any run-start state of its own.
const TOPIC_RUN_COMPLETE: &str = "stream.run_complete";

/// EventBus topics worth interrupting the user for. These already flow on
/// the daemon's EventBus — the shell adds no new core topics (R1).
///
/// Both `stream.*` entries are streaming-frame methods: the panel subscribes
/// to the same `stream.*` names (it rewrites them to `run.*` locally). The
/// previous `agent.ask.user` topic never matched the `stream.ask_user`
/// method the daemon actually emits, so question notifications silently
/// never fired — fixed by subscribing to the real method name.
const NOTIFY_TOPICS: &[&str] = &[TOPIC_APPROVAL, TOPIC_ASK_USER, TOPIC_RUN_COMPLETE];

/// Minimum turn duration before a completed run is worth a desktop banner.
/// A two-second answer does not deserve an interruption; a two-minute build
/// does. Mirrors Reasonix's `COMPLETION_NOTIFY_MIN_MS`.
const COMPLETION_NOTIFY_MIN_MS: u64 = 15_000;

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

    let Some((topic, data)) = resolve_event(msg) else {
        return;
    };
    // Resolve focus once, lazily, only for a real event frame.
    if let Some(note) = decide_notification(&topic, &data, panel_focused(app)) {
        emit_notification(app, &note);
    }
}

/// Extract `(topic, data)` from a forwarded frame, accommodating both wire
/// shapes the Gateway uses:
///
/// - **Topic events** — `{"method":"event","params":{"topic":T,"data":D}}`
///   (e.g. `approval.requested`).
/// - **Streaming frames** — `{"method":"stream.<kind>","params":<frame>}`
///   (e.g. `stream.ask_user`, `stream.run_complete`). Here the method *is*
///   the topic and the params *are* the payload.
///
/// Anything else (connect/subscribe responses, unknown methods) yields
/// `None` so the caller ignores it without querying window focus.
fn resolve_event(msg: &Value) -> Option<(String, Value)> {
    let method = msg.get("method").and_then(Value::as_str)?;
    if method == "event" {
        let params = msg.get("params")?;
        let topic = params.get("topic").and_then(Value::as_str)?;
        let data = params.get("data").cloned().unwrap_or(Value::Null);
        Some((topic.to_string(), data))
    } else if method.starts_with("stream.") {
        Some((method.to_string(), msg.get("params").cloned().unwrap_or(Value::Null)))
    } else {
        None
    }
}

/// A notification the policy decided is worth showing.
struct PreparedNotification {
    title: &'static str,
    body: String,
}

/// Pure notification policy: given an event topic, its payload, and whether
/// the Panel window is currently focused, decide whether to interrupt the
/// user — and with what. Returns `None` to stay silent.
///
/// Two gates, both R5 ("don't disturb"):
/// 1. A focused window never produces an OS banner — the user is already
///    here and the in-Panel UI shows the prompt.
/// 2. A completed run only notifies once it has run at least
///    [`COMPLETION_NOTIFY_MIN_MS`]; quick turns are not worth a banner.
fn decide_notification(topic: &str, data: &Value, focused: bool) -> Option<PreparedNotification> {
    if focused {
        return None;
    }
    match topic {
        TOPIC_APPROVAL => Some(PreparedNotification {
            title: "Aleph needs your approval",
            body: extract_text(data).unwrap_or_else(|| "A tool call is waiting for you.".to_string()),
        }),
        TOPIC_ASK_USER => Some(PreparedNotification {
            title: "Aleph has a question",
            body: extract_text(data).unwrap_or_else(|| "Aleph is waiting for your reply.".to_string()),
        }),
        TOPIC_RUN_COMPLETE => {
            let duration_ms = data.get("total_duration_ms").and_then(Value::as_u64).unwrap_or(0);
            if duration_ms < COMPLETION_NOTIFY_MIN_MS {
                return None;
            }
            Some(PreparedNotification {
                title: "Aleph finished",
                body: "Your turn is complete.".to_string(),
            })
        }
        _ => None,
    }
}

/// Whether the Panel window is currently focused. An unknown answer (no
/// window yet, or the platform getter failing) is treated as **not** focused
/// so a genuine "needs you" event is never silently dropped — over-notifying
/// is cheaper than missing an approval while the user is away (R5).
fn panel_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false)
}

/// Show a prepared notification natively.
fn emit_notification(app: &AppHandle, note: &PreparedNotification) {
    if let Err(e) = app
        .notification()
        .builder()
        .title(note.title)
        .body(note.body.clone())
        .show()
    {
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
        // The streaming method names — not the legacy `agent.ask.user` topic
        // that never matched — must be present, or questions never notify.
        let names: Vec<&str> = topics.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&TOPIC_ASK_USER));
        assert!(names.contains(&TOPIC_RUN_COMPLETE));
        assert!(!names.contains(&"agent.ask.user"));
    }

    #[test]
    fn resolve_event_reads_topic_event_shape() {
        let msg = json!({
            "method": "event",
            "params": { "topic": "approval.requested", "data": { "approval_id": "a1" } },
        });
        let (topic, data) = resolve_event(&msg).expect("topic event resolves");
        assert_eq!(topic, "approval.requested");
        assert_eq!(data["approval_id"], "a1");
    }

    #[test]
    fn resolve_event_reads_streaming_frame_shape() {
        // Streaming frames put the payload directly in `params`.
        let msg = json!({
            "method": "stream.ask_user",
            "params": { "question": "Proceed?", "run_id": "r1" },
        });
        let (topic, data) = resolve_event(&msg).expect("stream frame resolves");
        assert_eq!(topic, "stream.ask_user");
        assert_eq!(data["question"], "Proceed?");
    }

    #[test]
    fn resolve_event_ignores_rpc_responses() {
        // A subscribe acknowledgement has no `method` — must be ignored.
        let resp = json!({ "jsonrpc": "2.0", "id": 2, "result": { "subscribed": [] } });
        assert!(resolve_event(&resp).is_none());
        // An unrelated method is ignored too.
        let other = json!({ "method": "pong" });
        assert!(resolve_event(&other).is_none());
    }

    #[test]
    fn focused_window_never_notifies() {
        // R5: every topic stays silent while the Panel is focused.
        for topic in NOTIFY_TOPICS {
            let data = json!({ "question": "q", "total_duration_ms": 999_999 });
            assert!(
                decide_notification(topic, &data, true).is_none(),
                "topic {topic} should be suppressed when focused"
            );
        }
    }

    #[test]
    fn approval_notifies_when_unfocused() {
        let note = decide_notification(TOPIC_APPROVAL, &json!({}), false).expect("approval fires");
        assert_eq!(note.title, "Aleph needs your approval");
        assert_eq!(note.body, "A tool call is waiting for you.");
    }

    #[test]
    fn ask_user_surfaces_the_question_text() {
        let data = json!({ "question": "Should I delete it?" });
        let note = decide_notification(TOPIC_ASK_USER, &data, false).expect("ask fires");
        assert_eq!(note.title, "Aleph has a question");
        assert_eq!(note.body, "Should I delete it?");
    }

    #[test]
    fn run_complete_is_gated_by_duration() {
        // A quick turn is not worth a banner.
        let quick = json!({ "total_duration_ms": COMPLETION_NOTIFY_MIN_MS - 1 });
        assert!(decide_notification(TOPIC_RUN_COMPLETE, &quick, false).is_none());
        // A long-running turn earns one.
        let slow = json!({ "total_duration_ms": COMPLETION_NOTIFY_MIN_MS });
        let note = decide_notification(TOPIC_RUN_COMPLETE, &slow, false).expect("long run fires");
        assert_eq!(note.title, "Aleph finished");
    }

    #[test]
    fn run_complete_without_duration_stays_silent() {
        // Missing duration → treated as 0 → below threshold → no banner.
        assert!(decide_notification(TOPIC_RUN_COMPLETE, &json!({}), false).is_none());
    }

    #[test]
    fn unknown_topic_is_ignored() {
        assert!(decide_notification("agent.tool.start", &json!({}), false).is_none());
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
