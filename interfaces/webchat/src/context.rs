use crate::api::ExecApprovalApi;
use crate::components::sidebar::SystemAlert;
use crate::state::notifications::{PendingApprovalView, PendingAskView};
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, StreamExt};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use shared_ui_logic::connection::connector::AlephConnector;
use shared_ui_logic::connection::wasm::WasmConnector;
use shared_ui_logic::connection::{
    classify, stage_for_connect_error, ConnectionError, ConnectionFailure, FailureStage,
    OriginLiveness,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

const WS_OPEN_TIMEOUT_MS: u32 = 8_000;

// RPC request sent to the message loop
struct RpcRequest {
    id: String,
    method: String,
    params: Value,
    response_tx: oneshot::Sender<Result<Value, String>>,
}

// Event received from Gateway
#[derive(Clone, Debug)]
pub struct GatewayEvent {
    pub topic: String,
    pub data: Value,
}

// Event handler callback type
type EventHandler = Arc<dyn Fn(GatewayEvent) + Send + Sync>;

/// Pure predicate for the single-tier Gateway-token model: an authorized
/// connection reports the `"operator"` role (loopback, or a remote that
/// presented a valid token) and has full local-equivalent authority. Anything
/// else is unauthorized (login wall). Extracted for host-side unit testing.
pub(crate) fn role_is_operator(role: Option<&str>) -> bool {
    role == Some("operator")
}

/// localStorage key holding the legacy shared Gateway token. Kept for backward
/// compatibility with old `?token=` links and manually-entered tokens.
/// This is a key name, not a credential value.
// rust-doctor-disable-next-line hardcoded-secrets
#[cfg(target_arch = "wasm32")]
const GATEWAY_LEGACY_TOKEN_KEY: &str = "aleph_gateway_token";

/// localStorage key holding the long-lived device token issued after a
/// bootstrap ticket is exchanged.
/// This is a key name, not a credential value.
// rust-doctor-disable-next-line hardcoded-secrets
#[cfg(target_arch = "wasm32")]
const GATEWAY_DEVICE_TOKEN_KEY: &str = "aleph_device_token";

/// Credentials to present during the `connect` handshake.
/// Priority: bootstrap ticket (one-time) > device token > legacy shared token.
#[derive(Debug, Default, Clone)]
struct ConnectCredentials {
    bootstrap_ticket: Option<String>,
    device_token: Option<String>,
    legacy_token: Option<String>,
}

impl ConnectCredentials {
    fn is_empty(&self) -> bool {
        self.bootstrap_ticket.is_none()
            && self.device_token.is_none()
            && self.legacy_token.is_none()
    }
}

/// Read credentials to present at the `connect` handshake.
/// `?bt=` and `?token=` URL queries win over persisted localStorage values.
#[cfg(target_arch = "wasm32")]
fn read_connect_credentials() -> ConnectCredentials {
    let win = match web_sys::window() {
        Some(w) => w,
        None => return ConnectCredentials::default(),
    };

    let search = win.location().search().unwrap_or_default();
    let bootstrap_ticket = parse_query_param(&search, "bt=");
    let url_legacy_token = parse_query_param(&search, "token=");

    let storage = win.local_storage().ok().flatten();

    let device_token = storage
        .as_ref()
        .and_then(|s| s.get_item(GATEWAY_DEVICE_TOKEN_KEY).ok().flatten())
        .filter(|v| !v.is_empty());

    let legacy_token = url_legacy_token.or_else(|| {
        storage
            .as_ref()
            .and_then(|s| s.get_item(GATEWAY_LEGACY_TOKEN_KEY).ok().flatten())
            .filter(|v| !v.is_empty())
    });

    ConnectCredentials {
        bootstrap_ticket,
        device_token,
        legacy_token,
    }
}

/// Extract a query parameter value from a `?a=1&b=2…` query string.
/// The values we care about (`aleph-…` tokens) contain no URL-special
/// characters, so no percent-decoding is needed. Machine-format, regex-free.
#[cfg(target_arch = "wasm32")]
fn parse_query_param(search: &str, prefix: &str) -> Option<String> {
    let q = search.strip_prefix('?').unwrap_or(search);
    q.split('&')
        .find_map(|pair| pair.strip_prefix(prefix))
        .filter(|v| !v.is_empty())
        .map(std::string::ToString::to_string)
}

/// Drop listed query params from a `?…` string, returning the remaining query
/// (no leading `?`). Empty when all params were stripped.
///
/// Deliberately **not** cfg-gated. It used to be `cfg(any(wasm32, test))`, but
/// `views::memory` imports it unconditionally — so the host (non-wasm, non-test)
/// build of this crate did not compile, which in turn meant `cargo test -p
/// aleph-panel` could not build the lib and **every host unit test in this crate
/// silently stopped running**. Pure string logic has no reason to be gated.
pub(crate) fn strip_params(search: &str, prefixes: &[&str]) -> String {
    let q = search.strip_prefix('?').unwrap_or(search);
    q.split('&')
        .filter(|pair| !pair.is_empty() && !prefixes.iter().any(|prefix| pair.starts_with(prefix)))
        .collect::<Vec<_>>()
        .join("&")
}

/// What kind of credential the operator just pasted into the login wall.
///
/// The three prefixes are the wire contract with the server (`aleph-bt-*` is
/// minted by `gateway.ticket.create` / `aleph-server pair`, `aleph-dt-*` by the
/// bootstrap exchange, bare `aleph-*` is the shared token), so classification is
/// a pure prefix match — kept in one place because getting it wrong sends the
/// value in the wrong `connect` field and the server rejects a perfectly good
/// credential.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum SubmittedCredential {
    /// One-time pairing ticket. Not persisted: it is consumed by the very next
    /// `connect`, which hands back a device token to persist instead.
    BootstrapTicket,
    /// Long-lived per-device token from a previous pairing.
    DeviceToken,
    /// Legacy shared Gateway token.
    SharedToken,
}

pub(crate) fn classify_credential(token: &str) -> SubmittedCredential {
    if token.starts_with("aleph-bt-") {
        SubmittedCredential::BootstrapTicket
    } else if token.starts_with("aleph-dt-") {
        SubmittedCredential::DeviceToken
    } else {
        SubmittedCredential::SharedToken
    }
}

/// Rewrite the query string to carry a bootstrap ticket, preserving any other
/// params. Returns the query without a leading `?`.
///
/// A pasted ticket takes the same route as a scanned QR (`?bt=`) instead of
/// getting its own storage + handshake path: one credential, one code path.
/// Stale `token=` / `bt=` values are dropped first, since
/// `read_connect_credentials` prefers the URL over localStorage and an expired
/// leftover would shadow what the operator just typed.
// Only `submit_token`'s wasm arm calls it (the host build has no `location`),
// but the logic is pure so it stays compiled and unit-tested on the host.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn query_with_bootstrap_ticket(search: &str, ticket: &str) -> String {
    let remaining = strip_params(search, &["token=", "bt="]);
    if remaining.is_empty() {
        format!("bt={ticket}")
    } else {
        format!("{remaining}&bt={ticket}")
    }
}

