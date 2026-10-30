use crate::api::native_credentials::{
    attempt_current, native_expected, native_handshake, new_device_id, ticket_from_search,
    AttemptReceipt, ConnectSendError, HandshakeRequest, NativeAuthState, NativeOutcome, TauriIpc,
};
use crate::api::ExecApprovalApi;
use crate::api::TelecomApprovalApi;
use crate::components::sidebar::SystemAlert;
#[cfg(feature = "panel-modular")]
use crate::panel::discovery::{decode_snapshot, PanelDiscoveryState, SnapshotUpdate};
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
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

const WS_OPEN_TIMEOUT_MS: u32 = 8_000;
/// A/B requests emit no gateway event; mounted operator views sample this list.
const TELECOM_APPROVAL_POLL_MS: u32 = 4_000;

/// Releases the shared refresh gate even if a mounted view cancels its future.
struct TelecomRefreshGuard(Arc<AtomicBool>);
impl Drop for TelecomRefreshGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Retry a transient failure only after this run proved operator capability.
/// A role refusal is always terminal until a reconnect starts a new run.
fn telecom_poll_continues(authorized: bool, outcome: &Option<Result<(), RpcFailure>>) -> bool {
    match outcome {
        None | Some(Ok(())) => true,
        Some(Err(error)) => {
            authorized && error.code != Some(aleph_protocol::jsonrpc::AUTH_REQUIRED)
        }
    }
}

/// How long a request will park waiting for the socket to finish connecting
/// before it is failed. Covers [`WS_OPEN_TIMEOUT_MS`] plus the `connect`
/// handshake round-trip, with headroom for one reconnect backoff.
const GATEWAY_READY_TIMEOUT_MS: u32 = 12_000;

/// Poll interval while parked. `is_connected` is a signal, and a parked request
/// has no reactive owner to subscribe with, so readiness is sampled.
const GATEWAY_READY_POLL_MS: u32 = 50;

/// Error text of a request that was **never put on the wire** because the socket
/// had not finished connecting.
///
/// Deliberately distinguishable from every verdict the server can return: this
/// says the question was never asked, so no caller may read it as an answer. It
/// is a shared constant rather than a transcription for the reason
/// [`crate::components::admin_refusal`] gives about `ADMIN_REQUIRED_MESSAGE` — a
/// reword must move every consumer in one edit.
pub const GATEWAY_NOT_READY: &str = "Gateway not ready: still connecting";

/// Whether a request may be sent now, must wait, or is waiting on a human.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayReadiness {
    /// Authorized socket — send it.
    Ready,
    /// Connecting or reconnecting. Parking is right; failing is a lie.
    TooEarly,
    /// Walled behind the login prompt. No amount of waiting helps — only the
    /// user entering a credential does, and `TokenWall` owns the screen.
    Walled,
}

/// The transport's answer to "is this request too early, or is it hopeless?".
///
/// Extracted as a pure function because the whole defect it exists to stop is
/// invisible from inside `rpc_call`: the observable is a WS frame that either
/// does or does not appear, which is exactly what went unnoticed for every page
/// that loaded before the handshake. Same reason
/// [`crate::state::user_directory::should_fetch`] is a pure function.
///
/// Gates on `is_connected` and **not** on `rpc_tx.is_some()`. The two are not
/// interchangeable: `connect()` installs `rpc_tx` before it runs the handshake,
/// so a request admitted on the channel's existence alone would be written to an
/// unauthorized socket.
#[must_use]
pub const fn gateway_readiness(is_connected: bool, needs_token: bool) -> GatewayReadiness {
    if is_connected {
        GatewayReadiness::Ready
    } else if needs_token {
        GatewayReadiness::Walled
    } else {
        GatewayReadiness::TooEarly
    }
}

/// A failed RPC, as the wire reported it — or as this client manufactured it.
///
/// `code` is `Some` only when a real JSON-RPC error object carried one. Every
/// locally-minted failure (socket not up, send failed, response channel
/// closed, timeout) is `code: None`, so a caller branching on a code can
/// never mistake a transport hiccup for a server verdict — the same
/// "a refusal is not an answer" discipline [`GATEWAY_NOT_READY`] documents,
/// carried into the typed face.
///
/// Most consumers never see this type: [`DashboardState::rpc_call`] projects
/// it to `message` alone at exactly one site, so the ~150 `String`-error call
/// sites (and the `admin_refusal` classifier behind them) receive bytes
/// identical to what they always received. Callers that need the code — the
/// canvas conflict classifier is the one that forced this type into
/// existence — use [`DashboardState::rpc_call_with_code`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcFailure {
    /// The JSON-RPC `error.code`, when the server sent one.
    pub code: Option<i32>,
    /// The human/model-readable `error.message` (or the local failure text).
    pub message: String,
}

impl RpcFailure {
    /// A failure this client minted itself — never carries a code, by
    /// construction, so it can never impersonate a server verdict.
    fn local(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }
}

/// Parse a JSON-RPC `error` member into an [`RpcFailure`].
///
/// Pure so the message loop's one parsing decision is unit-testable off the
/// wasm target. A code outside `i32` (not producible by the server, but the
/// wire is untrusted input) degrades to `None` rather than wrapping.
fn parse_rpc_error(error: &Value) -> RpcFailure {
    RpcFailure {
        code: error
            .get("code")
            .and_then(Value::as_i64)
            .and_then(|c| i32::try_from(c).ok()),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Unknown error")
            .to_string(),
    }
}

// RPC request sent to the message loop
struct RpcRequest {
    id: String,
    method: String,
    params: Value,
    response_tx: oneshot::Sender<Result<Value, RpcFailure>>,
}

// Event received from Gateway
#[derive(Clone, Debug)]
pub struct GatewayEvent {
    pub topic: String,
    pub data: Value,
}

// Event handler callback type
type EventHandler = Arc<dyn Fn(GatewayEvent) + Send + Sync>;

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
        if let Err(e) = s.set_item(GATEWAY_DEVICE_TOKEN_KEY, token) {
            // Silent failure here used to surface as the user logging back in
            // on every reload ("Aleph keeps forgetting my token"). Log so
            // the dev console shows the real cause (full / private mode /
            // disabled storage) and the boot flow can react.
            web_sys::console::warn_1(
                &format!("localStorage write failed for device token: {e:?}").into(),
            );
        }
    }
}

/// Persist a validated legacy shared token for backward compatibility.
#[cfg(target_arch = "wasm32")]
fn persist_legacy_token(token: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        if let Err(e) = s.set_item(GATEWAY_LEGACY_TOKEN_KEY, token) {
            web_sys::console::warn_1(
                &format!("localStorage write failed for legacy token: {e:?}").into(),
            );
        }
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
    /// A newer attempt, a disconnect or an endpoint change owns the state;
    /// this attempt must touch nothing.
    Superseded,
    NeedsToken {
        was_rejected: bool,
    },
    Failed(ConnectionFailure),
}

/// Topics every socket carries regardless of which components are mounted:
/// live config pushes, the alert bell, and approval cards. They have no owning
/// component to re-subscribe them, so they are seeded on every connect.
const BASE_TOPICS: [&str; 3] = ["config.**", "alerts.**", "approval.**"];

/// One subscription as the ledger files it: a topic pattern and the topics
/// carved out of it (`aleph_protocol::TopicCarveOut`). The carve-out is part
/// of the key — the phone's `stream.*` minus reasoning and the wide chat's
/// plain `stream.*` are two subscriptions, not one.
type LedgerEntry = (String, Vec<String>);

/// `BASE_TOPICS` ∪ ledger, deduplicated and ordered.
///
/// Pure so the "a reconnect must not narrow the socket" rule is testable on the
/// host without a websocket.
fn replay_set(ledger: &BTreeSet<LedgerEntry>) -> Vec<LedgerEntry> {
    let mut all: BTreeSet<LedgerEntry> = ledger.clone();
    for base in BASE_TOPICS {
        all.insert((base.to_string(), Vec::new()));
    }
    all.into_iter().collect()
}

/// The `events.subscribe` / `events.unsubscribe` params for one entry.
fn topics_request(pattern: &str, except: &[String]) -> Value {
    serde_json::to_value(aleph_protocol::TopicsRequest::new(
        vec![pattern.to_string()],
        except.to_vec(),
    ))
    .unwrap_or(Value::Null)
}

/// Re-subscribe every replayed entry through `subscribe`, carve-out included,
/// and return the ones that failed with the error. The reconnect handler's
/// whole replay step: a carve-out that is filed in the ledger but not re-sent
/// here would vanish on the first reconnect and the phone would be back to
/// receiving every reasoning frame.
async fn replay_subscriptions<F, Fut>(
    entries: Vec<LedgerEntry>,
    mut subscribe: F,
) -> Vec<(String, String)>
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    // Concurrent, not serial: one subscribe that never answers (30 s ceiling in
    // `send_rpc`) must not hold every entry queued behind it. Each future is
    // independent and a failure is collected per entry.
    let pending: Vec<_> = entries
        .into_iter()
        .map(|(pattern, except)| {
            let request = subscribe(topics_request(&pattern, &except));
            async move { request.await.map_err(|e| (pattern, e)) }
        })
        .collect();
    futures::future::join_all(pending)
        .await
        .into_iter()
        .filter_map(Result::err)
        .collect()
}

/// Why a `subscribe_topic` call did not end with the entry in the ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SubscribeError {
    /// The RPC itself failed (server verdict or local transport failure).
    Rpc(RpcFailure),
    /// The RPC succeeded, but on a socket that is no longer the current one
    /// (reconnected, or dropped, while the reply was in flight). The reply
    /// says nothing about the new socket's server-side filter.
    Superseded,
}

impl SubscribeError {
    /// The message handed back to `String`-error callers.
    fn into_message(self) -> String {
        match self {
            Self::Rpc(failure) => failure.message,
            Self::Superseded => "Connection changed during subscribe".to_string(),
        }
    }
}

