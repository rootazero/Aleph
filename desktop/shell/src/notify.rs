//! OS-notification bridge.
//!
//! Subscribes to the daemon's `EventBus` over `ws://127.0.0.1:18790/ws` and
//! turns "the AI needs you" events into native desktop notifications — the
//! last mile of R5 ("AI comes to you"). Pure I/O: it forwards events, it
//! never interprets or acts on them.
//!
//! Best-effort by design. If the Gateway closes the connection or rejects the
//! handshake, the bridge logs a hint and the rest of the shell is entirely
//! unaffected.
//!
//! R5 ("Don't disturb user / Don't steal focus") is enforced here: a notification only fires
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
const MAX_BACKOFF: Duration = Duration::from_mins(1);

/// Core-decided approval banner, already scoped by the gateway to the session
/// it belongs to (`event_visibility`, same rule as the three raw `approval.*`
/// frames — it is NOT operator-only any more) and addressed to this desktop
/// surface. The raw `approval.requested` frame still drives the Panel card; the
/// shell only focus-gates and renders the core-supplied title/body.
///
/// The frame gained a `session_key` when that scoping landed. The shell does
/// not read it and deliberately does not start: this bridge renders what the
/// core decided to send, and a shell that began filtering by session would be a
/// second, weaker copy of a decision already made upstream.
const TOPIC_SURFACE_APPROVAL: &str = "surface.approval";

/// Core-decided R5 interrupt, already gated for interrupt-worthiness by the
/// gateway's R5 router and addressed to this desktop surface. The shell only
/// focus-gates and renders the core-supplied title/body.
const TOPIC_SURFACE_NOTIFY: &str = "surface.notify";

/// Topics this bridge subscribes to. The interrupt-worthiness policy for
/// run-completion / questions now lives in the core (it publishes
/// `surface.notify`); the shell no longer self-decides those.
const NOTIFY_TOPICS: &[&str] = &[TOPIC_SURFACE_NOTIFY, TOPIC_SURFACE_APPROVAL];