/// Persist a validated device token so refreshes / reconnects stay authorized.
#[cfg(target_arch = "wasm32")]
fn persist_device_token(token: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(GATEWAY_DEVICE_TOKEN_KEY, token);
    }
}

/// Persist a validated legacy shared token for backward compatibility.
#[cfg(target_arch = "wasm32")]
fn persist_legacy_token(token: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(GATEWAY_LEGACY_TOKEN_KEY, token);
    }
}

/// Scrub `?token=` and `?bt=` from the address bar (no reload) once the
/// credential is safely in localStorage. No-op when the query carries neither.
#[cfg(target_arch = "wasm32")]
fn scrub_credentials_from_url() {
    let Some(win) = web_sys::window() else { return };
    let Ok(search) = win.location().search() else {
        return;
    };
    if !search.contains("token=") && !search.contains("bt=") {
        return;
    }
    let Ok(history) = win.history() else { return };
    let pathname = win
        .location()
        .pathname()
        .unwrap_or_else(|_| "/".to_string());
    let remaining = strip_params(&search, &["token=", "bt="]);
    let new_url = if remaining.is_empty() {
        pathname
    } else {
        format!("{pathname}?{remaining}")
    };
    let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&new_url));
}

/// Navigate to `?bt=<ticket>`, which reloads the page so the fresh `connect`
/// presents the pairing ticket — the same route a scanned QR takes.
#[cfg(target_arch = "wasm32")]
fn redirect_with_bootstrap_ticket(ticket: &str) {
    if let Some(w) = web_sys::window() {
        let search = w.location().search().unwrap_or_default();
        let _ = w
            .location()
            .set_search(&query_with_bootstrap_ticket(&search, ticket));
    }
}

/// Reload after persisting a credential.
///
/// Stale `?token=` / `?bt=` are dropped from the URL **first**:
/// `read_connect_credentials` prefers the URL over localStorage, so an expired
/// link would otherwise shadow what the operator just entered and re-trip the
/// login wall on every reload.
#[cfg(target_arch = "wasm32")]
fn scrub_credentials_and_reload() {
    scrub_credentials_from_url();
    if let Some(w) = web_sys::window() {
        let _ = w.location().reload();
    }
}

/// Drop all persisted credentials. Called when authentication is rejected so
/// the login box starts empty on the next load.
#[cfg(target_arch = "wasm32")]
fn clear_credentials() {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.remove_item(GATEWAY_DEVICE_TOKEN_KEY);
        let _ = s.remove_item(GATEWAY_LEGACY_TOKEN_KEY);
    }
}

/// Drop only the legacy shared token.
#[cfg(target_arch = "wasm32")]
fn clear_legacy_token() {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.remove_item(GATEWAY_LEGACY_TOKEN_KEY);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_legacy_token() {}

#[cfg(not(target_arch = "wasm32"))]
fn read_connect_credentials() -> ConnectCredentials {
    ConnectCredentials::default()
}

#[cfg(not(target_arch = "wasm32"))]
fn persist_device_token(_token: &str) {}

#[cfg(not(target_arch = "wasm32"))]
fn persist_legacy_token(_token: &str) {}

#[cfg(not(target_arch = "wasm32"))]
fn clear_credentials() {}

#[cfg(not(target_arch = "wasm32"))]
fn scrub_credentials_from_url() {}

#[cfg(not(target_arch = "wasm32"))]
fn redirect_with_bootstrap_ticket(_ticket: &str) {}

#[cfg(not(target_arch = "wasm32"))]
fn scrub_credentials_and_reload() {}

enum Handshake {
    Authorized,
    NeedsToken { was_rejected: bool },
    Failed(ConnectionFailure),
}

#[derive(Clone, Copy)]
pub struct DashboardState {
    pub is_connected: RwSignal<bool>,
    pub reconnect_count: RwSignal<u32>,
    pub gateway_url: RwSignal<String>,
    pub connection_error: RwSignal<Option<String>>,
    pub is_reconnecting: RwSignal<bool>,
    /// Latched true on the first successful connect handshake; never reset.
    /// Lets the boot gate disengage and the service gate engage — two
    /// surfaces that differ only in "have we ever been live?".
    pub has_connected_once: RwSignal<bool>,

    // Phase 3: Channel to send RPC requests to message loop
    rpc_tx: StoredValue<Option<mpsc::UnboundedSender<RpcRequest>>>,
    next_id: StoredValue<Arc<Mutex<u64>>>,

    // Phase 3: Event handling
    event_handlers: StoredValue<Arc<Mutex<Vec<EventHandler>>>>,

    // Channel for stopping the message loop
    disconnect_tx: StoredValue<Option<oneshot::Sender<()>>>,

    /// System alert state bus
    pub alerts: RwSignal<HashMap<String, SystemAlert>>,

    /// Alert subscription ID for cleanup
    alert_subscription_id: StoredValue<Option<usize>>,

    /// Pending operator-approval requests rendered by the `NotificationCenter`
    /// with inline allow-once / allow-session / deny buttons. Sourced from the
    /// `exec.approvals.pending` RPC; `approval.**` events trigger a refetch
    /// (see `setup_approval_subscriptions`).
    pub pending_approvals: RwSignal<Vec<PendingApprovalView>>,

    /// Approval subscription ID for cleanup.
    approval_subscription_id: StoredValue<Option<usize>>,

    /// Questions the agent is parked on (`ask_user`), rendered as inline cards
    /// in the conversation that is waiting. Pushed live by the `stream.ask_user`
    /// frame (`views::chat::events`) and seeded from `clarification.pending`
    /// when the chat surface mounts, so a reload mid-question still finds the
    /// blocked tool. Lives here — not on `ChatState` — because a question can
    /// belong to a background conversation whose `ChatState` isn't mounted.
    pub pending_clarifications: RwSignal<Vec<PendingAskView>>,

    /// Connection role captured from the `connect` response. `Some("operator")`
    /// once authorized (loopback, or a remote that presented a valid Gateway
    /// token); `None` before the first successful connect. Read by
    /// `is_operator()`.
    pub role: RwSignal<Option<String>>,

    /// True when the `connect` response reported `needs_token` — a remote
    /// connection without a valid Gateway token. Drives the full-screen login
    /// wall (token box). False for loopback / authorized connections.
    pub needs_token: RwSignal<bool>,

    /// True when `needs_token` was triggered by a rejected (stale/rotated)
    /// token rather than a first-time connection. Set by the `NeedsToken`
    /// handshake outcome before `clear_gateway_token()` erases the evidence.
    /// Read by `TokenWall` to select the appropriate instruction copy.
    pub token_was_rejected: RwSignal<bool>,

    /// Typed classification of the latest connection failure. Single source of
    /// truth; `connection_error` (String) is derived from it for legacy readers.
    pub connection_failure: RwSignal<Option<ConnectionFailure>>,
}

/// Build the gateway WS URL from a page protocol + host. `https:` ⇒ `wss://`.
/// Plain `http:` is only allowed for a loopback host (zero-config desktop);
/// a remote `http:` page is refused (`Err`) so the Panel never opens a
/// plaintext socket to a remote gateway.
///
/// The sole non-test caller (`derive_gateway_url`) is `wasm32`-gated (it reads
/// `web_sys::window()`), so a host-target build with tests excluded sees this
/// as dead — hence the cfg-scoped allow. On `wasm32` (the panel's real build)
/// it is live.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn ws_url_for(protocol: &str, host: &str) -> Result<String, ()> {
    if protocol == "https:" {
        return Ok(format!("wss://{host}/ws"));
    }
    let host_only = host.split(':').next().unwrap_or(host);
    let is_loopback = host_only == "127.0.0.1" || host_only == "::1" || host_only == "localhost";
    if is_loopback {
        Ok(format!("ws://{host}/ws"))
    } else {
        Err(())
    }
}