/// Send one `events.subscribe` (`call`) and file `entry` in `ledger` **only
/// if it succeeded on the connection it was issued on**.
///
/// The ledger is what a reconnect replays, so it must mean "the server
/// accepted this" - never "someone asked for this". Filing before the RPC
/// (the previous shape) made a timed-out or rejected subscribe look
/// subscribed forever: the replay re-offered it without anyone knowing it had
/// never worked, and a `connected` socket silently lacked `approval.**`.
///
/// `epoch_then` is the `connection_epoch` captured after the socket was ready
/// and before the request went out; `current_epoch` yields the epoch now, or
/// `None` while disconnected. Anything but an exact match is `Superseded` and
/// leaves the ledger alone. Pure over its inputs so the host tests drive it
/// without a websocket.
async fn subscribe_recorded<Fut>(
    ledger: &Mutex<BTreeSet<LedgerEntry>>,
    entry: LedgerEntry,
    epoch_then: u64,
    current_epoch: impl FnOnce() -> Option<u64>,
    call: Fut,
) -> Result<(), SubscribeError>
where
    Fut: std::future::Future<Output = Result<Value, RpcFailure>>,
{
    call.await.map_err(SubscribeError::Rpc)?;
    if current_epoch() != Some(epoch_then) {
        return Err(SubscribeError::Superseded);
    }
    ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(entry);
    Ok(())
}

/// One console line per subscribe attempt: the topic pattern and an outcome
/// KIND (`ok`, `error (server|local|superseded)`). Deliberately never the
/// error message, params, URL or any credential - a server message may echo
/// them.
fn subscribe_diagnostic(topic: &str, result: &Result<(), SubscribeError>) -> String {
    let outcome = match result {
        Ok(()) => "ok",
        Err(SubscribeError::Rpc(f)) if f.code.is_some() => "error (server)",
        Err(SubscribeError::Rpc(_)) => "error (local)",
        Err(SubscribeError::Superseded) => "error (superseded)",
    };
    format!("events.subscribe {topic}: {outcome}")
}