/// Run the bridge forever, reconnecting with exponential backoff.
pub async fn run_notification_bridge(app: AppHandle) {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        let target = crate::connection::load_target();
        match session(&app, &target).await {
            Ok(()) => backoff = INITIAL_BACKOFF,
            Err(e) => tracing::debug!("notification bridge disconnected: {e}"),
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// One connection: connect, handshake, subscribe, then forward events
/// until the socket closes or errors.
async fn session(
    app: &AppHandle,
    target: &crate::connection::ConnectionTarget,
) -> Result<(), String> {
    // NOTE (cert trust): `connect_async` validates `wss://` with the platform
    // TLS roots (native-tls, same stack as shared/client). It does NOT take
    // part in the workspace TOFU pin (`cert_trust::TrustStore`), which today
    // only hooks the webview's certificate challenge — so against a
    // self-signed remote Gateway this bridge fails the TLS handshake while the
    // Panel itself connects fine. CA-issued certificates work. Integrating
    // the WS client with TrustStore is deliberately deferred; see
    // `cert_trust/` for the pin model.
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_url(target))
        .await
        .map_err(|e| format!("connect failed: {e}"))?;

    // The Gateway rejects any first frame that is not `connect`.
    ws.send(Message::Text(connect_request(target).into()))
        .await
        .map_err(|e| format!("connect send failed: {e}"))?;
    ws.send(Message::Text(subscribe_request().into()))
        .await
        .map_err(|e| format!("subscribe send failed: {e}"))?;

    while let Some(frame) = ws.next().await {
        match frame.map_err(|e| format!("stream error: {e}"))? {
            Message::Text(text) => match serde_json::from_str::<Value>(text.as_str()) {
                Ok(value) => {
                    // A JSON-RPC error on our own `connect`/`subscribe` means the
                    // Gateway refused to authorize this bridge — typically a
                    // remote token-protected Gateway we have not (yet) presented
                    // a valid token to. End the session (Err, not a silent drop)
                    // so the caller backs off and reconnects, re-reading the
                    // token the Panel may have just deposited. Without this the
                    // bridge would block forever on a walled connection and R5
                    // banners would silently stop on remote.
                    if let Some(reason) = handshake_error(&value) {
                        return Err(format!(
                            "Gateway refused the desktop notification bridge ({reason}); \
                             present a valid Gateway token to enable desktop notifications"
                        ));
                    }
                    store_issued_device_token(&value);
                    handle_message(app, &value);
                }
                Err(e) => {
                    // A malformed frame may still carry event payload (approval
                    // titles/bodies, run summaries) — keep the raw text out of
                    // the warn log; it is available at trace level only.
                    tracing::warn!(
                        frame_len = text.len(),
                        head = &text[..text.len().min(80)],
                        "notification bridge: failed to parse JSON frame: {e}"
                    );
                    tracing::trace!(raw = text.as_str(), "notification bridge: raw bad frame");
                }
            },
            Message::Close(_) => return Ok(()),
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }
    Ok(())
}

/// If `msg` is a JSON-RPC error response to the bridge's own `connect` (id 1)
/// or `events.subscribe` (id 2) request, return its message. Both ids belong to
/// the bridge's handshake, so an error on either means the Gateway declined to
/// authorize this connection (login wall / `AUTH_REQUIRED`). Anything else —
/// success replies, forwarded events — yields `None`.
fn handshake_error(msg: &Value) -> Option<String> {
    let id = msg.get("id").and_then(Value::as_i64)?;
    if id != 1 && id != 2 {
        return None;
    }
    let error = msg.get("error")?;
    Some(
        error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("authorization refused")
            .to_string(),
    )
}

/// Build the `EventBus` WS URL for a target. Local → the loopback default;
/// Remote → `ws(s)://host:port/ws` derived from the target origin (https→wss).
fn ws_url(target: &crate::connection::ConnectionTarget) -> String {
    match target {
        crate::connection::ConnectionTarget::Local => WS_URL.to_string(),
        crate::connection::ConnectionTarget::Remote(url) => {
            let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
            let host = url.host_str().unwrap_or("127.0.0.1");
            // port_or_known_default(): the url crate elides scheme-default ports
            // (443/80) from `.port()`, so use the known-default form to recover
            // an https-on-443 remote correctly; parse() always sets a port, so
            // this is effectively always Some; the unwrap_or is belt-and-braces.
            let port = url.port_or_known_default().unwrap_or(18790);
            format!("{scheme}://{host}:{port}/ws")
        }
    }
}

/// Build the `connect` handshake request for `target`.
///
/// A Local (loopback) Gateway is operator under LAN-trust — the bridge connects
/// bare. A Remote Gateway runs the single-tier Gateway-token model: a remote
/// `connect` without a valid token is walled (`guest`, no scopes), so the bridge
/// presents the credential the Panel URL carried (`connection::load_gateway_token`).
/// Either way it declares its surface identity so the gateway routes
/// `surface.notify` (audience `["desktop"]`) and `surface.approval` to this
/// desktop surface — needed even for a remote, whose non-loopback client_ip
/// would otherwise be labelled Unknown and receive nothing.
fn connect_request(target: &crate::connection::ConnectionTarget) -> String {
    let mut params = json!({
        "device_name": "Aleph Desktop",
        "device_type": "desktop",
        "device_id": "aleph-desktop-shell",
        "channel_kind": "desktop",
    });
    if let crate::connection::ConnectionTarget::Remote(url) = target {
        // Only present a credential over an encrypted transport. The remote
        // Gateway exposes operator tokens via the same JSON-RPC frame the
        // bridge sends over `ws://` — sending one in clear is a credential
        // leak to network observers. If the operator really runs a remote
        // Gateway over http://, the bridge connects bare and the gateway
        // walls it (`guest`); the user gets a debug log explaining why.
        let scheme = url.scheme();
        if !matches!(scheme, "https" | "wss") {
            tracing::warn!(
                "notification bridge: remote target scheme {scheme:?} is unencrypted; \
                 skipping Gateway credential presentation and connecting as guest"
            );
        } else if let (Some(credential), Some(obj)) = (
            crate::connection::load_gateway_token(),
            params.as_object_mut(),
        ) {
            let key = if credential.starts_with("aleph-bt-") {
                "bootstrap_ticket"
            } else if credential.starts_with("aleph-dt-") {
                "device_token"
            } else {
                "token"
            };
            obj.insert(key.to_owned(), json!(credential));
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

/// If `msg` is the successful JSON-RPC response to the bridge's own `connect`
/// (id 1) and it carries a newly-issued `device_token`, persist it so future
/// reconnects use the long-lived token instead of a one-off bootstrap ticket.
fn store_issued_device_token(msg: &Value) {
    let id = msg.get("id").and_then(Value::as_i64);
    if id != Some(1) {
        return;
    }
    let Some(result) = msg.get("result") else {
        return;
    };
    let authorized = result
        .get("authorized")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !authorized {
        return;
    }
    if let Some(dt) = result.get("device_token").and_then(Value::as_str) {
        if let Err(e) = crate::connection::store_gateway_token(dt) {
            tracing::warn!("notification bridge: failed to persist issued device token: {e}");
        }
    }
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

/// Route one parsed JSON-RPC frame to a native notification. Handshake errors
/// (on `connect`/`subscribe`) are intercepted earlier by the session loop via
/// [`handshake_error`], which ends the session to reconnect — so here we only
/// ever see forwarded events and benign non-event replies.
fn handle_message(app: &AppHandle, msg: &Value) {
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
        Some((
            method.to_string(),
            msg.get("params").cloned().unwrap_or(Value::Null),
        ))
    } else {
        None
    }
}

/// A notification the policy decided is worth showing.
struct PreparedNotification {
    title: String,
    body: String,
}

/// Pure notification policy. The core already decided WHAT is worth
/// interrupting for (`surface.notify` is only published when worthy). The shell
/// keeps the one gate only it can apply: a focused Panel never produces an OS
/// banner (R5 — the in-Panel UI already shows the prompt).
fn decide_notification(topic: &str, data: &Value, focused: bool) -> Option<PreparedNotification> {
    if focused {
        return None;
    }
    match topic {
        TOPIC_SURFACE_NOTIFY => Some(PreparedNotification {
            title: data
                .get("title")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Aleph")
                .to_string(),
            body: data
                .get("body")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Aleph needs you.")
                .to_string(),
        }),
        // Approval banner now arrives via the gated delivery surface
        // (surface.approval). The core supplies title/body; the shell renders
        // it through the same path as surface.notify (Phase 2). The Panel card
        // still listens on approval.requested for the inbound approve/deny UI.
        TOPIC_SURFACE_APPROVAL => Some(PreparedNotification {
            title: data
                .get("title")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("Aleph needs your approval")
                .to_string(),
            body: data
                .get("body")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("A tool call is waiting for you.")
                .to_string(),
        }),
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
        .title(note.title.as_str())
        .body(note.body.clone())
        .show()
    {
        tracing::debug!("failed to show notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_request_is_well_formed() {
        let v: Value = serde_json::from_str(&connect_request(
            &crate::connection::ConnectionTarget::Local,
        ))
        .unwrap();
        assert_eq!(v["method"], "connect");
        assert_eq!(v["params"]["device_type"], "desktop");
        assert_eq!(v["params"]["channel_kind"], "desktop");
    }

    #[test]
    fn connect_request_local_carries_no_token() {
        // Loopback is operator under LAN-trust: the handshake stays bare.
        let v: Value = serde_json::from_str(&connect_request(
            &crate::connection::ConnectionTarget::Local,
        ))
        .unwrap();
        assert!(
            v["params"].get("token").is_none(),
            "Local connect must not carry a token (LAN-trust)"
        );
    }

    #[test]
    fn handshake_error_flags_connect_and_subscribe_failures() {
        // An error on the bridge's own connect (id 1) or subscribe (id 2)
        // means the Gateway walled the bridge — both must be flagged so the
        // session ends and reconnects with the (possibly fresh) token.
        for id in [1, 2] {
            let msg = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32001, "message": "Not authorized" },
            });
            assert_eq!(handshake_error(&msg).as_deref(), Some("Not authorized"));
        }
    }

    #[test]
    fn handshake_error_ignores_events_and_success() {
        // A forwarded event (no id) and an unrelated error id are not handshake
        // failures and must not tear the session down.
        let event = json!({ "method": "event", "params": { "topic": "x", "data": {} } });
        assert!(handshake_error(&event).is_none());
        let other = json!({ "id": 7, "error": { "message": "transient" } });
        assert!(handshake_error(&other).is_none());
        let ok = json!({ "id": 1, "result": { "authorized": true } });
        assert!(handshake_error(&ok).is_none());
    }

    #[test]
    fn subscribe_request_carries_notify_topics() {
        let v: Value = serde_json::from_str(&subscribe_request()).unwrap();
        assert_eq!(v["method"], "events.subscribe");
        let topics = v["params"]["topics"].as_array().unwrap();
        let names: Vec<&str> = topics.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&TOPIC_SURFACE_NOTIFY));
        assert!(names.contains(&TOPIC_SURFACE_APPROVAL));
        // Banner leg moved to the gated surface.approval; the shell no longer
        // subscribes the raw approval.requested (the Panel card still does).
        assert!(!names.contains(&"approval.requested"));
        // The raw stream topics moved to the core; the shell no longer subscribes.
        assert!(!names.contains(&"stream.run_complete"));
        assert!(!names.contains(&"stream.ask_user"));
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
        // R5: every subscribed topic stays silent while the Panel is focused.
        for topic in NOTIFY_TOPICS {
            let data = json!({ "title": "t", "body": "b", "message": "m" });
            assert!(
                decide_notification(topic, &data, true).is_none(),
                "topic {topic} should be suppressed when focused"
            );
        }
    }

    #[test]
    fn surface_notify_renders_core_supplied_title_body() {
        let data = json!({
            "type": "surface_notify",
            "audience": ["desktop"],
            "title": "Aleph finished",
            "body": "Your turn is complete."
        });
        let note = decide_notification(TOPIC_SURFACE_NOTIFY, &data, false)
            .expect("surface.notify fires when unfocused");
        assert_eq!(note.title, "Aleph finished");
        assert_eq!(note.body, "Your turn is complete.");
    }

    #[test]
    fn surface_notify_falls_back_on_missing_fields() {
        let note = decide_notification(TOPIC_SURFACE_NOTIFY, &json!({}), false).unwrap();
        assert_eq!(note.title, "Aleph");
        assert_eq!(note.body, "Aleph needs you.");
    }

    #[test]
    fn surface_approval_renders_core_supplied_title_body() {
        let data = json!({
            "type": "surface_approval",
            "audience": ["desktop"],
            "approval_id": "a1",
            "title": "Aleph needs your approval",
            "body": "A tool call is waiting for you."
        });
        let note = decide_notification(TOPIC_SURFACE_APPROVAL, &data, false)
            .expect("surface.approval fires when unfocused");
        assert_eq!(note.title, "Aleph needs your approval");
        assert_eq!(note.body, "A tool call is waiting for you.");
    }

    #[test]
    fn surface_approval_falls_back_on_missing_fields() {
        let note = decide_notification(TOPIC_SURFACE_APPROVAL, &json!({}), false).unwrap();
        assert_eq!(note.title, "Aleph needs your approval");
        assert_eq!(note.body, "A tool call is waiting for you.");
    }

    #[test]
    fn surface_approval_suppressed_when_focused() {
        let data = json!({ "title": "Aleph needs your approval", "body": "x" });
        assert!(decide_notification(TOPIC_SURFACE_APPROVAL, &data, true).is_none());
    }

    #[test]
    fn ws_url_local_is_the_loopback_constant() {
        assert_eq!(ws_url(&crate::connection::ConnectionTarget::Local), WS_URL);
    }

    #[test]
    fn ws_url_http_remote_uses_ws_scheme() {
        let t = crate::connection::ConnectionTarget::parse("10.0.0.5:9000").unwrap();
        assert_eq!(ws_url(&t), "ws://10.0.0.5:9000/ws");
    }

    #[test]
    fn ws_url_https_remote_uses_wss_scheme() {
        let t = crate::connection::ConnectionTarget::parse("https://gw.example.com").unwrap();
        assert_eq!(ws_url(&t), "wss://gw.example.com:18790/ws");
    }

    #[test]
    fn ws_url_https_remote_on_443_keeps_port() {
        let t = crate::connection::ConnectionTarget::parse("https://gw.example.com:443").unwrap();
        assert_eq!(ws_url(&t), "wss://gw.example.com:443/ws");
    }
}