/// Derive the Gateway WebSocket URL from the current page location.
/// Same-origin (Panel UI and Gateway share a port). Remote-over-http is
/// refused — callers must surface an "insecure transport, use https" error.
fn derive_gateway_url() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let location = window.location();
            if let (Ok(protocol), Ok(host)) = (location.protocol(), location.host()) {
                match ws_url_for(&protocol, &host) {
                    Ok(url) => return url,
                    Err(()) => {
                        web_sys::console::error_1(
                            &"Aleph Panel: refusing insecure transport — open this Panel over https"
                                .into(),
                        );
                        // Non-connectable sentinel; connect() treats an empty
                        // URL as a typed ConnectionFailure and opens no socket.
                        return String::new();
                    }
                }
            }
        }
        "ws://127.0.0.1:18790/ws".to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "ws://127.0.0.1:18790/ws".to_string()
    }
}

/// How long to wait for the liveness probe before giving up on it. Only ever
/// paid on a failed connect, and only when the socket died before OPEN. A live
/// origin answers in milliseconds and a dead one refuses immediately; this cap
/// exists solely so a *hung* origin can't reintroduce a long stall.
#[cfg(target_arch = "wasm32")]
const ORIGIN_PROBE_TIMEOUT_MS: u32 = 3_000;

/// Ask the page's own origin whether any HTTP server is still there.
///
/// The WebSocket cannot answer this: a refused TCP connect and a `403` on the
/// upgrade both surface as `error` + `close(1006)` with no reason. Without
/// this second question the Panel would tell someone whose server is simply
/// not running that "the server is running but refused this connection".
///
/// Any HTTP response counts as alive — `fetch` only rejects on a network-level
/// failure, so even a 404 proves something is listening.
#[cfg(target_arch = "wasm32")]
async fn probe_origin_liveness() -> OriginLiveness {
    use wasm_bindgen_futures::JsFuture;

    let Some(window) = web_sys::window() else {
        return OriginLiveness::Unknown;
    };
    let Ok(origin) = window.location().origin() else {
        return OriginLiveness::Unknown;
    };
    // Cache-bust: a cached 200 would happily "prove" a server that died.
    let url = format!(
        "{origin}/health?_aleph_probe={}",
        js_sys::Date::now() as u64
    );
    let fetch = JsFuture::from(window.fetch_with_str(&url));

    use futures::future::{select, Either};
    match select(Box::pin(fetch), TimeoutFuture::new(ORIGIN_PROBE_TIMEOUT_MS)).await {
        Either::Left((Ok(_), _)) => OriginLiveness::Serving,
        Either::Left((Err(_), _)) => OriginLiveness::Silent,
        // No answer in time. We did not establish liveness, so we must not
        // claim it — but we also did not prove absence.
        Either::Right(((), _)) => OriginLiveness::Unknown,
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn probe_origin_liveness() -> OriginLiveness {
    OriginLiveness::Unknown
}

/// Stable per-Panel identity for device-tier pairing (G2/G3). The `device_id`
/// is a random handle generated once and kept in `localStorage`; the
/// `device_name` is a friendly label derived from the user agent. Sent in the
/// `connect` handshake so the gateway can look up / assign this device's
/// permission tier. The local (loopback) desktop App is always operator
/// regardless of this id, so it never lands in the device store.
#[cfg(target_arch = "wasm32")]
fn panel_device_identity() -> (String, String) {
    const KEY: &str = "aleph_device_id";
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    let device_id = storage
        .as_ref()
        .and_then(|s| s.get_item(KEY).ok().flatten())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            let id = format!(
                "panel-{:x}-{:x}",
                js_sys::Date::now() as u64,
                (js_sys::Math::random() * 1.0e9) as u64
            );
            if let Some(s) = storage.as_ref() {
                let _ = s.set_item(KEY, &id);
            }
            id
        });
    let device_name = web_sys::window()
        .map(|w| w.navigator())
        .and_then(|n| n.user_agent().ok())
        .map(|ua| friendly_device_name(&ua))
        .unwrap_or_else(|| "Panel".to_string());
    (device_id, device_name)
}

#[cfg(not(target_arch = "wasm32"))]
fn panel_device_identity() -> (String, String) {
    (String::new(), "Panel".to_string())
}

/// This Panel's `device_id`, or `None` before it has ever paired.
///
/// Exposed so the paired-device roster can mark which row is *this* browser.
/// Comparison happens client-side on purpose: the server would have to thread
/// connection identity into a pure-I/O handler to answer the same question, and
/// the client already knows. Now that revoking closes the device's live sessions
/// immediately, "am I about to log myself out?" is a question the operator must
/// be able to answer before clicking.
#[must_use]
pub(crate) fn local_device_id() -> Option<String> {
    let (id, _) = panel_device_identity();
    Some(id).filter(|s| !s.is_empty())
}

/// Best-effort "Browser on OS" label from a user-agent string.
#[cfg(target_arch = "wasm32")]
fn friendly_device_name(ua: &str) -> String {
    let browser = if ua.contains("Edg") {
        "Edge"
    } else if ua.contains("Chrome") || ua.contains("CriOS") {
        "Chrome"
    } else if ua.contains("Firefox") {
        "Firefox"
    } else if ua.contains("Safari") {
        "Safari"
    } else {
        "Browser"
    };
    let os = if ua.contains("Mac OS") {
        "macOS"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Linux") || ua.contains("X11") {
        "Linux"
    } else {
        "device"
    };
    format!("{browser} on {os}")
}

impl Default for DashboardState {
    fn default() -> Self {
        Self::new()
    }
}