#[cfg(target_arch = "wasm32")]
fn log_subscribe_diagnostic(topic: &str, result: &Result<(), SubscribeError>) {
    let line = subscribe_diagnostic(topic, result);
    if result.is_ok() {
        web_sys::console::log_1(&line.into());
    } else {
        web_sys::console::warn_1(&line.into());
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn log_subscribe_diagnostic(topic: &str, result: &Result<(), SubscribeError>) {
    // No console off the wasm target; build the line so the formatter stays
    // exercised (and not dead code) on the host.
    let _ = subscribe_diagnostic(topic, result);
}

/// Responses the message loop is still waiting for, keyed by JSON-RPC id.
type PendingRpcs = HashMap<String, oneshot::Sender<Result<Value, RpcFailure>>>;

/// File `response_tx` under `id` **before** the request goes on the wire, then
/// send it; a failed send un-files it and answers the caller with `on_error`.
///
/// Registering first makes "a reply can never arrive for an id nobody is
/// waiting for" true by construction instead of by the loop's current shape
/// (the stream is only polled after this returns). A reply that did find no
/// entry is dropped without a trace and the caller then waits out the 30 s
/// ceiling in `send_rpc` — and every `events.subscribe` queued behind it with
/// it. `send` receives the map so the order is assertable off the wasm target.
async fn send_tracked<E, Fut>(
    pending: &mut PendingRpcs,
    id: String,
    response_tx: oneshot::Sender<Result<Value, RpcFailure>>,
    send: impl FnOnce(&PendingRpcs) -> Fut,
    on_error: impl FnOnce(E) -> RpcFailure,
) where
    Fut: std::future::Future<Output = Result<(), E>>,
{
    pending.insert(id.clone(), response_tx);
    let sending = send(pending);
    if let Err(e) = sending.await {
        if let Some(tx) = pending.remove(&id) {
            let _ = tx.send(Err(on_error(e)));
        }
    }
}

/// Whether an approval-pending seed fetched at (`epoch_then`, `edits_then`) may
/// still be applied at (`epoch_now`, `edits_now`). The seed runs off the
/// subscription path now, so it can land after the socket was replaced (a list
/// read from a dead connection) or after an `approval.*` event already
/// refetched a newer list (the seed would overwrite it with an older one).
const fn approval_seed_applies(
    epoch_then: u64,
    epoch_now: u64,
    edits_then: u64,
    edits_now: u64,
) -> bool {
    epoch_then == epoch_now && edits_then == edits_now
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
    /// Bumped once per **successful handshake**, including the first. Any state
    /// a client derives from a stream of server-sequenced frames is only valid
    /// within one connection, so this is the edge on which such state must be
    /// re-based.
    ///
    /// It exists because the running-session set was not. `SessionMap::
    /// set_server_running` discards any `stream.running_set_changed` whose
    /// `seq` is `<=` the highest it has applied — correct for reordering
    /// *within* a socket, and quietly fatal across one: a restarted core
    /// begins its `SessionRunRegistry` seq at 0 again, so every frame from the
    /// new process was `<=` the old process's last seq and was dropped
    /// **forever**. The Panel's running dots froze at whatever they showed
    /// when the old process died, with no error anywhere. The cold-load seed
    /// could not repair it either: it only applies while `server_seq == 0` and
    /// only ran once, at mount.
    ///
    /// A monotonic counter rather than a server-reported instance id on
    /// purpose: a reconnect voids the baseline whether or not the core
    /// restarted (frames sent while we were offline are gone either way), so
    /// the client already knows everything it needs without a protocol change.
    pub connection_epoch: RwSignal<u64>,
    #[cfg(feature = "panel-modular")]
    pub panel_discovery: RwSignal<PanelDiscoveryState>,

    // Phase 3: Channel to send RPC requests to message loop
    rpc_tx: StoredValue<Option<mpsc::UnboundedSender<RpcRequest>>>,
    next_id: StoredValue<Arc<Mutex<u64>>>,

    // Phase 3: Event handling
    event_handlers: StoredValue<Arc<Mutex<Vec<Option<EventHandler>>>>>,

    /// Ledger of every topic pattern currently subscribed on behalf of this
    /// client, maintained by `subscribe_topic` / `unsubscribe_topic` (the two
    /// sole entry points).
    ///
    /// Subscriptions are **per-socket**: a reconnect opens a fresh `conn_id`
    /// whose gateway-side filter starts empty, and an empty filter means
    /// "receive everything" (`gateway/handlers/events.rs::should_receive`,
    /// `None => true`). So the replay after a reconnect does not merely restore
    /// — whatever it replays *narrows* the socket to exactly that set. Replaying
    /// a hardcoded list therefore silently kills every topic not on it; that is
    /// how `stream.*` / `team.*` used to die after the first reconnect while the
    /// connection indicator stayed green (component subscriptions are
    /// mount-only, and `ChatView` is never unmounted).
    ///
    /// The ledger is the single source for "what is this client subscribed to",
    /// so a new subscription site can never again forget to register itself for
    /// replay. It records only subscriptions the server **accepted** on the
    /// connection they were issued on (`subscribe_recorded`); there is
    /// deliberately no separate "intent" set - a failed subscribe is reported
    /// to its caller instead of being silently retried by the next replay.
    subscribed_topics: StoredValue<Arc<Mutex<BTreeSet<LedgerEntry>>>>,

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

    /// Pending telecom A/B approvals (`telecom.approval.pending`), rendered as
    /// `TelecomApprovalCard`s next to the exec approvals. Operator-only on the
    /// server; any fetch failure (e.g. `AUTH_REQUIRED` for a non-operator)
    /// collapses to an empty list rather than an error surface.
    pub pending_telecom_approvals:
        RwSignal<Vec<crate::state::telecom_approvals::TelecomApprovalView>>,
    /// One RPC at a time across mount polling, open refresh and event refresh.
    telecom_refresh_in_flight: StoredValue<Arc<AtomicBool>>,

    /// Approval subscription ID for cleanup.
    approval_subscription_id: StoredValue<Option<usize>>,

    /// Questions the agent is parked on (`ask_user`), rendered as inline cards
    /// in the conversation that is waiting. Pushed live by the `stream.ask_user`
    /// frame (`views::chat::events`) and seeded from `clarification.pending`
    /// when the chat surface mounts, so a reload mid-question still finds the
    /// blocked tool. Lives here — not on `ChatState` — because a question can
    /// belong to a background conversation whose `ChatState` isn't mounted.
    pub pending_clarifications: RwSignal<Vec<PendingAskView>>,

    // The `connect` response's `role` is DELIBERATELY not kept (2026-08-07).
    // It had exactly one reader, `is_operator()`, which had exactly one
    // consuming view — the cluster settings page — and a client-captured role
    // cannot be an enforcement point: it is stamped once at handshake, and
    // `handlers::users::restamp_live_connections` can promote or demote that
    // same live connection without telling the client, so the cached value is
    // wrong in both directions after a `users.update`. Authorization is
    // server-side (`method_admin.rs` for RPCs, `event_scope.rs` for topics);
    // a surface that a member may not use now learns so from the refusal it
    // gets back. Do not reintroduce this field to hide admin UI — that is the
    // same gate under a new name.
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

    /// Gateway-verified User of the current native connection. Set only by a
    /// `NativeOutcome::Protected` of the *current* connect attempt (after the
    /// Gateway accepted `connect` and the credential was stored). `None` in a
    /// plain browser, for the credentialless loopback compat connect, on every
    /// new attempt, on disconnect and on drop. Never derived from the URL or
    /// `localStorage`.
    pub protected_user: RwSignal<Option<String>>,

    /// Private native handshake state (attempt fence, in-memory bootstrap
    /// ticket). Not exposed: it can hold a one-shot ticket.
    native_auth: StoredValue<Arc<NativeAuthState>>,
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

/// `window.location.search`, or empty off-browser.
#[cfg(target_arch = "wasm32")]
fn location_search() -> String {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn location_search() -> String {
    String::new()
}

/// Friendly device label for the native handshake. Unlike
/// `panel_device_identity` it reads/writes no `localStorage` (the native device
/// id comes from the keyring or is minted in memory).
#[cfg(target_arch = "wasm32")]
fn native_device_name() -> String {
    web_sys::window()
        .map(|w| w.navigator())
        .and_then(|n| n.user_agent().ok())
        .map(|ua| friendly_device_name(&ua))
        .unwrap_or_else(|| "Panel".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn native_device_name() -> String {
    "Panel".to_string()
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
            connection_epoch: RwSignal::new(0),
            #[cfg(feature = "panel-modular")]
            panel_discovery: RwSignal::new(PanelDiscoveryState::new()),
            rpc_tx: StoredValue::new(None),
            next_id: StoredValue::new(Arc::new(Mutex::new(1))),
            event_handlers: StoredValue::new(Arc::new(Mutex::new(Vec::new()))),
            subscribed_topics: StoredValue::new(Arc::new(Mutex::new(BTreeSet::new()))),
            disconnect_tx: StoredValue::new(None),
            alerts: RwSignal::new(HashMap::new()),
            alert_subscription_id: StoredValue::new(None),
            pending_approvals: RwSignal::new(Vec::new()),
            pending_telecom_approvals: RwSignal::new(Vec::new()),
            telecom_refresh_in_flight: StoredValue::new(Arc::new(AtomicBool::new(false))),
            approval_subscription_id: StoredValue::new(None),
            pending_clarifications: RwSignal::new(Vec::new()),
            needs_token: RwSignal::new(false),
            token_was_rejected: RwSignal::new(false),
            connection_failure: RwSignal::new(None),
            protected_user: RwSignal::new(None),
            native_auth: StoredValue::new(Arc::new(NativeAuthState::default())),
        }
        .with_native_cleanup()
    }

    /// Owner cleanup invalidates every outstanding native attempt so a late
    /// reply can never write a credential or set `protected_user` after the
    /// owner is gone.
    fn with_native_cleanup(self) -> Self {
        let auth = self.native_auth.get_value();
        on_cleanup(move || auth.invalidate());
        self
    }

    /// Capture the `needs_token` verdict from a `connect` response. A missing
    /// field resets to the safe default (not walled), keeping the signal
    /// consistent across reconnects. The response also carries a `role`; it is
    /// deliberately ignored — see the note above the `needs_token` field for
    /// why the client keeps no copy of it.
    fn capture_connect_verdict(&self, resp: &Value) {
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

    /// Open the native pairing prompt without persisting or redirecting a ticket.
    pub fn request_native_pairing(&self) {
        if !native_expected() {
            return;
        }
        let state = *self;
        spawn_local(async move {
            let _ = state.disconnect().await;
            let _ = state.needs_token.try_set(true);
            let _ = state.token_was_rejected.try_set(false);
            let _ = state
                .connection_failure
                .try_set(Some(ConnectionFailure::AuthRequired));
        });
    }

    /// Native mode accepts an in-memory pairing ticket; the browser compatibility
    /// path retains its existing ticket/device/shared-token login behavior.
    pub fn submit_token(&self, token: String) {
        let token = token.trim().to_string();
        if token.is_empty() {
            return;
        }
        if native_expected() {
            if !matches!(
                classify_credential(&token),
                SubmittedCredential::BootstrapTicket
            ) {
                let _ = self.token_was_rejected.try_set(true);
                return;
            }
            // Keep the pairing form mounted while the native connection is
            // replaced, including when pairing starts from the welcome screen.
            let _ = self.needs_token.try_set(true);
            let _ = self.token_was_rejected.try_set(false);
            let state = *self;
            spawn_local(async move {
                let _ = state.disconnect().await;
                let Some(auth) = state.native_auth.try_get_value() else {
                    return;
                };
                auth.stash_ticket(token);
                let _ = state.connect().await;
            });
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
        // Reuse a vacated slot before growing: a long-lived session mounts
        // and unmounts views constantly, and an append-only Vec of dead
        // closures is a slow leak (each tombstone still rode every dispatch).
        if let Some((id, slot)) = handlers
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.is_none())
        {
            *slot = Some(Arc::new(handler));
            id
        } else {
            let id = handlers.len();
            handlers.push(Some(Arc::new(handler)));
            id
        }
    }

    /// Unsubscribe from events
    pub fn unsubscribe_events(&self, id: usize) {
        let handlers = self.event_handlers.with_value(std::clone::Clone::clone);
        let mut handlers = handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if id < handlers.len() {
            // Vacate the slot instead of removing to preserve indices
            // (ids held by other components must not shift).
            handlers[id] = None;
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
        // Copy the (Arc) handlers out under the lock, then drop the guard
        // before invoking any of them. A handler that synchronously calls
        // subscribe_events / unsubscribe_events on the same `event_handlers`
        // Mutex would otherwise re-enter the lock on the same thread and
        // deadlock (Wasm is single-threaded — no `try_lock` escape hatch).
        // Bind the `&Arc<Mutex<...>>` to a let first so the `MutexGuard`
        // borrows from a named location rather than a chain temporary.
        let store = self.event_handlers.with_value(std::clone::Clone::clone);
        let handlers = {
            let guard = store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.iter().flatten().cloned().collect::<Vec<_>>()
        };
        for handler in handlers {
            handler(event.clone());
        }
    }

    /// Subscribe to a specific event topic on the Gateway.
    ///
    /// The pattern is filed in `subscribed_topics` only **after** the RPC
    /// succeeded on the connection it was issued on: the ledger is what a
    /// reconnect replays, so it records what the server accepted, not what was
    /// asked for (see `subscribe_recorded`). A failed subscribe therefore
    /// returns `Err` and leaves no entry - the caller decides whether to retry.
    /// A subscribe whose reply lands after a reconnect is re-issued once on the
    /// new socket, because the replay (which only knows ledger entries) will
    /// not cover it. A duplicate server-side subscribe is idempotent.
    pub async fn subscribe_topic(&self, pattern: &str) -> Result<(), String> {
        self.subscribe_topic_except(pattern, &[]).await
    }

    /// [`Self::subscribe_topic`], minus the topics in `except` - for a surface
    /// that never renders them. Unsubscribe with the same `except`
    /// ([`Self::unsubscribe_topic_except`]): the carve-out names the
    /// subscription, so another surface's plain subscription to the same
    /// pattern is neither narrowed nor removed by this one.
    pub async fn subscribe_topic_except(
        &self,
        pattern: &str,
        except: &[&str],
    ) -> Result<(), String> {
        /// One re-issue after a reconnect raced the reply; more would just
        /// chase a flapping socket.
        const ATTEMPTS: u32 = 2;
        let except: Vec<String> = except.iter().map(|e| (*e).to_string()).collect();
        let ledger = self.subscribed_topics.with_value(std::clone::Clone::clone);
        let mut last = SubscribeError::Superseded;
        for _ in 0..ATTEMPTS {
            if let Err(message) = self.await_gateway_ready().await {
                last = SubscribeError::Rpc(RpcFailure::local(message));
                break;
            }
            // Captured after readiness and before the request goes out: this
            // is the connection the request will be sent on.
            let Some(epoch_then) = self.connection_epoch.try_get_untracked() else {
                break;
            };
            let result = subscribe_recorded(
                &ledger,
                (pattern.to_string(), except.clone()),
                epoch_then,
                || {
                    let connected = self.is_connected.try_get_untracked().unwrap_or(false);
                    connected
                        .then(|| self.connection_epoch.try_get_untracked())
                        .flatten()
                },
                self.send_rpc("events.subscribe", topics_request(pattern, &except)),
            )
            .await;
            log_subscribe_diagnostic(pattern, &result);
            match result {
                Ok(()) => return Ok(()),
                Err(SubscribeError::Superseded) => last = SubscribeError::Superseded,
                Err(e) => return Err(e.into_message()),
            }
        }
        Err(last.into_message())
    }

    /// Subscribe **without** filing the pattern in the reconnect ledger.
    ///
    /// For component-scoped catch-all subscriptions (the home activity
    /// feed's `"**"`): the owning component re-subscribes from its own
    /// connect-driven effect on every reconnect, so a ledger entry would only
    /// outlive the component — turning every later reconnect of *this*
    /// socket into a receive-everything stream long after the view that
    /// wanted it is gone. The caller must pair this with
    /// [`Self::unsubscribe_topic`] on unmount.
    pub async fn subscribe_topic_ephemeral(&self, pattern: &str) -> Result<(), String> {
        self.rpc_call("events.subscribe", topics_request(pattern, &[]))
            .await
            .map(|_| ())
    }

    /// Unsubscribe from an event topic
    pub async fn unsubscribe_topic(&self, pattern: &str) -> Result<(), String> {
        self.unsubscribe_topic_except(pattern, &[]).await
    }

    /// Undo [`Self::subscribe_topic_except`] with the same `except`.
    pub async fn unsubscribe_topic_except(
        &self,
        pattern: &str,
        except: &[&str],
    ) -> Result<(), String> {
        let except: Vec<String> = except.iter().map(|e| (*e).to_string()).collect();
        self.rpc_call("events.unsubscribe", topics_request(pattern, &except))
            .await?;
        self.subscribed_topics.with_value(|set| {
            set.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&(pattern.to_string(), except));
        });
        Ok(())
    }

    /// Every subscription this socket must (re-)subscribe to after a connect.
    fn topics_to_replay(&self) -> Vec<LedgerEntry> {
        let ledger = self.subscribed_topics.with_value(|set| {
            set.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        });
        replay_set(&ledger)
    }

    /// Make an RPC call to the gateway, waiting for the socket if it is still
    /// connecting.
    ///
    /// # Why this waits
    ///
    /// `Self::send_rpc` answers "the socket is not up yet" with
    /// `Err("Not connected")` — a value indistinguishable from a verdict about
    /// the call. Every mount-time load in the Panel inherited that confusion: a
    /// page opened by URL or reloaded (as opposed to navigated to inside the
    /// SPA) issues its first read while `connect()` is still in flight, gets a
    /// failure back, renders whatever it was initialized with, and **never
    /// retries**. It reproduced 100% of the time, which is why it read as
    /// "this page is broken" rather than as a race.
    ///
    /// Sixty-eight mount-reachable call sites had been answering that question
    /// independently, in five different idioms (a tracked `Effect`, a bare
    /// `spawn_local`, a 3×500 ms retry loop, a 50×100 ms poll, a pure
    /// `should_fetch` predicate). Twenty-eight answered it wrong — thirteen
    /// fired outside any reactive scope, fifteen from an `Effect` that never
    /// read `is_connected` — and every one of those files had passing tests.
    /// So the floor lives here instead: a caller that never heard of
    /// `is_connected` still cannot ask too early.
    ///
    /// # What it does not do
    ///
    /// It does not re-run anything after a reconnect — that is a different
    /// question ("ask again", not "do not ask too early") and it is answered
    /// per page, by an `Effect` that reads `is_connected` alongside whatever
    /// else that page reloads on. See `WorkspacesView` (tracks
    /// `include_archived`) and `galaxy::GalaxyView` (tracks "agent list still
    /// empty", because re-running its fetch would discard the user's
    /// selection).
    ///
    /// A generic `on_gateway_ready(state, load)` helper was considered and
    /// **withdrawn under R10**: every site that needs re-running needs it
    /// keyed on additional signals, so a helper that knows only about
    /// `is_connected` would be strictly less capable than the four-line
    /// `Effect` it replaced, and would have had no callers.
    ///
    /// # The `String` face is a projection
    ///
    /// The wire hands back a full JSON-RPC error object; this method keeps
    /// `message` alone, derived from [`RpcFailure`] at exactly this one site,
    /// so every existing `String`-error consumer (and the `admin_refusal`
    /// classifier behind them) receives byte-identical text. A caller that
    /// needs the error **code** uses [`Self::rpc_call_with_code`] — never a
    /// re-parse of the message text.
    pub async fn rpc_call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.rpc_call_with_code(method, params)
            .await
            .map_err(|failure| failure.message)
    }

    /// [`Self::rpc_call`], keeping the JSON-RPC error code.
    ///
    /// Same readiness floor, same single send path — the only difference is
    /// that the `Err` arm carries what the server actually said instead of
    /// its `message` projection. Born for the canvas conflict classifier
    /// (`api/canvas.rs` branches on `REVISION_CONFLICT`); any future caller
    /// that needs to branch on a code belongs here too.
    pub async fn rpc_call_with_code(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcFailure> {
        if let Err(message) = self.await_gateway_ready().await {
            return Err(RpcFailure::local(message));
        }
        self.send_rpc(method, params).await
    }

    /// Refresh the complete canonical Panel snapshot for the current connection.
    #[cfg(feature = "panel-modular")]
    pub async fn refresh_panel_discovery(&self) -> Result<(), String> {
        let Some(epoch) = self.connection_epoch.try_get_untracked() else {
            return Err("Panel discovery owner was disposed".to_string());
        };
        self.panel_discovery
            .update(|state| state.begin_connection(epoch));
        let response = self
            .rpc_call(
                aleph_protocol::panel::PANEL_UI_CAPABILITY_SNAPSHOT_METHOD,
                Value::Null,
            )
            .await;
        match response {
            Ok(value) => match decode_snapshot(value) {
                Ok(snapshot) => {
                    match self
                        .panel_discovery
                        .try_update(|state| state.accept_snapshot(epoch, snapshot))
                    {
                        Some(
                            SnapshotUpdate::Accepted
                            | SnapshotUpdate::StaleEpoch
                            | SnapshotUpdate::StaleRevision,
                        ) => Ok(()),
                        None => Err("Panel discovery owner was disposed".to_string()),
                    }
                }
                Err(error) => {
                    let message = format!("Panel discovery payload could not be decoded: {error}");
                    self.panel_discovery
                        .update(|state| state.mark_error(epoch, message.clone()));
                    Err(message)
                }
            },
            Err(message) => {
                self.panel_discovery
                    .update(|state| state.mark_error(epoch, message.clone()));
                Err(message)
            }
        }
    }

    /// Park until the socket is authorized, bounded by
    /// [`GATEWAY_READY_TIMEOUT_MS`].
    ///
    /// Returns immediately on the hot path — once connected this costs one
    /// untracked signal read and never yields, so a warm Panel is unaffected.
    async fn await_gateway_ready(&self) -> Result<(), String> {
        let mut waited = 0_u32;
        loop {
            // `try_get_untracked`, not `get_untracked`, because this loop reads
            // signals on the far side of an `.await` — see `crate::disposed_reads`,
            // whose rule admits no exceptions even for root-owned signals. Note
            // that guard's scanner only walks `spawn_local` blocks and cannot see
            // an `async fn` like this one, so this is the rule being followed,
            // not the rule being enforced.
            //
            // `None` means the whole `DashboardState` scope is gone (the page is
            // tearing down). Give up rather than park a task nothing can wake.
            let (Some(connected), Some(walled)) = (
                self.is_connected.try_get_untracked(),
                self.needs_token.try_get_untracked(),
            ) else {
                return Err(GATEWAY_NOT_READY.to_string());
            };
            match gateway_readiness(connected, walled) {
                GatewayReadiness::Ready => return Ok(()),
                GatewayReadiness::Walled => return Err(GATEWAY_NOT_READY.to_string()),
                GatewayReadiness::TooEarly if waited >= GATEWAY_READY_TIMEOUT_MS => {
                    return Err(GATEWAY_NOT_READY.to_string())
                }
                GatewayReadiness::TooEarly => {}
            }
            TimeoutFuture::new(GATEWAY_READY_POLL_MS).await;
            waited += GATEWAY_READY_POLL_MS;
        }
    }

    /// Put a request on the wire **without** waiting for authorization.
    ///
    /// Private, and it has exactly one legitimate caller: [`Self::handshake`],
    /// whose `connect` call is the thing that makes the socket authorized in
    /// the first place. Routing it through [`Self::rpc_call`] would deadlock —
    /// the handshake would wait for a state only the handshake can produce.
    ///
    /// That carve-out is a *separate function* rather than a `method ==
    /// "connect"` test on purpose: an exemption spelled as a string compare is
    /// one rename away from silently covering something else, and one new
    /// unauthorized method away from being wrong.
    async fn send_rpc(&self, method: &str, params: Value) -> Result<Value, RpcFailure> {
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
                tx.unbounded_send(request).map_err(|e| {
                    // Keep the source error so triage can tell a closed channel
                    // from a full one (the old "Failed to send RPC request"
                    // string lumped every failure mode together).
                    RpcFailure::local(format!("Failed to send RPC request: {e}"))
                })?;
            } else {
                return Err(RpcFailure::local("Not connected"));
            }
        }

        // Wait for response, but don't hang forever. `onclose` covers the
        // socket-close case (it drops the pending sender → "Response channel
        // closed"); this 30s ceiling additionally covers a server that accepts
        // the request and then never replies without closing the socket,
        // converting an infinite spinner into a surfaced, retryable error.
        use futures::future::{select, Either};
        match select(response_rx, TimeoutFuture::new(30_000)).await {
            Either::Left((res, _)) => {
                res.map_err(|e| RpcFailure::local(format!("Response channel closed: {e}")))?
            }
            Either::Right(((), _)) => Err(RpcFailure::local("Request timed out")),
        }
    }

    /// Handshake outcome — distinguishes "authorized", "needs a token"
    /// (login wall, NOT a transport failure), and a transport-level failure.
    async fn handshake(&self, receipt: Option<AttemptReceipt>, dialed_url: &str) -> Handshake {
        // Native shell: the OS keyring is the only credential store. Decide
        // BEFORE `panel_device_identity` / `read_connect_credentials`, both of
        // which touch localStorage.
        if native_expected() {
            return self.native_handshake_branch(receipt, dialed_url).await;
        }
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

        // `send_rpc`, not `rpc_call`: this call is what makes the socket
        // authorized, so waiting for authorization here would deadlock.
        let response = self.send_rpc("connect", params).await;
        if !attempt_current(&receipt)
            || self.gateway_url.try_get_untracked().as_deref() != Some(dialed_url)
        {
            return Handshake::Superseded;
        }
        let resp = match response {
            Ok(r) => r,
            Err(e) => {
                return Handshake::Failed(classify(
                    FailureStage::Handshake,
                    Some(&e.message),
                    false,
                ))
            }
        };
        self.capture_connect_verdict(&resp);
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

    /// Native handshake via the Bridge keyring. Reuses the existing
    /// `connect` RPC on the already-open socket (`send_rpc`), so no second
    /// socket exists. Every state write after an await is fenced by the
    /// attempt receipt and uses `try_set`.
    async fn native_handshake_branch(
        &self,
        receipt: Option<AttemptReceipt>,
        dialed_url: &str,
    ) -> Handshake {
        let Some(receipt) = receipt else {
            // Attempt counter exhausted: fail closed.
            return Handshake::Failed(classify(
                FailureStage::Handshake,
                Some("Gateway connection failed"),
                false,
            ));
        };
        let state = *self;
        let auth = self.native_auth.get_value();
        // A pairing ticket from the URL goes to memory and leaves the URL; it
        // is never written to localStorage.
        if let Some(ticket) = ticket_from_search(&location_search()) {
            auth.stash_ticket(ticket);
            scrub_credentials_from_url();
        }
        let device_name = native_device_name();
        let dialed = dialed_url.to_string();
        let current = || {
            receipt.is_current()
                && state.gateway_url.try_get_untracked().as_deref() == Some(dialed.as_str())
        };
        let send_receipt = receipt.clone();
        let outcome = native_handshake(
            &TauriIpc,
            HandshakeRequest {
                dialed_url: &dialed,
                device_name: &device_name,
                auth: &auth,
                new_device_id: &new_device_id,
            },
            &current,
            move |params| async move {
                match state.send_rpc("connect", params).await {
                    Ok(resp) => {
                        if send_receipt.is_current() {
                            let walled = resp
                                .get("needs_token")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);
                            let _ = state.needs_token.try_set(walled);
                        }
                        Ok(resp)
                    }
                    // The transport message can echo the dialed URL: drop it.
                    Err(_) => Err(ConnectSendError),
                }
            },
        )
        .await;
        if !current() {
            return Handshake::Superseded;
        }
        match outcome {
            NativeOutcome::Protected { user_id } => {
                let _ = state.needs_token.try_set(false);
                let _ = state.token_was_rejected.try_set(false);
                let _ = state.protected_user.try_set(Some(user_id));
                Handshake::Authorized
            }
            // Works, but is NOT protected.
            NativeOutcome::Credentialless => {
                let _ = state.needs_token.try_set(false);
                let _ = state.token_was_rejected.try_set(false);
                Handshake::Authorized
            }
            NativeOutcome::NeedsToken { was_rejected } => {
                let _ = state.protected_user.try_set(None);
                Handshake::NeedsToken { was_rejected }
            }
            NativeOutcome::Superseded => Handshake::Superseded,
            NativeOutcome::Failed(f) => {
                let _ = state.protected_user.try_set(None);
                Handshake::Failed(classify(FailureStage::Handshake, Some(f.message()), false))
            }
        }
    }

    /// Connect to the gateway
    pub async fn connect(&self) -> Result<(), String> {
        let auth = self
            .native_auth
            .try_get_value()
            .ok_or("Connection owner closed")?;
        let receipt = auth.begin();
        if receipt.is_none() {
            let _ = self.disconnect().await;
            return Err("Connection attempts exhausted".to_string());
        }
        self.connect_attempt(receipt).await
    }

    async fn connect_attempt(&self, receipt: Option<AttemptReceipt>) -> Result<(), String> {
        // Every attempt starts a fresh fence and forgets the previous User; a
        // plain browser simply never sets it again.
        if !attempt_current(&receipt) {
            return Ok(());
        }
        let native = native_expected();
        let _ = self.protected_user.try_set(None);
        let _ = self.is_connected.try_set(false);
        let mut previous = None;
        let _ = self
            .disconnect_tx
            .try_update_value(|slot| previous = slot.take());
        if let Some(previous) = previous {
            let _ = previous.send(());
        }
        let _ = self.rpc_tx.try_set_value(None);
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
        if !attempt_current(&receipt)
            || self.gateway_url.try_get_untracked().as_deref() != Some(url.as_str())
        {
            let _ = connector.disconnect().await;
            return Ok(());
        }
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
                let loop_receipt = receipt.clone();
                let Some(loop_auth) = self.native_auth.try_get_value() else {
                    let _ = connector.disconnect().await;
                    return Ok(());
                };
                let loop_url = url.clone();

                // Spawn message loop task that owns the connector
                spawn_local(async move {
                    web_sys::console::log_1(&"Message loop started".into());

                    let mut stream = stream.fuse();
                    let mut rpc_rx = rpc_rx.fuse();
                    let mut disconnect_rx = disconnect_rx.fuse();
                    let mut pending_rpcs: PendingRpcs = HashMap::new();
                    // Track whether the loop exited because of an explicit
                    // disconnect() call (no auto-reconnect) or an unintentional
                    // drop (auto-reconnect to drive ConnectionPhase::Reconnecting
                    // → ServiceBlockingGate). Captured drop_reason becomes the
                    // connection_error surfaced in the UI.
                    let mut intentional_close = false;
                    let mut drop_reason: Option<String> = None;

                    loop {
                        if !attempt_current(&loop_receipt)
                            || state.gateway_url.try_get_untracked().as_deref()
                                != Some(loop_url.as_str())
                        {
                            let _ = connector.disconnect().await;
                            break;
                        }
                        // Use futures::select! to handle multiple async operations
                        futures::select! {
                            // Handle incoming RPC requests
                            rpc_req = rpc_rx.select_next_some() => {
                                if !attempt_current(&loop_receipt) || state.gateway_url.try_get_untracked().as_deref() != Some(loop_url.as_str()) { break; }
                                let credential_request = rpc_req.method == "connect";
                                // Build JSON-RPC request
                                let request = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": rpc_req.id.clone(),
                                    "method": rpc_req.method,
                                    "params": rpc_req.params
                                });

                                // Register the pending response, then send
                                // (see `send_tracked`).
                                send_tracked(
                                    &mut pending_rpcs,
                                    rpc_req.id,
                                    rpc_req.response_tx,
                                    |_| connector.send(request),
                                    |e| {
                                        let detail = if native || credential_request {
                                            "Gateway request could not be sent".to_string()
                                        } else { e.to_string() };
                                        web_sys::console::error_1(&detail.clone().into());
                                        RpcFailure::local(detail)
                                    },
                                ).await;
                            }

                            // Handle incoming WebSocket messages. `next()` preserves
                            // the stream-end signal; `select_next_some()` turns a clean
                            // remote close into a permanently pending/panicking branch,
                            // which leaves `is_connected` true and prevents reconnect.
                            msg = stream.next() => {
                                if !attempt_current(&loop_receipt) || state.gateway_url.try_get_untracked().as_deref() != Some(loop_url.as_str()) { break; }
                                let Some(msg) = msg else {
                                    drop_reason = Some(
                                        "WebSocket stream ended".to_string(),
                                    );
                                    break;
                                };
                                match msg {
                                    Ok(value) => {
                                        // Never log per-frame breadcrumbs here: a streaming run
                                        // fires this branch per token, and payloads can carry
                                        // secrets/content that must not reach the console.

                                        // Check if this is an RPC response (has 'id' field)
                                        if let Some(id) = value.get("id").and_then(|id| id.as_str()) {
                                            // Handle RPC response
                                            if let Some(tx) = pending_rpcs.remove(id) {
                                                if let Some(error) = value.get("error") {
                                                    let _ = tx.send(Err(parse_rpc_error(error)));
                                                } else if let Some(result) = value.get("result") {
                                                    let _ = tx.send(Ok(result.clone()));
                                                }
                                            }
                                        } else {
                                            // This is an event notification
                                            // Parse event format: { "method": "event", "params": { "topic": "...", "data": {...} } }
                                            //
                                            // The method name comes from the
                                            // shared constant, not a literal:
                                            // it is a wire key, and this was
                                            // the sixth writer of it — the one
                                            // the constant was introduced to
                                            // abolish and then missed, on the
                                            // client side of the same wire.
                                            if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                                                if method == aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD {
                                                    if let Some(params) = value.get("params") {
                                                        if let Some(topic) = params.get("topic").and_then(|t| t.as_str()) {
                                                            let data = params.get("data").cloned().unwrap_or(Value::Null);

                                                            let event = GatewayEvent {
                                                                topic: topic.to_string(),
                                                                data,
                                                            };

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
                                                    state.dispatch_event(event);
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let detail = if native {
                                            if matches!(classify(FailureStage::AfterOpen, Some(&e.to_string()), false), ConnectionFailure::AuthRequired) {
                                                "auth required".to_string()
                                            } else { "Protected connection closed".to_string() }
                                        } else { format!("WebSocket dropped: {e}") };
                                        web_sys::console::error_1(&detail.clone().into());
                                        drop_reason = Some(detail);
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

                    let _ = connector.disconnect().await;
                    web_sys::console::log_1(&"Message loop stopped".into());
                    if !attempt_current(&loop_receipt) {
                        return;
                    }

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
                        // Only the newest attempt may forget the User; a stale
                        // loop must not clear a newer attempt's state.
                        if attempt_current(&loop_receipt) {
                            loop_auth.invalidate();
                            let _ = state.protected_user.try_set(None);
                        }
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
                            if !native {
                                clear_credentials();
                            }
                        }
                        state.is_connected.set(false);
                        // Clear the dead rpc_tx so the next rpc_call() won't
                        // block on a sender whose receiver task just exited.
                        state.rpc_tx.set_value(None);
                        let retry_receipt = loop_auth.begin();
                        spawn_local(async move {
                            let _ = state.reconnect_attempt(retry_receipt, loop_url).await;
                        });
                    }
                });

                // Complete the session handshake before marking as connected.
                let handshake_state = *self;
                let handshake = handshake_state.handshake(receipt.clone(), &url).await;
                if !attempt_current(&receipt) {
                    return Ok(());
                }
                if native
                    && matches!(
                        &handshake,
                        Handshake::NeedsToken { .. } | Handshake::Failed(_) | Handshake::Superseded
                    )
                {
                    let mut sender = None;
                    let _ = self
                        .disconnect_tx
                        .try_update_value(|slot| sender = slot.take());
                    if let Some(sender) = sender {
                        let _ = sender.send(());
                    }
                    let _ = self.rpc_tx.try_set_value(None);
                }
                match handshake {
                    Handshake::Superseded => Ok(()),
                    Handshake::Authorized => {
                        self.is_connected.set(true);
                        self.connection_error.set(None);
                        self.connection_failure.set(None);
                        self.reconnect_count.set(0);
                        self.is_reconnecting.set(false);
                        self.has_connected_once.set(true);
                        // Bumped in the same place the topic ledger is
                        // replayed below, and for the same reason: a fresh
                        // socket starts from nothing. Anything a component
                        // derived from server-sequenced frames on the previous
                        // connection is now a baseline that can only cause
                        // frames to be discarded — see the field's doc.
                        self.connection_epoch.update(|n| *n += 1);

                        #[cfg(feature = "panel-modular")]
                        {
                            let state_for_panel_discovery = *self;
                            spawn_local(async move {
                                let _ = state_for_panel_discovery.refresh_panel_discovery().await;
                            });
                        }

                        let state_for_subscribe = *self;
                        spawn_local(async move {
                            // Per-socket topic subscriptions must be re-established on
                            // every (re)connect: a reconnect opens a fresh conn_id whose
                            // gateway filter starts empty. Replay the *ledger*, not a
                            // literal list — an empty filter receives everything, so
                            // whatever we replay here is also what we narrow the socket
                            // down to. A hardcoded list silently kills every topic
                            // subscribed by a component that has since mounted
                            // (`stream.*`, `team.*`, `team.*.task.*`, artifacts…), and
                            // those mount-only subscriptions never re-run because
                            // `ChatView` is never unmounted. subscribe_topic is idempotent
                            // on the gateway, so the one-time client-side handlers stay
                            // registered.
                            let failed = replay_subscriptions(
                                state_for_subscribe.topics_to_replay(),
                                |params| state_for_subscribe.rpc_call("events.subscribe", params),
                            )
                            .await;
                            // Topic + kind only: the server's message may echo
                            // params or credentials (see `subscribe_diagnostic`).
                            for (topic, _message) in failed {
                                web_sys::console::error_1(
                                    &format!("events.subscribe {topic}: error (replay)").into(),
                                );
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
                if !attempt_current(&receipt) {
                    return Ok(());
                }
                let stage = stage_for_connect_error(&e, origin);
                let detail = if native {
                    "Gateway connection failed".to_string()
                } else {
                    e.to_string()
                };
                self.set_failure(classify(stage, Some(&detail), false));
                Err(detail)
            }
        }
    }

    /// Disconnect from the gateway
    pub async fn disconnect(&self) -> Result<(), String> {
        // A queued native action may first run after its reactive owner is gone.
        let Some(auth) = self.native_auth.try_get_value() else {
            return Ok(());
        };
        // Cleanup alert subscriptions first
        self.cleanup_alert_subscriptions();

        // Invalidate every outstanding native attempt and forget the User.
        auth.invalidate();
        let _ = self.protected_user.try_set(None);

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
        #[cfg(feature = "panel-modular")]
        if let Some(epoch) = self.connection_epoch.try_get_untracked() {
            self.panel_discovery.update(|state| state.disconnect(epoch));
        }
        self.connection_error.set(None);
        self.is_reconnecting.set(false);
        Ok(())
    }

    /// Attempt to reconnect. Differentiated by failure type: an `AuthRequired`
    /// failure breaks out immediately to the login wall (retrying the same bad
    /// token is wasted); every other class uses exponential backoff with
    /// downward jitter, reusing the shared `ReconnectStrategy`.
    pub async fn reconnect(&self) -> Result<(), String> {
        let Some(auth) = self.native_auth.try_get_value() else {
            return Ok(());
        };
        let Some(scope) = self.gateway_url.try_get_untracked() else {
            return Ok(());
        };
        self.reconnect_attempt(auth.begin(), scope).await
    }

    async fn reconnect_attempt(
        &self,
        mut retry_receipt: Option<AttemptReceipt>,
        scope: String,
    ) -> Result<(), String> {
        if !attempt_current(&retry_receipt)
            || self.gateway_url.try_get_untracked().as_deref() != Some(scope.as_str())
        {
            return Ok(());
        }
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

        let Some(auth) = self.native_auth.try_get_value() else {
            return Ok(());
        };
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
            if !attempt_current(&retry_receipt)
                || self.gateway_url.try_get_untracked().as_deref() != Some(scope.as_str())
            {
                return Ok(());
            }
            retry_receipt = auth.begin();
            let result = self.connect_attempt(retry_receipt.clone()).await;
            if !attempt_current(&retry_receipt) {
                return Ok(());
            }
            match result {
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

        // Subscribe last: the handler above exists even when this fails (the
        // reconnect replay seeds `alerts.**` regardless), and a slow or failing
        // subscribe cannot keep it from being registered.
        self.subscribe_topic("alerts.**").await
    }

    /// Refetch pending telecom approvals. `Some(Ok(()))` means the operator-only
    /// method succeeded; `Some(Err)` carries a refusal/failure; `None` means a
    /// refresh was already in flight. Non-operators see an empty list, never a
    /// private error surface. A single gate keeps event/open/poll refreshes
    /// from racing and painting an older snapshot over a newer one.
    pub async fn refresh_telecom_approvals(&self) -> Option<Result<(), RpcFailure>> {
        let in_flight = self.telecom_refresh_in_flight.with_value(Clone::clone);
        if in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        let _guard = TelecomRefreshGuard(in_flight);
        let result = TelecomApprovalApi::list_pending(self).await;
        match result {
            Ok(list) => {
                self.pending_telecom_approvals.set(list);
                Some(Ok(()))
            }
            Err(error) => {
                self.pending_telecom_approvals.set(Vec::new());
                Some(Err(error))
            }
        }
    }

    /// Install a view-scoped 4s poll. Continue after an authorized `pending`
    /// response; an initial failure or a role refusal stops this run.
    /// Reconnect starts a fresh run, while cleanup stops a disposed view.
    /// No telecom metadata is broadcast via the shared event stream.
    pub fn poll_telecom_approvals_while_mounted(&self) {
        let state = *self;
        let mounted = Arc::new(AtomicBool::new(true));
        let cleanup = mounted.clone();
        on_cleanup(move || cleanup.store(false, Ordering::Release));
        Effect::new(move |previous: Option<Arc<AtomicBool>>| {
            if let Some(previous) = previous {
                previous.store(false, Ordering::Release);
            }
            let connected = state.is_connected.get();
            state.connection_epoch.track();
            let current = Arc::new(AtomicBool::new(true));
            if connected {
                let current_run = current.clone();
                let mounted_run = mounted.clone();
                spawn_local(async move {
                    let mut authorized = false;
                    loop {
                        if !mounted_run.load(Ordering::Acquire)
                            || !current_run.load(Ordering::Acquire)
                            || state.is_connected.try_get_untracked() != Some(true)
                        {
                            break;
                        }
                        let outcome = state.refresh_telecom_approvals().await;
                        if !telecom_poll_continues(authorized, &outcome) {
                            break;
                        }
                        authorized |= matches!(outcome, Some(Ok(())));
                        // Sequential awaits and the shared gate prevent overlap.
                        TimeoutFuture::new(TELECOM_APPROVAL_POLL_MS).await;
                    }
                });
            }
            current
        });
    }

    /// Subscribe to `approval.**` events so the `NotificationCenter` can render
    /// inline operator approval cards. The `ApprovalRequested` event is sparse
    /// (ids only), so `exec.approvals.pending` is the source of truth: any
    /// approval event simply triggers a refetch.
    pub async fn setup_approval_subscriptions(&self) -> Result<(), String> {
        // The refetch counter lets the seed below notice that an event already
        // delivered a newer list.
        let refetches = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let handler_refetches = Arc::clone(&refetches);
        let state = *self;
        let subscription_id =
            self.subscribe_events(move |event: GatewayEvent| match event.topic.as_str() {
                // `approval.reminder` is deliberately absent: it announces
                // nothing new about the pending set, so refetching on it would
                // be one round trip per reminder for a list that cannot have
                // changed. Its audience is the desktop shell's OS banner.
                "approval.requested" | "approval.resolved" | "approval.expired" => {
                    handler_refetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    spawn_local(async move {
                        if let Ok(list) = ExecApprovalApi::list_pending(&state).await {
                            state.pending_approvals.set(list);
                        }
                        state.refresh_telecom_approvals().await;
                    });
                }
                _ => {}
            });

        self.approval_subscription_id
            .set_value(Some(subscription_id));

        // Seed with whatever is already pending at connect time — AFTER the
        // handler exists (an event during the fetch is no longer missed) and
        // off this path: the mount effect awaits this method before it would
        // otherwise move on, so a slow `exec.approvals.pending` (30 s ceiling)
        // must not hold anything behind it. One task per call, one bounded RPC,
        // applied only if still current.
        let epoch = self
            .connection_epoch
            .try_get_untracked()
            .unwrap_or(u64::MAX);
        let edits = refetches.load(std::sync::atomic::Ordering::SeqCst);
        let seed_state = *self;
        spawn_local(async move {
            if let Ok(list) = ExecApprovalApi::list_pending(&seed_state).await {
                let applies = approval_seed_applies(
                    epoch,
                    seed_state
                        .connection_epoch
                        .try_get_untracked()
                        .unwrap_or(u64::MAX),
                    edits,
                    refetches.load(std::sync::atomic::Ordering::SeqCst),
                );
                if applies {
                    let _ = seed_state.pending_approvals.try_set(list);
                }
            }
            seed_state.refresh_telecom_approvals().await;
        });

        // Subscribe last, same reason as `setup_alert_subscriptions`.
        self.subscribe_topic("approval.**").await
    }

    /// Load initial alert states from Gateway
    ///
    /// This method fetches the current alert states when the UI first connects,
    /// ensuring that existing alerts are displayed even if no new events arrive.
    ///
    /// # Implementation Note
    ///
    /// Currently uses direct `rpc_call()` methods instead of a shared alerts API.
    /// The old `AlertsApi` lived in a `shared_ui_logic` crate that no longer exists in this tree
    /// (the shared crates now live under `shared/`), and it used a different `RpcClient`
    /// implementation that is incompatible with the current architecture.
    ///
    /// **TODO**: Refactor to a shared alerts API once the shared crates are unified and the
    /// `RpcClient` implementations are aligned.
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
mod native_lifecycle_tests {
    use super::*;

    #[test]
    fn a_delayed_retry_does_not_supersede_a_new_connection() {
        let owner = Owner::new();
        owner.with(|| {
            let state = DashboardState::new();
            let scope = state.gateway_url.get_untracked();
            let auth = state.native_auth.get_value();
            let retry = state.reconnect_attempt(auth.begin(), scope);
            let new_attempt = auth.begin().unwrap();
            state.protected_user.set(Some("new-user".into()));
            state.is_connected.set(true);
            futures::executor::block_on(retry).unwrap();
            assert!(new_attempt.is_current());
            assert!(state.is_connected.get_untracked());
            assert_eq!(
                state.protected_user.get_untracked().as_deref(),
                Some("new-user")
            );
        });
        owner.cleanup();
    }

    #[test]
    fn queued_disconnect_is_harmless_after_dashboard_disposal() {
        let owner = Owner::new();
        let state = owner.with(DashboardState::new);
        let pending = state.disconnect();
        owner.cleanup();
        futures::executor::block_on(pending).unwrap();
    }

    #[test]
    fn destroying_the_dashboard_invalidates_the_native_attempt() {
        let owner = Owner::new();
        let receipt = owner.with(|| {
            DashboardState::new()
                .native_auth
                .with_value(|auth| auth.begin().unwrap())
        });
        assert!(receipt.is_current());
        owner.cleanup();
        assert!(!receipt.is_current());
    }
}

#[cfg(test)]
mod tests {
    // `role_is_operator` was imported here until 2026-08-08 — by NOTHING, the
    // import line was its only mention. It went out with the client-side role
    // predicate on 2026-08-07 (see the `DashboardState` field comment above),
    // and because `cargo check` does not build `#[cfg(test)]`, this dangling
    // name took the panel's ENTIRE unit-test target down with it, silently, for
    // a day: every test in this crate stopped compiling, so none of them ran
    // and none of them could fail.
    use super::{
        approval_seed_applies,
        // `role_is_operator` was removed with the cached `role` field (see the
        // 2026-08-07 note above); nothing in this module used it and only the
        // test tree still named it, so `cargo check -p aleph-panel` stayed
        // green while `cargo test` could not build (判据清单 §10).
        classify_credential,
        gateway_readiness,
        query_with_bootstrap_ticket,
        replay_set,
        replay_subscriptions,
        send_tracked,
        strip_params,
        subscribe_diagnostic,
        subscribe_recorded,
        ws_url_for,
        GatewayReadiness,
        PendingRpcs,
        RpcFailure,
        SubmittedCredential,
        SubscribeError,
        BASE_TOPICS,
        GATEWAY_NOT_READY,
    };
    use leptos::prelude::{GetUntracked, Update};
    use std::collections::BTreeSet;

    #[test]
    fn telecom_poll_needs_initial_success_and_stops_on_role_refusal() {
        use super::{telecom_poll_continues, RpcFailure};
        let transport = Some(Err(RpcFailure {
            code: None,
            message: "temporary disconnect".into(),
        }));
        let denied = Some(Err(RpcFailure {
            code: Some(aleph_protocol::jsonrpc::AUTH_REQUIRED),
            message: "authenticated operator required".into(),
        }));
        assert!(telecom_poll_continues(false, &None)); // another fetch owns the gate
        assert!(telecom_poll_continues(false, &Some(Ok(()))));
        assert!(!telecom_poll_continues(false, &transport));
        assert!(telecom_poll_continues(true, &transport));
        assert!(!telecom_poll_continues(true, &denied));
    }

    /// The defect this floor exists to stop, stated as the predicate: a request
    /// issued while the socket is still connecting must **wait**, not fail.
    ///
    /// Before this, `rpc_call` answered "not connected" with an `Err` that was
    /// indistinguishable from a verdict about the call, and every mount-time
    /// load in the Panel inherited it. A cold load of `/settings/general` (URL
    /// or refresh — not SPA navigation) lost that race every time, showed
    /// "gateway unavailable", rendered its initial defaults as though they were
    /// the stored config, and never retried.
    #[test]
    fn a_request_issued_before_the_handshake_waits_instead_of_failing() {
        assert_eq!(
            gateway_readiness(false, false),
            GatewayReadiness::TooEarly,
            "still connecting is not a verdict — parking is the only honest answer"
        );
    }

    #[test]
    fn an_authorized_socket_is_never_delayed() {
        assert_eq!(gateway_readiness(true, false), GatewayReadiness::Ready);
        // needs_token is stale-but-set on the tick after a successful retry;
        // `is_connected` wins, so a warm Panel never parks.
        assert_eq!(gateway_readiness(true, true), GatewayReadiness::Ready);
    }

    /// Waiting is only right while something is still trying. Behind the login
    /// wall nothing is: only the user entering a credential changes the state,
    /// so parking every request for the full budget would turn `TokenWall` into
    /// a screen full of spinners.
    #[test]
    fn the_login_wall_fails_fast_instead_of_parking() {
        assert_eq!(gateway_readiness(false, true), GatewayReadiness::Walled);
    }

    /// The floor gates on authorization, not on "a channel exists".
    ///
    /// `connect()` installs `rpc_tx` *before* it runs the handshake, so those
    /// two states are not interchangeable: admitting on the channel alone would
    /// write requests to an unauthorized socket. This pins the choice, because
    /// `rpc_tx.is_some()` is the tempting reading of "connected" and it is
    /// wrong in exactly the window this code exists to cover.
    #[test]
    fn readiness_is_not_merely_that_the_channel_exists() {
        // The handshake window: channel installed, not yet authorized.
        assert_ne!(
            gateway_readiness(false, false),
            GatewayReadiness::Ready,
            "an unauthorized socket must not be treated as ready"
        );
    }

    /// The marker must not read as a verdict to any of the classifiers that
    /// sort failures for the UI, or "we never asked" becomes an answer again.
    #[test]
    fn the_not_ready_marker_is_not_a_permission_verdict() {
        assert!(!crate::components::admin_refusal::is_admin_refusal(
            GATEWAY_NOT_READY
        ));
        assert!(
            !GATEWAY_NOT_READY.is_empty(),
            "an empty marker would make every framed error indistinguishable"
        );
    }

    fn ledger(patterns: &[&str]) -> BTreeSet<super::LedgerEntry> {
        patterns
            .iter()
            .map(|p| ((*p).to_string(), Vec::new()))
            .collect()
    }

    fn replayed(replay: &[super::LedgerEntry], topic: &str) -> bool {
        replay.iter().any(|(t, _)| t == topic)
    }

    /// The reconnect replay re-sends a carve-out with its topic. Drives the
    /// replay step the reconnect handler runs (`replay_subscriptions` over
    /// `replay_set` of the ledger) and reads what it puts on the wire — not
    /// the ledger — because a carve-out that is filed but not re-sent is lost
    /// on the first reconnect and the phone gets every reasoning frame again.
    #[test]
    fn a_reconnect_resends_the_carve_out_with_its_topic() {
        let reasoning = aleph_protocol::STREAM_REASONING_TOPIC.to_string();
        let mut set = ledger(&["team.*"]);
        set.insert(("stream.*".to_string(), vec![reasoning.clone()]));
        let mut sent = Vec::new();
        let failed =
            futures::executor::block_on(replay_subscriptions(replay_set(&set), |params| {
                sent.push(params);
                std::future::ready(Ok(serde_json::Value::Null))
            }));
        assert!(failed.is_empty());
        let wire = |topic: &str, except: Vec<String>| {
            serde_json::to_value(aleph_protocol::TopicsRequest::new(
                vec![topic.to_string()],
                except,
            ))
            .unwrap()
        };
        assert!(
            sent.contains(&wire("stream.*", vec![reasoning])),
            "the phone's carve-out must be re-sent: {sent:?}"
        );
        assert!(sent.contains(&wire("team.*", vec![])), "{sent:?}");
    }

    // ---- Subscription ledger: records successes only, fenced by epoch ----

    fn entry(topic: &str) -> super::LedgerEntry {
        (topic.to_string(), Vec::new())
    }

    fn new_ledger() -> std::sync::Mutex<BTreeSet<super::LedgerEntry>> {
        std::sync::Mutex::new(BTreeSet::new())
    }

    fn ledger_topics(l: &std::sync::Mutex<BTreeSet<super::LedgerEntry>>) -> Vec<String> {
        l.lock().unwrap().iter().map(|(t, _)| t.clone()).collect()
    }

    fn ok_reply() -> std::future::Ready<Result<serde_json::Value, RpcFailure>> {
        std::future::ready(Ok(serde_json::Value::Null))
    }

    /// The defect: the ledger was written BEFORE the RPC, so a subscribe the
    /// server never accepted was replayed as if it had succeeded.
    #[test]
    fn a_successful_subscribe_is_filed_in_the_ledger() {
        let l = new_ledger();
        let r = futures::executor::block_on(subscribe_recorded(
            &l,
            entry("approval.**"),
            3,
            || Some(3),
            ok_reply(),
        ));
        assert_eq!(r, Ok(()));
        assert_eq!(ledger_topics(&l), vec!["approval.**".to_string()]);
    }

    #[test]
    fn a_failed_subscribe_leaves_no_ledger_entry() {
        let l = new_ledger();
        let r = futures::executor::block_on(subscribe_recorded(
            &l,
            entry("approval.**"),
            3,
            || Some(3),
            std::future::ready(Err(RpcFailure::local("Request timed out"))),
        ));
        assert!(matches!(r, Err(SubscribeError::Rpc(_))), "{r:?}");
        assert!(
            ledger_topics(&l).is_empty(),
            "a failure must not be replayed"
        );
    }

    /// A reply that lands after a reconnect was issued on the old socket; it
    /// says nothing about the new socket's (empty) server-side filter.
    #[test]
    fn a_stale_epoch_success_does_not_touch_the_ledger() {
        let l = new_ledger();
        let r = futures::executor::block_on(subscribe_recorded(
            &l,
            entry("config.**"),
            3,
            || Some(4),
            ok_reply(),
        ));
        assert_eq!(r, Err(SubscribeError::Superseded));
        assert!(ledger_topics(&l).is_empty());
    }

    /// Disconnected at reply time is the same verdict as a newer epoch.
    #[test]
    fn a_success_after_the_socket_dropped_does_not_touch_the_ledger() {
        let l = new_ledger();
        let r = futures::executor::block_on(subscribe_recorded(
            &l,
            entry("config.**"),
            3,
            || None,
            ok_reply(),
        ));
        assert_eq!(r, Err(SubscribeError::Superseded));
        assert!(ledger_topics(&l).is_empty());
    }

    #[test]
    fn a_duplicate_successful_subscribe_keeps_one_entry() {
        let l = new_ledger();
        for _ in 0..2 {
            futures::executor::block_on(subscribe_recorded(
                &l,
                entry("alerts.**"),
                1,
                || Some(1),
                ok_reply(),
            ))
            .unwrap();
        }
        assert_eq!(ledger_topics(&l).len(), 1);
    }

    /// The carve-out is part of the key: the phone's `stream.*` minus
    /// reasoning and a plain `stream.*` are two successful subscriptions.
    #[test]
    fn the_carve_out_is_part_of_the_recorded_key() {
        let l = new_ledger();
        let phone = ("stream.*".to_string(), vec!["stream.reasoning".to_string()]);
        for e in [entry("stream.*"), phone] {
            futures::executor::block_on(subscribe_recorded(&l, e, 1, || Some(1), ok_reply()))
                .unwrap();
        }
        assert_eq!(l.lock().unwrap().len(), 2);
    }

    /// Reconnect replay re-offers the successes (plus the always-seeded base
    /// topics) and nothing that never succeeded.
    #[test]
    fn replay_only_replays_successful_entries() {
        let l = new_ledger();
        futures::executor::block_on(subscribe_recorded(
            &l,
            entry("browser.live.resource"),
            1,
            || Some(1),
            ok_reply(),
        ))
        .unwrap();
        let _ = futures::executor::block_on(subscribe_recorded(
            &l,
            entry("browser.live.closed"),
            1,
            || Some(1),
            std::future::ready(Err(RpcFailure::local("boom"))),
        ));
        let _ = futures::executor::block_on(subscribe_recorded(
            &l,
            entry("session.artifact"),
            1,
            || Some(2),
            ok_reply(),
        ));
        let snapshot = l.lock().unwrap().clone();
        let replay = replay_set(&snapshot);
        assert!(replayed(&replay, "browser.live.resource"));
        assert!(!replayed(&replay, "browser.live.closed"), "{replay:?}");
        assert!(!replayed(&replay, "session.artifact"), "{replay:?}");
        for base in BASE_TOPICS {
            assert!(replayed(&replay, base), "{base} is always seeded");
        }
    }

    /// The console line carries the topic and an error KIND only: never the
    /// server's message (which may echo params, URLs or credentials).
    #[test]
    fn the_diagnostic_names_topic_and_kind_but_not_the_message() {
        let secret = "ticket=abc123 url=ws://h/?token=xyz";
        let server = Err(SubscribeError::Rpc(RpcFailure {
            code: Some(-32000),
            message: secret.to_string(),
        }));
        let local = Err(SubscribeError::Rpc(RpcFailure::local(secret)));
        let stale = Err(SubscribeError::Superseded);
        let ok = Ok(());
        let lines = [
            subscribe_diagnostic("approval.**", &ok),
            subscribe_diagnostic("approval.**", &server),
            subscribe_diagnostic("approval.**", &local),
            subscribe_diagnostic("approval.**", &stale),
        ];
        for line in &lines {
            assert!(line.contains("approval.**"), "{line}");
            for leak in ["abc123", "token", "ws://", "ticket", "-32000"] {
                assert!(!line.contains(leak), "{line} leaks {leak}");
            }
        }
        assert!(lines[0].contains("ok"));
        assert!(lines[1].contains("server"));
        assert!(lines[2].contains("local"));
        assert!(lines[3].contains("superseded"));
    }

    /// A request is filed as pending BEFORE it is sent, so no reply can arrive
    /// for an id nobody is waiting for; the send closure observes the map.
    #[test]
    fn a_request_is_registered_before_it_is_sent() {
        let mut pending = PendingRpcs::new();
        let (tx, mut rx) = futures::channel::oneshot::channel();
        let mut registered_at_send = false;
        futures::executor::block_on(send_tracked(
            &mut pending,
            "7".to_string(),
            tx,
            |p| {
                registered_at_send = p.contains_key("7");
                std::future::ready(Ok::<(), String>(()))
            },
            RpcFailure::local,
        ));
        assert!(
            registered_at_send,
            "must be registered when the send starts"
        );
        assert!(pending.contains_key("7"), "stays pending until the reply");
        assert!(rx.try_recv().unwrap().is_none(), "nothing answered yet");
    }

    /// A failed send removes the entry and answers the caller once, with the
    /// failure `on_error` minted (no code: it is local, not a server verdict).
    #[test]
    fn a_failed_send_unregisters_and_answers_the_caller() {
        let mut pending = PendingRpcs::new();
        let (tx, mut rx) = futures::channel::oneshot::channel();
        futures::executor::block_on(send_tracked(
            &mut pending,
            "8".to_string(),
            tx,
            |_| std::future::ready(Err::<(), &str>("closed")),
            |e| RpcFailure::local(format!("could not send: {e}")),
        ));
        assert!(pending.is_empty(), "a failed send must not leak an entry");
        assert_eq!(
            rx.try_recv().unwrap().unwrap().unwrap_err(),
            RpcFailure::local("could not send: closed")
        );
    }

    /// The approval seed is discarded when the socket was replaced or an event
    /// already delivered a newer list; otherwise it applies.
    #[test]
    fn a_stale_approval_seed_is_discarded() {
        assert!(approval_seed_applies(3, 3, 0, 0));
        assert!(!approval_seed_applies(3, 4, 0, 0), "new connection");
        assert!(!approval_seed_applies(3, 3, 0, 1), "newer event refetch");
    }

    /// The reconnect replay is also a *narrowing*: an empty gateway-side filter
    /// receives every frame, so whatever we replay becomes the whole filter.
    /// A component-owned topic missing from the replay is therefore killed, not
    /// merely un-restored — this is what silently stopped `stream.*` after the
    /// first reconnect while the connection indicator stayed green.
    #[test]
    fn a_reconnect_replays_component_topics_not_just_the_base_set() {
        let replay = replay_set(&ledger(&[
            "stream.*",
            "team.*",
            "team.*.task.*",
            "config.**",
        ]));
        for topic in ["stream.*", "team.*", "team.*.task.*"] {
            assert!(
                replayed(&replay, topic),
                "{topic} must survive a reconnect; it has no component to re-subscribe it \
                 (ChatView is never unmounted)"
            );
        }
    }

    /// The three app-level topics have no owning component, so they must be
    /// seeded even on the very first connect when the ledger is still empty.
    #[test]
    fn the_base_set_is_replayed_even_with_an_empty_ledger() {
        let replay = replay_set(&BTreeSet::new());
        for base in BASE_TOPICS {
            assert!(replayed(&replay, base), "{base} must be seeded");
        }
    }

    /// Unsubscribing removes the pattern from the ledger, so an unmounted
    /// component's topic must not be resurrected by the next reconnect.
    #[test]
    fn an_unsubscribed_topic_is_not_replayed() {
        let mut set = ledger(&["stream.*", "voice.transcribe.delta"]);
        set.remove(&("voice.transcribe.delta".to_string(), Vec::new()));
        let replay = replay_set(&set);
        assert!(replayed(&replay, "stream.*"));
        assert!(
            !replayed(&replay, "voice.transcribe.delta"),
            "an unsubscribed topic must stay unsubscribed across a reconnect"
        );
    }

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

    /// The message loop's one error-parsing decision: both halves of a real
    /// JSON-RPC error object survive into [`RpcFailure`]. The projection in
    /// `rpc_call` (`message` alone) is one `map_err` away from this value, so
    /// this test also pins the bytes every legacy `String` consumer receives.
    #[test]
    fn a_wire_error_keeps_both_its_code_and_its_message() {
        let failure = super::parse_rpc_error(&serde_json::json!({
            "code": -32031,
            "message": "Failed to apply canvas ops: revision conflict: canvas is at revision 7",
        }));
        assert_eq!(failure.code, Some(-32031));
        assert_eq!(
            failure.message,
            "Failed to apply canvas ops: revision conflict: canvas is at revision 7"
        );
    }

    /// A code the wire never carried — and one that cannot fit `i32` (the
    /// wire is untrusted input) — both degrade to `None` rather than to some
    /// invented number a classifier might branch on.
    #[test]
    fn a_missing_or_oversized_code_degrades_to_none() {
        let missing = super::parse_rpc_error(&serde_json::json!({ "message": "boom" }));
        assert_eq!(missing.code, None);
        assert_eq!(missing.message, "boom");

        let oversized = super::parse_rpc_error(&serde_json::json!({
            "code": i64::from(i32::MAX) + 1,
            "message": "boom",
        }));
        assert_eq!(oversized.code, None);
    }

    /// Locally-minted failures (socket not up, timeout, closed channel) can
    /// never impersonate a server verdict: `RpcFailure::local` has no code by
    /// construction, so a code-branching caller treats them as "not asked".
    #[test]
    fn a_local_failure_never_carries_a_code() {
        let failure = super::RpcFailure::local("Request timed out");
        assert_eq!(failure.code, None);
        assert_eq!(failure.message, "Request timed out");
    }

    /// `with_owner` analogue to the one in `state::sessions::tests`: signals
    /// need a current Owner to allocate under, even when they are only read
    /// with `get_untracked`. The state tests already prove this; doing it
    /// again here would just be a copy — `DashboardState::new` does its own
    /// allocation, so a plain local is sufficient.
    fn with_owner<T>(f: impl FnOnce() -> T) -> T {
        let owner = leptos::prelude::Owner::new();
        owner.set();
        f()
    }

    /// `connection_epoch` is the single dependency every "reconnect-handler"
    /// Effect tracks: it ticks once per successful handshake, and any handler
    /// that reads it must see a STRICTLY INCREASING sequence, because Effects
    /// distinguish "re-run" from "still the same epoch" by value comparison.
    ///
    /// The T2.7 guard pins the contract that the only writer
    /// (`connection_epoch.update(|n| *n += 1)` in the handshake path) is the
    /// only writer — a refactor that turned it into a snapshot of an external
    /// counter, or that accidentally allowed a decrement on disconnect, would
    /// silently break every reconnect-driven Effect across the panel
    /// (`app.rs`, `chat_sidebar.rs`, `chat/state/mod.rs::pending_reattach`).
    #[test]
    fn connection_epoch_is_monotone_across_reconnect() {
        with_owner(|| {
            let dash = super::DashboardState::new();
            let e0 = dash.connection_epoch.get_untracked();
            dash.connection_epoch.update(|n| *n += 1);
            let e1 = dash.connection_epoch.get_untracked();
            dash.connection_epoch.update(|n| *n += 1);
            let e2 = dash.connection_epoch.get_untracked();
            dash.connection_epoch.update(|n| *n += 1);
            let e3 = dash.connection_epoch.get_untracked();
            assert!(
                e1 > e0 && e2 > e1 && e3 > e2,
                "every reconnect must strictly increase the epoch (got {e0} → {e1} → {e2} → {e3})"
            );
        });
    }
}