impl DashboardState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            is_connected: RwSignal::new(false),
            reconnect_count: RwSignal::new(0),
            gateway_url: RwSignal::new(derive_gateway_url()),
            connection_error: RwSignal::new(None),
            is_reconnecting: RwSignal::new(false),
            has_connected_once: RwSignal::new(false),
            rpc_tx: StoredValue::new(None),
            next_id: StoredValue::new(Arc::new(Mutex::new(1))),
            event_handlers: StoredValue::new(Arc::new(Mutex::new(Vec::new()))),
            disconnect_tx: StoredValue::new(None),
            alerts: RwSignal::new(HashMap::new()),
            alert_subscription_id: StoredValue::new(None),
            pending_approvals: RwSignal::new(Vec::new()),
            approval_subscription_id: StoredValue::new(None),
            pending_clarifications: RwSignal::new(Vec::new()),
            role: RwSignal::new(None),
            needs_token: RwSignal::new(false),
            token_was_rejected: RwSignal::new(false),
            connection_failure: RwSignal::new(None),
        }
    }

    /// Reactive predicate: did this connection report the `operator` role?
    /// Consults the `role` captured from the `connect` response.
    /// Returns false before the first successful connect. Used by
    /// operator-only settings surfaces to gate UI up front.
    #[must_use]
    pub fn is_operator(&self) -> bool {
        role_is_operator(self.role.get().as_deref())
    }

    /// Capture the connection `role` + `needs_token` verdict from a `connect`
    /// response. Missing fields reset to a safe default (no role / not walled),
    /// keeping the signals consistent across reconnects.
    fn capture_role(&self, resp: &Value) {
        self.role
            .set(resp.get("role").and_then(|r| r.as_str()).map(String::from));
        self.needs_token.set(
            resp.get("needs_token")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        );
    }

    /// Record a typed failure and derive its legacy string. Centralised so the
    /// two never drift.
    fn set_failure(&self, f: ConnectionFailure) {
        let legacy = match &f {
            ConnectionFailure::AuthRequired => "auth required".to_string(),
            ConnectionFailure::Unreachable { detail }
            | ConnectionFailure::Timeout { detail }
            | ConnectionFailure::Rejected { detail }
            | ConnectionFailure::Dropped { detail }
            | ConnectionFailure::Unknown { detail } => detail.clone(),
        };
        self.connection_failure.set(Some(f));
        self.connection_error.set(Some(legacy));
    }

    /// Submit a credential from the login wall: route it by kind, then reload
    /// the page so the fresh `connect` handshake presents it. Reload (vs.
    /// in-place reconnect) keeps the boot/subscription wiring on its single
    /// happy path.
    ///
    /// Accepts all three credentials the server understands — a one-time pairing
    /// ticket (`aleph-bt-…`, what `aleph-server pair` and the QR hand out), a
    /// device token (`aleph-dt-…`), or the legacy shared token (`aleph-…`).
    /// Tickets used to be silently misfiled as shared tokens and rejected by the
    /// server, which made "read the code off the QR and type it in" — the only
    /// option when a phone cannot scan — impossible.
    pub fn submit_token(&self, token: String) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return;
        }
        match classify_credential(&token) {
            // Hand a ticket to the `?bt=` path rather than persisting it: it is
            // single-use, and the exchange returns the device token that actually
            // deserves storage. Navigating is the reload.
            SubmittedCredential::BootstrapTicket => redirect_with_bootstrap_ticket(&token),
            SubmittedCredential::DeviceToken => {
                persist_device_token(&token);
                scrub_credentials_and_reload();
            }
            SubmittedCredential::SharedToken => {
                persist_legacy_token(&token);
                scrub_credentials_and_reload();
            }
        }
    }

    /// Subscribe to Gateway events
    /// Returns a subscription ID that can be used to unsubscribe
    pub fn subscribe_events<F>(&self, handler: F) -> usize
    where
        F: Fn(GatewayEvent) + Send + Sync + 'static,
    {
        let handlers = self.event_handlers.with_value(std::clone::Clone::clone);
        let mut handlers = handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = handlers.len();
        handlers.push(Arc::new(handler));
        id
    }

    /// Unsubscribe from events
    pub fn unsubscribe_events(&self, id: usize) {
        let handlers = self.event_handlers.with_value(std::clone::Clone::clone);
        let mut handlers = handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if id < handlers.len() {
            // Replace with a no-op handler instead of removing to preserve indices
            handlers[id] = Arc::new(|_| {});
        }
    }

    /// Update alert state
    pub fn update_alert(&self, key: String, alert: SystemAlert) {
        self.alerts.update(|map| {
            map.insert(key, alert);
        });
    }

    /// Get alert state
    #[must_use]
    pub fn get_alert(&self, key: &str) -> Option<SystemAlert> {
        self.alerts.with(|map| map.get(key).cloned())
    }

    /// Clear alert state
    pub fn clear_alert(&self, key: &str) {
        self.alerts.update(|map| {
            map.remove(key);
        });
    }

    /// Dispatch event to all subscribers
    fn dispatch_event(&self, event: GatewayEvent) {
        let handlers = self.event_handlers.with_value(std::clone::Clone::clone);
        let handlers = handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for handler in handlers.iter() {
            handler(event.clone());
        }
    }

    /// Subscribe to a specific event topic on the Gateway
    pub async fn subscribe_topic(&self, pattern: &str) -> Result<(), String> {
        self.rpc_call(
            "events.subscribe",
            serde_json::json!({
                "topics": [pattern]
            }),
        )
        .await?;
        Ok(())
    }

    /// Unsubscribe from an event topic
    pub async fn unsubscribe_topic(&self, pattern: &str) -> Result<(), String> {
        self.rpc_call(
            "events.unsubscribe",
            serde_json::json!({
                "topics": [pattern]
            }),
        )
        .await?;
        Ok(())
    }

    /// Make an RPC call to the gateway
    pub async fn rpc_call(&self, method: &str, params: Value) -> Result<Value, String> {
        // Generate unique ID
        let id = {
            let next_id = self.next_id.with_value(std::clone::Clone::clone);
            let mut id_gen = next_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let id = *id_gen;
            *id_gen += 1;
            id.to_string()
        };

        // Create oneshot channel for response
        let (response_tx, response_rx) = oneshot::channel();

        // Create RPC request
        let request = RpcRequest {
            id,
            method: method.to_string(),
            params,
            response_tx,
        };

        // Send request to message loop
        {
            let rpc_tx = self.rpc_tx.with_value(std::clone::Clone::clone);
            if let Some(tx) = rpc_tx {
                tx.unbounded_send(request)
                    .map_err(|_| "Failed to send RPC request".to_string())?;
            } else {
                return Err("Not connected".to_string());
            }
        }

        // Wait for response, but don't hang forever. `onclose` covers the
        // socket-close case (it drops the pending sender → "Response channel
        // closed"); this 30s ceiling additionally covers a server that accepts
        // the request and then never replies without closing the socket,
        // converting an infinite spinner into a surfaced, retryable error.
        use futures::future::{select, Either};
        match select(response_rx, TimeoutFuture::new(30_000)).await {
            Either::Left((res, _)) => res.map_err(|_| "Response channel closed".to_string())?,
            Either::Right(((), _)) => Err("Request timed out".to_string()),
        }
    }

    /// Handshake outcome — distinguishes "authorized", "needs a token"
    /// (login wall, NOT a transport failure), and a transport-level failure.
    async fn handshake(&self) -> Handshake {
        let (device_id, device_name) = panel_device_identity();
        let creds = read_connect_credentials();
        let mut params = serde_json::json!({
            "device_id": device_id,
            "device_name": device_name,
        });

        // Priority: one-time bootstrap ticket > long-lived device token > legacy shared token.
        let credential_kind = if let Some(t) = creds.bootstrap_ticket.as_ref() {
            params["bootstrap_ticket"] = serde_json::json!(t);
            "bootstrap"
        } else if let Some(t) = creds.device_token.as_ref() {
            params["device_token"] = serde_json::json!(t);
            "device"
        } else if let Some(t) = creds.legacy_token.as_ref() {
            params["token"] = serde_json::json!(t);
            "legacy"
        } else {
            "none"
        };

        let resp = match self.rpc_call("connect", params).await {
            Ok(r) => r,
            Err(e) => return Handshake::Failed(classify(FailureStage::Handshake, Some(&e), false)),
        };
        self.capture_role(&resp);
        if resp.get("authorized").and_then(serde_json::Value::as_bool) == Some(true) {
            // A bootstrap exchange returns a fresh device token; store it for
            // subsequent reconnects and clear any legacy token.
            if let Some(dt) = resp.get("device_token").and_then(serde_json::Value::as_str) {
                persist_device_token(dt);
                clear_legacy_token();
            } else if credential_kind == "legacy" {
                if let Some(t) = creds.legacy_token {
                    persist_legacy_token(&t);
                }
            } else if credential_kind == "device" {
                // Device token still valid; keep it.
            }
            scrub_credentials_from_url();
            Handshake::Authorized
        } else {
            // Reachable but unauthorized: a stale/rotated/mismatched credential (if
            // any) must not silently re-fail next load. Login wall takes over.
            // Capture whether a credential was present *before* clearing it so
            // TokenWall can distinguish first-time vs rejected-token copy.
            let was_rejected = !read_connect_credentials().is_empty();
            clear_credentials();
            Handshake::NeedsToken { was_rejected }
        }
    }

    /// Connect to the gateway
    pub async fn connect(&self) -> Result<(), String> {
        let url = self.gateway_url.get();
        if url.is_empty() {
            // `derive_gateway_url()` refused to build a plaintext socket URL
            // for a remote (non-loopback) http origin and returned this
            // sentinel instead — never hand WasmConnector an empty address.
            let detail = "Insecure transport: open this Panel over https to reach a remote gateway"
                .to_string();
            self.is_connected.set(false);
            self.set_failure(ConnectionFailure::Unreachable {
                detail: detail.clone(),
            });
            return Err(detail);
        }
        let mut connector = WasmConnector::new();

        use futures::future::{select, Either};
        let open_result = {
            let connect_fut = Box::pin(connector.connect(&url));
            let open = select(connect_fut, TimeoutFuture::new(WS_OPEN_TIMEOUT_MS)).await;
            match open {
                Either::Left((res, _)) => res,
                Either::Right(((), _)) => {
                    // TCP may be up but WS upgrade hung — fail closed instead of
                    // spinning the boot gate forever.
                    Err(ConnectionError::ConnectFailed(
                        "WebSocket open timed out".into(),
                    ))
                }
            }
        };
        match open_result {
            Ok(()) => {
                // Get the message stream
                let stream = connector.receive();

                // Create channels
                let (rpc_tx, rpc_rx) = mpsc::unbounded::<RpcRequest>();
                let (disconnect_tx, disconnect_rx) = oneshot::channel::<()>();

                // Store channels
                self.rpc_tx.set_value(Some(rpc_tx));
                self.disconnect_tx.set_value(Some(disconnect_tx));

                // Clone state for message loop
                let state = *self;

                // Spawn message loop task that owns the connector
                spawn_local(async move {
                    web_sys::console::log_1(&"Message loop started".into());

                    let mut stream = stream.fuse();
                    let mut rpc_rx = rpc_rx.fuse();
                    let mut disconnect_rx = disconnect_rx.fuse();
                    let mut pending_rpcs: HashMap<String, oneshot::Sender<Result<Value, String>>> =
                        HashMap::new();
                    // Track whether the loop exited because of an explicit
                    // disconnect() call (no auto-reconnect) or an unintentional
                    // drop (auto-reconnect to drive ConnectionPhase::Reconnecting
                    // → ServiceBlockingGate). Captured drop_reason becomes the
                    // connection_error surfaced in the UI.
                    let mut intentional_close = false;
                    let mut drop_reason: Option<String> = None;

                    loop {
                        // Use futures::select! to handle multiple async operations
                        futures::select! {
                            // Handle incoming RPC requests
                            rpc_req = rpc_rx.select_next_some() => {
                                // Build JSON-RPC request
                                let request = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": rpc_req.id.clone(),
                                    "method": rpc_req.method,
                                    "params": rpc_req.params
                                });

                                // Send request
                                match connector.send(request).await {
                                    Ok(()) => {
                                        // Store pending request
                                        pending_rpcs.insert(rpc_req.id, rpc_req.response_tx);
                                    }
                                    Err(e) => {
                                        web_sys::console::error_1(&format!("Failed to send RPC: {e:?}").into());
                                        let _ = rpc_req.response_tx.send(Err(e.to_string()));
                                    }
                                }
                            }

                            // Handle incoming WebSocket messages
                            msg = stream.select_next_some() => {
                                match msg {
                                    Ok(value) => {
                                        // Topic/id-level breadcrumb only — never dump the full
                                        // message: RPC responses (e.g. gateway.token.current) and
                                        // event payloads carry secrets/content that must not leak
                                        // into the browser console.
                                        web_sys::console::log_1(&"Received WS message".into());

                                        // Check if this is an RPC response (has 'id' field)
                                        if let Some(id) = value.get("id").and_then(|id| id.as_str()) {
                                            // Handle RPC response
                                            if let Some(tx) = pending_rpcs.remove(id) {
                                                if let Some(error) = value.get("error") {
                                                    let msg = error.get("message")
                                                        .and_then(|m| m.as_str())
                                                        .unwrap_or("Unknown error")
                                                        .to_string();
                                                    let _ = tx.send(Err(msg));
                                                } else if let Some(result) = value.get("result") {
                                                    let _ = tx.send(Ok(result.clone()));
                                                }
                                            }
                                        } else {
                                            // This is an event notification
                                            // Parse event format: { "method": "event", "params": { "topic": "...", "data": {...} } }
                                            if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                                                if method == "event" {
                                                    if let Some(params) = value.get("params") {
                                                        if let Some(topic) = params.get("topic").and_then(|t| t.as_str()) {
                                                            let data = params.get("data").cloned().unwrap_or(Value::Null);

                                                            let event = GatewayEvent {
                                                                topic: topic.to_string(),
                                                                data,
                                                            };

                                                            web_sys::console::log_1(&format!("Event: {}", event.topic).into());

                                                            // Dispatch event to subscribers
                                                            state.dispatch_event(event);
                                                        }
                                                    }
                                                } else if method.starts_with("stream.") {
                                                    // Gateway sends streaming events as {method: "stream.run_accepted", params: {...StreamEvent...}}
                                                    // Convert to GatewayEvent with run.* topic for subscriber filtering
                                                    let data = value.get("params").cloned().unwrap_or(Value::Null);
                                                    let topic = method.replacen("stream.", "run.", 1);
                                                    let event = GatewayEvent {
                                                        topic,
                                                        data,
                                                    };
                                                    web_sys::console::log_1(&format!("Stream event: {}", event.topic).into());
                                                    state.dispatch_event(event);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        web_sys::console::error_1(&format!("Message loop error: {e:?}").into());
                                        drop_reason = Some(format!("WebSocket dropped: {e}"));
                                        break;
                                    }
                                }
                            }

                            // Handle disconnect signal
                            _ = disconnect_rx => {
                                web_sys::console::log_1(&"Disconnect signal received".into());
                                let _ = connector.disconnect().await;
                                intentional_close = true;
                                break;
                            }

                            // If all channels are closed, exit (graceful close
                            // by remote, or sender side dropped). Treated as
                            // unintentional — auto-reconnect handles it below.
                            complete => {
                                drop_reason = Some(
                                    "WebSocket closed (channels exhausted)".to_string(),
                                );
                                break;
                            }
                        }
                    }

                    web_sys::console::log_1(&"Message loop stopped".into());

                    // Unintentional drop: flip is_connected so ConnectionPhase
                    // re-derives, then kick off reconnect() from a fresh task
                    // so ServiceBlockingGate engages after the 5-attempt
                    // budget exhausts. We intentionally do NOT set
                    // connection_error here — reconnect() sets it on final
                    // failure, and setting it now would make the chip flash
                    // "Failed" during the retry window (the derive rule treats
                    // any error as terminal). The drop_reason is logged for
                    // dev-console debugging.
                    if !intentional_close {
                        if let Some(reason) = drop_reason.as_deref() {
                            web_sys::console::warn_1(
                                &format!(
                                    "WS dropped unintentionally; auto-reconnecting. reason={reason}"
                                )
                                .into(),
                            );
                        }
                        // If the drop carries an auth signal — the gateway closed us with
                        // `token_rotated` or `device_revoked` — route straight to the login
                        // wall instead of burning a backoff cycle: set the typed failure so
                        // reconnect()'s AuthRequired short-circuit fires immediately. Set ONLY
                        // connection_failure (NOT connection_error): ConnectionPhase::derive keys
                        // "Failed" off connection_error, so leaving it None keeps the chip showing
                        // "Reconnecting N/5" for ordinary drops while still routing auth kicks.
                        if matches!(
                            classify(FailureStage::AfterOpen, drop_reason.as_deref(), false),
                            ConnectionFailure::AuthRequired
                        ) {
                            state
                                .connection_failure
                                .set(Some(ConnectionFailure::AuthRequired));
                            // That short-circuit skips the handshake, so the two things
                            // the handshake's wall path does must happen here instead.
                            // We were authorized a moment ago and got kicked *for an auth
                            // reason*, so by construction a credential existed and the
                            // server has already killed it: say so in the wall copy, and
                            // drop it so the next page load doesn't re-present a corpse.
                            state.token_was_rejected.set(true);
                            clear_credentials();
                        }
                        state.is_connected.set(false);
                        // Clear the dead rpc_tx so the next rpc_call() won't
                        // block on a sender whose receiver task just exited.
                        state.rpc_tx.set_value(None);
                        spawn_local(async move {
                            let _ = state.reconnect().await;
                        });
                    }
                });

                // Complete the session handshake before marking as connected.
                let handshake_state = *self;
                match handshake_state.handshake().await {
                    Handshake::Authorized => {
                        self.is_connected.set(true);
                        self.connection_error.set(None);
                        self.connection_failure.set(None);
                        self.reconnect_count.set(0);
                        self.is_reconnecting.set(false);
                        self.has_connected_once.set(true);

                        let state_for_subscribe = *self;
                        spawn_local(async move {
                            // Per-socket topic subscriptions must be re-established on
                            // every (re)connect: a reconnect opens a fresh conn_id whose
                            // gateway filter starts empty, and subscribing config.** alone
                            // would drop alerts.**/approval.** (bell + approval cards go
                            // silent after the first reconnect). subscribe_topic is
                            // idempotent on the gateway, so the one-time client-side
                            // handlers stay registered.
                            for topic in ["config.**", "alerts.**", "approval.**"] {
                                if let Err(e) = state_for_subscribe.subscribe_topic(topic).await {
                                    web_sys::console::error_1(
                                        &format!("Failed to subscribe to {topic} events: {e}")
                                            .into(),
                                    );
                                }
                            }
                        });
                        Ok(())
                    }
                    Handshake::NeedsToken { was_rejected } => {
                        // Reachable but walled — do NOT mark connected and do NOT
                        // spawn subscriptions (only `connect` is allowed unauthorized).
                        self.is_connected.set(false);
                        self.needs_token.set(true);
                        self.token_was_rejected.set(was_rejected);
                        self.set_failure(ConnectionFailure::AuthRequired);
                        // Returning Ok keeps the boot/reconnect path from treating
                        // the wall as a transport failure. `needs_token` makes
                        // BootCheckGate yield (see `boot_gate_visible`), so the
                        // login wall (TokenWall) owns the screen instead of the
                        // higher-z-index "cannot reach core" gate burying it.
                        Ok(())
                    }
                    Handshake::Failed(f) => {
                        self.is_connected.set(false);
                        let msg = match &f {
                            ConnectionFailure::Unreachable { detail }
                            | ConnectionFailure::Timeout { detail }
                            | ConnectionFailure::Rejected { detail }
                            | ConnectionFailure::Dropped { detail }
                            | ConnectionFailure::Unknown { detail } => detail.clone(),
                            ConnectionFailure::AuthRequired => "auth required".to_string(),
                        };
                        self.set_failure(f);
                        Err(msg)
                    }
                }
            }
            Err(e) => {
                self.is_connected.set(false);
                // Keep the connector's verdict: a peer that answered and
                // refused is `Rejected` (config problem, server is up), a
                // silent socket is `BeforeOpen`/`Unreachable`. Hardcoding
                // BeforeOpen here is what made every 403 read as "timed out".
                //
                // The socket alone cannot tell a 403 from a dead port, so a
                // refusal is corroborated by an independent probe of the
                // origin before we dare tell the user their server is up.
                // Only pay for it on that branch — a timeout already proved
                // silence, and probing it would just add another wait.
                let origin = if matches!(e, ConnectionError::FailedBeforeOpen(_)) {
                    probe_origin_liveness().await
                } else {
                    OriginLiveness::Unknown
                };
                let stage = stage_for_connect_error(&e, origin);
                let detail = e.to_string();
                self.set_failure(classify(stage, Some(&detail), false));
                Err(detail)
            }
        }
    }

    /// Disconnect from the gateway
    pub async fn disconnect(&self) -> Result<(), String> {
        // Cleanup alert subscriptions first
        self.cleanup_alert_subscriptions();

        // Send disconnect signal to message loop (take ownership)
        let mut tx_opt = None;
        self.disconnect_tx.update_value(|v| {
            tx_opt = v.take();
        });
        if let Some(tx) = tx_opt {
            let _ = tx.send(());
        }

        // Clear RPC channel
        self.rpc_tx.set_value(None);

        // Update state
        self.is_connected.set(false);
        self.connection_error.set(None);
        self.is_reconnecting.set(false);
        Ok(())
    }

    /// Attempt to reconnect. Differentiated by failure type: an `AuthRequired`
    /// failure breaks out immediately to the login wall (retrying the same bad
    /// token is wasted); every other class uses exponential backoff with
    /// downward jitter, reusing the shared `ReconnectStrategy`.
    pub async fn reconnect(&self) -> Result<(), String> {
        use crate::state::connection::MAX_RECONNECT_ATTEMPTS;
        use shared_ui_logic::connection::{ConnectionFailure, ReconnectStrategy};

        if matches!(
            self.connection_failure.get_untracked(),
            Some(ConnectionFailure::AuthRequired)
        ) {
            self.needs_token.set(true);
            self.is_reconnecting.set(false);
            return Ok(());
        }

        self.is_reconnecting.set(true);
        let mut strategy = ReconnectStrategy::new(MAX_RECONNECT_ATTEMPTS, 1000);
        let mut attempt: u32 = 0;
        while let Some(delay) = {
            // ~±10% downward jitter; Math::random is wasm-only.
            #[cfg(target_arch = "wasm32")]
            let permille = (js_sys::Math::random() * 100.0) as u64;
            #[cfg(not(target_arch = "wasm32"))]
            let permille = 0u64;
            strategy.next_delay_jittered(permille)
        } {
            self.reconnect_count.set(attempt);
            TimeoutFuture::new(delay as u32).await;
            match self.connect().await {
                Ok(()) => {
                    // connect() returns Ok for both Authorized and NeedsToken;
                    // if it walled, stop here (TokenWall covers it).
                    self.is_reconnecting.set(false);
                    return Ok(());
                }
                Err(_) => {
                    attempt += 1;
                }
            }
        }

        // Budget exhausted — leave the classified failure in place (connect()
        // already set it) so the gate shows the right copy.
        self.reconnect_count.set(MAX_RECONNECT_ATTEMPTS);
        self.is_reconnecting.set(false);
        Err("Reconnection failed".to_string())
    }

    /// Setup alert subscriptions
    ///
    /// This method subscribes to alert-related events from the Gateway and
    /// updates the DashboardState.alerts `HashMap` when events arrive.
    /// It also fetches initial alert states on mount.
    pub async fn setup_alert_subscriptions(&self) -> Result<(), String> {
        // Subscribe to alert events on the Gateway
        self.subscribe_topic("alerts.**").await?;

        web_sys::console::log_1(&"Subscribed to alerts.** events".into());

        // Load initial alert states
        let state_for_init = *self;
        spawn_local(async move {
            if let Err(e) = state_for_init.load_initial_alerts().await {
                web_sys::console::error_1(&format!("Failed to load initial alerts: {e}").into());
            }
        });

        // Setup event handler for alert events
        let state = *self;
        let subscription_id = self.subscribe_events(move |event: GatewayEvent| {
            web_sys::console::log_1(
                &format!("Alert event received: {} - {:?}", event.topic, event.data).into(),
            );

            // Parse alert data and update state
            if event.topic.starts_with("alerts.") {
                // Extract alert type from topic (e.g., "alerts.system.health" -> "system.health")
                let alert_key = event.topic.strip_prefix("alerts.").unwrap_or(&event.topic);

                // Parse alert data
                if let Some(severity) = event.data.get("severity").and_then(|s| s.as_str()) {
                    let level = match severity {
                        "info" => crate::components::sidebar::AlertLevel::Info,
                        "warning" => crate::components::sidebar::AlertLevel::Warning,
                        "error" | "critical" => crate::components::sidebar::AlertLevel::Critical,
                        _ => {
                            web_sys::console::warn_1(
                                &format!("Unknown alert severity: {severity}").into(),
                            );
                            crate::components::sidebar::AlertLevel::None
                        }
                    };

                    let count = event
                        .data
                        .get("count")
                        .and_then(serde_json::Value::as_u64)
                        .map(|c| c as u32);

                    let message = event
                        .data
                        .get("message")
                        .and_then(|m| m.as_str())
                        .map(std::string::ToString::to_string);

                    // Create SystemAlert with String key (no memory leak)
                    let alert = crate::components::sidebar::SystemAlert {
                        key: alert_key.to_string(),
                        level,
                        count,
                        message,
                    };

                    // Update alert state
                    state.update_alert(alert.key.clone(), alert);
                } else {
                    // If no severity, clear the alert
                    web_sys::console::warn_1(
                        &format!("Alert event missing severity field: {}", event.topic).into(),
                    );
                    state.clear_alert(alert_key);
                }
            }
        });

        // Store subscription ID for cleanup
        self.alert_subscription_id.set_value(Some(subscription_id));

        Ok(())
    }

    /// Subscribe to `approval.**` events so the `NotificationCenter` can render
    /// inline operator approval cards. The `ApprovalRequested` event is sparse
    /// (ids only), so `exec.approvals.pending` is the source of truth: any
    /// approval event simply triggers a refetch.
    pub async fn setup_approval_subscriptions(&self) -> Result<(), String> {
        self.subscribe_topic("approval.**").await?;
        web_sys::console::log_1(&"Subscribed to approval.** events".into());

        // Seed with whatever is already pending at connect time.
        if let Ok(list) = ExecApprovalApi::list_pending(self).await {
            self.pending_approvals.set(list);
        }

        let state = *self;
        let subscription_id =
            self.subscribe_events(move |event: GatewayEvent| match event.topic.as_str() {
                "approval.requested" | "approval.resolved" | "approval.expired" => {
                    spawn_local(async move {
                        if let Ok(list) = ExecApprovalApi::list_pending(&state).await {
                            state.pending_approvals.set(list);
                        }
                    });
                }
                _ => {}
            });

        self.approval_subscription_id
            .set_value(Some(subscription_id));
        Ok(())
    }

    /// Load initial alert states from Gateway
    ///
    /// This method fetches the current alert states when the UI first connects,
    /// ensuring that existing alerts are displayed even if no new events arrive.
    ///
    /// # Implementation Note
    ///
    /// Currently uses direct `rpc_call()` methods instead of `AlertsApi` from `shared_ui_logic`.
    /// This is because the `AlertsApi` in `/Volumes/TBU4/Workspace/Aleph/shared_ui_logic/` uses
    /// a different `RpcClient` implementation that is incompatible with the current architecture.
    ///
    /// **TODO**: Refactor to use `AlertsApi::get_system_health()` and `AlertsApi::get_memory_status()`
    /// once the `shared_ui_logic` crate is unified and the `RpcClient` implementations are aligned.
    async fn load_initial_alerts(&self) -> Result<(), String> {
        web_sys::console::log_1(&"Loading initial alert states...".into());

        // Fetch system health
        match self.rpc_call("health", serde_json::json!({})).await {
            Ok(result) => {
                if let Some(status) = result.get("status").and_then(|s| s.as_str()) {
                    let level = match status {
                        "healthy" => crate::components::sidebar::AlertLevel::None,
                        "degraded" => crate::components::sidebar::AlertLevel::Warning,
                        "unhealthy" => crate::components::sidebar::AlertLevel::Critical,
                        _ => crate::components::sidebar::AlertLevel::None,
                    };

                    if level != crate::components::sidebar::AlertLevel::None {
                        let message = result
                            .get("message")
                            .and_then(|m| m.as_str())
                            .map(std::string::ToString::to_string);

                        let alert = crate::components::sidebar::SystemAlert {
                            key: "system.health".to_string(),
                            level,
                            count: None,
                            message,
                        };

                        self.update_alert(alert.key.clone(), alert);
                        web_sys::console::log_1(
                            &format!("Loaded system.health alert: {level:?}").into(),
                        );
                    }
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to fetch system health: {e}").into());
            }
        }

        // Fetch memory status
        match self.rpc_call("memory.stats", serde_json::json!({})).await {
            Ok(result) => {
                if let Some(db_size) = result
                    .get("databaseSizeMb")
                    .and_then(serde_json::Value::as_f64)
                {
                    // Warn if database is larger than 100MB
                    if db_size > 100.0 {
                        let alert = crate::components::sidebar::SystemAlert {
                            key: "memory.status".to_string(),
                            level: crate::components::sidebar::AlertLevel::Warning,
                            count: None,
                            message: Some(format!("Database size: {db_size:.1} MB")),
                        };

                        self.update_alert(alert.key.clone(), alert);
                        web_sys::console::log_1(
                            &format!("Loaded memory.status alert: {db_size:.1} MB").into(),
                        );
                    }
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to fetch memory stats: {e}").into());
            }
        }

        web_sys::console::log_1(&"Initial alert states loaded".into());
        Ok(())
    }

    /// Cleanup alert subscriptions
    ///
    /// This method unsubscribes from alert events and clears the subscription ID.
    pub fn cleanup_alert_subscriptions(&self) {
        if let Some(subscription_id) = self.alert_subscription_id.get_value() {
            self.unsubscribe_events(subscription_id);
            self.alert_subscription_id.set_value(None);
            web_sys::console::log_1(&"Unsubscribed from alert events".into());
        }
    }
}

#[component]
#[must_use]
pub fn DashboardContext(children: Children) -> impl IntoView {
    let state = DashboardState::new();
    provide_context(state);

    view! {
        <ErrorBoundary
            fallback=|errors| view! {
                <div class="min-h-screen flex items-center justify-center bg-surface text-text-primary p-8">
                    <div class="max-w-md w-full bg-surface-raised border border-danger/20 rounded-2xl p-8">
                        <h2 class="text-2xl font-bold text-danger mb-4 flex items-center gap-2">
                            "System Error"
                        </h2>
                        <div class="space-y-4">
                            <For
                                each=move || errors.get()
                                key=|(id, _)| id.clone()
                                children=move |(_, error)| {
                                    let error_string = error.to_string();
                                    view! {
                                        <div class="bg-danger-subtle border border-danger/20 rounded-xl p-4 text-sm text-danger font-mono">
                                            {error_string}
                                        </div>
                                    }
                                }
                            />
                        </div>
                        <button
                            on:click=|_| {
                                #[cfg(target_arch = "wasm32")]
                                {
                                    if let Some(w) = web_sys::window() {
                                        let _ = w.location().reload();
                                    }
                                }
                            }
                            class="mt-8 w-full py-3 bg-surface-sunken hover:bg-surface-raised rounded-xl transition-colors font-semibold"
                        >
                            "Reload Dashboard"
                        </button>
                    </div>
                </div>
            }
        >
            {children()}
        </ErrorBoundary>
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_credential, query_with_bootstrap_ticket, role_is_operator, strip_params,
        ws_url_for, SubmittedCredential,
    };

    /// What `scrub_credentials_from_url` actually strips — both credential
    /// params, not just the legacy one.
    const CREDENTIAL_PARAMS: &[&str] = &["token=", "bt="];

    #[test]
    fn stripping_the_only_param_collapses_to_empty() {
        assert_eq!(strip_params("?token=aleph-abc", CREDENTIAL_PARAMS), "");
        assert_eq!(strip_params("token=aleph-abc", CREDENTIAL_PARAMS), "");
        assert_eq!(strip_params("?bt=aleph-bt-abc", CREDENTIAL_PARAMS), "");
    }

    #[test]
    fn stripping_keeps_other_params() {
        assert_eq!(
            strip_params("?token=aleph-abc&view=chat", CREDENTIAL_PARAMS),
            "view=chat"
        );
        assert_eq!(
            strip_params("?view=chat&token=aleph-abc", CREDENTIAL_PARAMS),
            "view=chat"
        );
        assert_eq!(
            strip_params("?a=1&bt=aleph-bt-abc&b=2", CREDENTIAL_PARAMS),
            "a=1&b=2"
        );
    }

    #[test]
    fn stripping_is_a_noop_without_credentials() {
        assert_eq!(strip_params("?view=chat", CREDENTIAL_PARAMS), "view=chat");
        assert_eq!(strip_params("", CREDENTIAL_PARAMS), "");
        assert_eq!(strip_params("?", CREDENTIAL_PARAMS), "");
    }

    #[test]
    fn credentials_are_classified_by_their_wire_prefix() {
        assert_eq!(
            classify_credential("aleph-bt-1234"),
            SubmittedCredential::BootstrapTicket
        );
        assert_eq!(
            classify_credential("aleph-dt-1234"),
            SubmittedCredential::DeviceToken
        );
        assert_eq!(
            classify_credential("aleph-1234"),
            SubmittedCredential::SharedToken
        );
    }

    #[test]
    fn a_pasted_ticket_becomes_a_bt_query_and_drops_stale_credentials() {
        // Typing the code off a QR must reach the same `?bt=` path as scanning it.
        assert_eq!(
            query_with_bootstrap_ticket("", "aleph-bt-1"),
            "bt=aleph-bt-1"
        );
        assert_eq!(
            query_with_bootstrap_ticket("?view=chat", "aleph-bt-1"),
            "view=chat&bt=aleph-bt-1"
        );
        // A stale/expired credential in the URL would otherwise shadow the one
        // just typed, because the URL wins over localStorage.
        assert_eq!(
            query_with_bootstrap_ticket("?bt=aleph-bt-expired&token=aleph-old", "aleph-bt-1"),
            "bt=aleph-bt-1"
        );
    }

    #[test]
    fn operator_role_is_operator() {
        assert!(role_is_operator(Some("operator")));
    }

    #[test]
    fn guest_role_is_not_operator() {
        assert!(!role_is_operator(Some("guest")));
    }

    #[test]
    fn missing_role_is_not_operator() {
        assert!(!role_is_operator(None));
    }

    #[test]
    fn unknown_role_is_not_operator() {
        assert!(!role_is_operator(Some("node")));
    }

    #[test]
    fn https_page_yields_wss() {
        assert_eq!(
            ws_url_for("https:", "app.example.com").unwrap(),
            "wss://app.example.com/ws"
        );
    }

    #[test]
    fn loopback_http_yields_ws() {
        assert_eq!(
            ws_url_for("http:", "127.0.0.1:18790").unwrap(),
            "ws://127.0.0.1:18790/ws"
        );
        assert_eq!(
            ws_url_for("http:", "localhost:18790").unwrap(),
            "ws://localhost:18790/ws"
        );
    }

    #[test]
    fn remote_http_is_refused() {
        assert!(ws_url_for("http:", "app.example.com").is_err());
        assert!(ws_url_for("http:", "203.0.113.9:18790").is_err());
    }
}
