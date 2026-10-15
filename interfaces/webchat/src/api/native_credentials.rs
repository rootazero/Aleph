//! Panel side of the native protected-credential Bridge (Task 41).
//!
//! Inside the desktop shell the Panel keeps its long-lived device credential in
//! the OS keyring through exactly three fixed shell commands
//! (`aleph_protocol::native_credentials::{COMMAND_LOAD, COMMAND_SAVE,
//! COMMAND_DELETE}`). This module owns three things and nothing else:
//!
//! * the IPC seam ([`BridgeIpc`]) and its strict decoding of the shared DTOs;
//! * the attempt fence ([`AttemptClock`] / [`AttemptReceipt`]) that lets a late
//!   response be recognised as stale *before* it can write an OS credential or
//!   be accepted as the session identity;
//! * the native handshake decision ([`native_handshake`]), a pure async function
//!   over the IPC seam and the Gateway `connect` call so it can be tested with
//!   fakes.
//!
//! Hard rules, each pinned by a test below:
//!
//! * **No fallback.** When the Panel expects a native shell, a missing, denied
//!   or malformed Bridge is a fixed failure. It never degrades to
//!   `localStorage`, and nothing here reads or writes `localStorage` at all.
//! * **Protected means verified by the Gateway and persisted.** A session is
//!   `Protected` only when the Gateway returned `authorized`, a non-blank
//!   `device_id` equal to the one we presented and a non-blank `user_id`; and,
//!   for a fresh pairing, only after the native save ACKed. A weak echo, a
//!   rebound device id or a failed save is never protected.
//! * **The bootstrap ticket is memory-only and single use.** It is taken from
//!   [`NativeAuthState`] at the moment the `connect` is sent, so a later attempt
//!   can never replay it, and the save payload carries only the server-issued
//!   long-lived token.
//! * **Fixed errors.** Failures map to fixed strings; backend text, arguments
//!   and secrets are never forwarded.
//!
//! The Gateway stays the only authority on who the User is; this module never
//! stores a role and never derives identity from a URL.

use aleph_protocol::native_credentials::{
    DeviceConnectCredential, PanelCredentialLease, COMMAND_LOAD, COMMAND_SAVE,
};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Why a Bridge call failed. Carries no backend text on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeError {
    /// There is no Tauri IPC to call (plain browser, or the shell is missing).
    Unavailable,
    /// The shell refused or failed the command (ACL, lease, keyring, ...).
    Rejected,
    /// The reply did not decode as the shared DTO / did not ACK.
    Malformed,
}

/// The IPC seam: invoke one fixed command, get its JSON reply.
///
/// `args` is `Value::Null` for the no-argument `load`.
pub trait BridgeIpc {
    fn invoke(
        &self,
        command: &'static str,
        args: Value,
    ) -> impl Future<Output = Result<Value, BridgeError>>;
}

/// Begin a storage lease and read the stored credential.
pub async fn load_lease<I: BridgeIpc>(ipc: &I) -> Result<PanelCredentialLease, BridgeError> {
    let reply = ipc.invoke(COMMAND_LOAD, Value::Null).await?;
    // Never surface the decoder's message: it may echo reply content.
    serde_json::from_value::<PanelCredentialLease>(reply).map_err(|_| BridgeError::Malformed)
}

/// Persist `credential` under `lease_id`. Success requires the exact `null` ACK
/// the shell returns for `()`.
pub async fn save_credential<I: BridgeIpc>(
    ipc: &I,
    lease_id: u64,
    credential: &DeviceConnectCredential,
) -> Result<(), BridgeError> {
    let reply = ipc
        .invoke(
            COMMAND_SAVE,
            json!({ "lease_id": lease_id, "credential": credential }),
        )
        .await?;
    if reply.is_null() {
        Ok(())
    } else {
        Err(BridgeError::Malformed)
    }
}

// ---------------------------------------------------------------------------
// Native detection
// ---------------------------------------------------------------------------

/// Pure truth table: the Panel expects a native Bridge if the shell's
/// main-frame marker is set, the shell's `data-shell` attribute is present, or
/// Tauri's IPC internals exist. Any one is enough: a *missing* marker must not
/// downgrade a native shell to "pure browser".
#[must_use]
pub const fn native_expected_from(
    marker: bool,
    shell_attribute: bool,
    tauri_internals: bool,
) -> bool {
    marker || shell_attribute || tauri_internals
}

/// Whether this Panel runs inside the native shell and must use the Bridge.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn native_expected() -> bool {
    use js_sys::Reflect;
    use wasm_bindgen::JsValue;

    let Some(window) = web_sys::window() else {
        return false;
    };
    let has = |key: &str| {
        Reflect::get(&window, &JsValue::from_str(key))
            .map(|v| !v.is_undefined() && !v.is_null())
            .unwrap_or(false)
    };
    let marker = Reflect::get(&window, &JsValue::from_str("__ALEPH_PANEL_KEYRING__"))
        .map(|v| v.as_bool() == Some(true))
        .unwrap_or(false);
    let shell_attribute = window
        .document()
        .and_then(|d| d.document_element())
        .and_then(|e| e.get_attribute("data-shell"))
        .as_deref()
        == Some("aleph-tauri");
    native_expected_from(marker, shell_attribute, has("__TAURI_INTERNALS__"))
}

#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn native_expected() -> bool {
    false
}

/// The real IPC: `window.__TAURI_INTERNALS__.invoke`, with the three fixed
/// commands only (the callers above are the only users). No `eval`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TauriIpc;

#[cfg(target_arch = "wasm32")]
impl BridgeIpc for TauriIpc {
    fn invoke(
        &self,
        command: &'static str,
        args: Value,
    ) -> impl Future<Output = Result<Value, BridgeError>> {
        wasm_invoke(command, args)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl BridgeIpc for TauriIpc {
    fn invoke(
        &self,
        _command: &'static str,
        _args: Value,
    ) -> impl Future<Output = Result<Value, BridgeError>> {
        std::future::ready(Err(BridgeError::Unavailable))
    }
}

#[cfg(target_arch = "wasm32")]
async fn wasm_invoke(command: &'static str, args: Value) -> Result<Value, BridgeError> {
    use js_sys::{Function, Promise, Reflect, JSON};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window().ok_or(BridgeError::Unavailable)?;
    let internals = Reflect::get(&window, &JsValue::from_str("__TAURI_INTERNALS__"))
        .map_err(|_| BridgeError::Unavailable)?;
    if internals.is_undefined() || internals.is_null() {
        return Err(BridgeError::Unavailable);
    }
    let invoke: Function = Reflect::get(&internals, &JsValue::from_str("invoke"))
        .map_err(|_| BridgeError::Unavailable)?
        .dyn_into()
        .map_err(|_| BridgeError::Unavailable)?;
    let js_args = if args.is_null() {
        JsValue::UNDEFINED
    } else {
        JSON::parse(&args.to_string()).map_err(|_| BridgeError::Malformed)?
    };
    let ret = invoke
        .call2(&internals, &JsValue::from_str(command), &js_args)
        .map_err(|_| BridgeError::Unavailable)?;
    let promise: Promise = ret.dyn_into().map_err(|_| BridgeError::Unavailable)?;
    // The rejection payload is deliberately dropped: it may name the account
    // or the keyring backend.
    let value = JsFuture::from(promise)
        .await
        .map_err(|_| BridgeError::Rejected)?;
    if value.is_undefined() {
        return Ok(Value::Null);
    }
    let text = JSON::stringify(&value)
        .map_err(|_| BridgeError::Malformed)?
        .as_string()
        .ok_or(BridgeError::Malformed)?;
    serde_json::from_str(&text).map_err(|_| BridgeError::Malformed)
}

// ---------------------------------------------------------------------------
// Attempt fence
// ---------------------------------------------------------------------------

/// Monotonic generation counter. Every new connect attempt, every disconnect
/// and every unexpected drop moves it, which makes every older
/// [`AttemptReceipt`] stale.
#[derive(Debug, Default)]
pub struct AttemptClock(AtomicU64);

/// Proof that an attempt was the newest one when it was issued.
#[derive(Clone, Debug)]
pub struct AttemptReceipt {
    clock: Arc<AttemptClock>,
    id: u64,
}

impl AttemptClock {
    /// Start a new attempt, invalidating all earlier receipts. `None` when the
    /// counter is exhausted (fail closed; unreachable in practice).
    #[must_use]
    pub fn begin(clock: &Arc<Self>) -> Option<AttemptReceipt> {
        let next = clock
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| {
                (c < u64::MAX - 1).then(|| c + 1)
            })
            .ok()?
            + 1;
        Some(AttemptReceipt {
            clock: Arc::clone(clock),
            id: next,
        })
    }

    /// Make every outstanding receipt stale without issuing a new one.
    pub fn invalidate(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| {
                Some(c.saturating_add(1))
            });
    }
}

impl AttemptReceipt {
    /// Still the newest attempt?
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.id < u64::MAX && self.clock.0.load(Ordering::SeqCst) == self.id
    }
}

/// `None` (no receipt) is never current.
#[must_use]
pub fn attempt_current(receipt: &Option<AttemptReceipt>) -> bool {
    receipt.as_ref().is_some_and(AttemptReceipt::is_current)
}

// ---------------------------------------------------------------------------
// Private native auth state
// ---------------------------------------------------------------------------

/// Everything the native handshake keeps in WASM memory. Deliberately has no
/// `Debug`: it can hold a one-shot ticket.
#[derive(Default)]
pub struct NativeAuthState {
    clock: Arc<AttemptClock>,
    ticket: Mutex<Option<String>>,
    ephemeral_id: Mutex<Option<String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl NativeAuthState {
    /// Start a new attempt (see [`AttemptClock::begin`]).
    #[must_use]
    pub fn begin(&self) -> Option<AttemptReceipt> {
        AttemptClock::begin(&self.clock)
    }

    /// Invalidate every outstanding attempt.
    pub fn invalidate(&self) {
        self.clock.invalidate();
    }

    /// Hold a bootstrap ticket in memory for the next `connect`. Replaces any
    /// ticket already held.
    pub fn stash_ticket(&self, ticket: String) {
        *lock(&self.ticket) = Some(ticket);
    }

    /// Consume the ticket. Single use: a second call returns `None`.
    #[must_use]
    pub fn take_ticket(&self) -> Option<String> {
        lock(&self.ticket).take()
    }

    /// Memory-only device id for the credentialless local-compat connect, so
    /// reconnects do not look like a fresh device each time. Never persisted.
    fn ephemeral_id(&self, make: &dyn Fn() -> String) -> String {
        lock(&self.ephemeral_id).get_or_insert_with(make).clone()
    }
}

/// Pull a non-empty `bt=` value out of a `?a=1&bt=...` query string.
#[must_use]
pub fn ticket_from_search(search: &str) -> Option<String> {
    let q = search.strip_prefix('?').unwrap_or(search);
    q.split('&')
        .find_map(|pair| pair.strip_prefix("bt="))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// A fresh device id for a new pairing (never read from `localStorage`).
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn new_device_id() -> String {
    use js_sys::{Function, Reflect};
    use wasm_bindgen::{JsCast, JsValue};

    let uuid = web_sys::window()
        .and_then(|w| Reflect::get(&w, &JsValue::from_str("crypto")).ok())
        .and_then(|crypto| {
            let f: Function = Reflect::get(&crypto, &JsValue::from_str("randomUUID"))
                .ok()?
                .dyn_into()
                .ok()?;
            f.call0(&crypto).ok()?.as_string()
        });
    match uuid {
        Some(u) if !u.is_empty() => format!("panel-{u}"),
        // The id is not a secret (the token is); this only needs to be unique.
        _ => format!(
            "panel-{:x}-{:x}-{:x}",
            js_sys::Date::now() as u64,
            (js_sys::Math::random() * 1.0e9) as u64,
            (js_sys::Math::random() * 1.0e9) as u64
        ),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn new_device_id() -> String {
    "panel-host".to_string()
}

// ---------------------------------------------------------------------------
// Endpoint check
// ---------------------------------------------------------------------------

/// `wss://…`, or `ws://` to a loopback host. Rejects userinfo so
/// `ws://localhost:80@evil/` cannot pass as loopback.
#[must_use]
pub fn endpoint_allowed(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("wss://") {
        let authority = rest.split('/').next().unwrap_or("");
        return !authority.is_empty() && !authority.contains('@');
    }
    let Some(rest) = url.strip_prefix("ws://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Strict identity of two WebSocket endpoints: same scheme, host, effective
/// port, path, query and fragment, and neither carries userinfo. A `wss://`
/// URL may spell its default port explicitly; nothing else is aliased
/// (no localhost <-> 127.0.0.1, no `ws://` default-port folding).
#[must_use]
pub fn endpoint_matches(lease_url: &str, dialed_url: &str) -> bool {
    let (Ok(a), Ok(b)) = (url::Url::parse(lease_url), url::Url::parse(dialed_url)) else {
        return false;
    };
    let no_userinfo = |u: &url::Url| u.username().is_empty() && u.password().is_none();
    if !no_userinfo(&a) || !no_userinfo(&b) || a.scheme() != b.scheme() {
        return false;
    }
    let (Some(host_a), Some(host_b)) = (a.host_str(), b.host_str()) else {
        return false;
    };
    let ports_equal = if a.scheme() == "wss" {
        a.port_or_known_default() == b.port_or_known_default()
    } else {
        a.port() == b.port()
    };
    host_a == host_b
        && ports_equal
        && a.path() == b.path()
        && a.query() == b.query()
        && a.fragment() == b.fragment()
}

// ---------------------------------------------------------------------------
// The native handshake decision
// ---------------------------------------------------------------------------

/// Why a native handshake could not produce a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeFailure {
    /// Load failed / malformed.
    Bridge(BridgeError),
    /// The shell's Gateway is not the endpoint being dialed (or is insecure).
    Endpoint,
    /// The `connect` call itself failed. The transport's message is dropped:
    /// it can echo the dialed URL.
    Connect,
    /// The Gateway authorized, but not with an identity we can trust.
    Identity,
    /// The native save failed; the new token was NOT kept.
    Save(BridgeError),
}

impl NativeFailure {
    /// Fixed, secret-free user-facing text for the connection failure.
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            Self::Bridge(BridgeError::Unavailable) => {
                "Secure credential storage is unavailable in this app"
            }
            Self::Bridge(_) => "Secure credential storage could not be read",
            Self::Endpoint => "Secure credential storage does not match this Gateway",
            Self::Connect => "Gateway connection failed",
            Self::Identity => "The Gateway did not confirm a trusted identity",
            Self::Save(_) => "The pairing could not be saved to secure credential storage",
        }
    }
}

/// Result of [`native_handshake`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeOutcome {
    /// Gateway-verified User, credential persisted (or already stored).
    Protected {
        user_id: String,
    },
    /// Authorized without any stored/offered credential (loopback local
    /// compat). Works, but is NOT protected.
    Credentialless,
    /// Unauthorized: show the login wall.
    NeedsToken {
        was_rejected: bool,
    },
    /// A newer attempt/disconnect/endpoint change owns the state now; touch
    /// nothing.
    Superseded,
    Failed(NativeFailure),
}

/// Redacted failure of the `connect` transport (no payload by design).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectSendError;

/// Inputs of one native handshake.
pub struct HandshakeRequest<'a> {
    /// The WebSocket endpoint actually being dialed.
    pub dialed_url: &'a str,
    pub device_name: &'a str,
    pub auth: &'a NativeAuthState,
    /// Mint a fresh device id (called only for a bootstrap pairing and for the
    /// first credentialless connect).
    pub new_device_id: &'a dyn Fn() -> String,
}

fn nonblank(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

enum Kind {
    Bootstrap { ticket: String, device_id: String },
    Stored { credential: DeviceConnectCredential },
    None,
}

/// Run the native handshake. `send_connect` performs the Gateway's existing
/// `connect` RPC with the params built here; `is_current` is the attempt +
/// endpoint fence (it is consulted before the credential is sent, after the
/// reply, before the OS save and again after the save ACK).
pub async fn native_handshake<I, S, Fut>(
    ipc: &I,
    req: HandshakeRequest<'_>,
    is_current: &dyn Fn() -> bool,
    send_connect: S,
) -> NativeOutcome
where
    I: BridgeIpc,
    S: FnOnce(Value) -> Fut,
    Fut: Future<Output = Result<Value, ConnectSendError>>,
{
    if !is_current() {
        return NativeOutcome::Superseded;
    }
    let lease = match load_lease(ipc).await {
        Ok(lease) => lease,
        Err(e) => return NativeOutcome::Failed(NativeFailure::Bridge(e)),
    };
    // Exact equality with what we dial, over a transport that may carry the
    // token. Nothing about the URL picks an identity.
    if !endpoint_matches(&lease.gateway_url, req.dialed_url)
        || !endpoint_allowed(&lease.gateway_url)
    {
        return NativeOutcome::Failed(NativeFailure::Endpoint);
    }
    if !is_current() {
        return NativeOutcome::Superseded;
    }

    // Bootstrap wins; the ticket leaves memory here, success or not.
    let kind = if let Some(ticket) = req.auth.take_ticket() {
        Kind::Bootstrap {
            ticket,
            device_id: (req.new_device_id)(),
        }
    } else if let Some(credential) = lease.credential.clone() {
        Kind::Stored { credential }
    } else {
        Kind::None
    };

    let mut params = json!({ "device_name": req.device_name });
    match &kind {
        Kind::Bootstrap { ticket, device_id } => {
            params["device_id"] = json!(device_id);
            params["bootstrap_ticket"] = json!(ticket);
        }
        Kind::Stored { credential } => {
            params["device_id"] = json!(credential.device_id);
            params["device_token"] = json!(credential.device_token);
        }
        Kind::None => {
            params["device_id"] = json!(req.auth.ephemeral_id(req.new_device_id));
        }
    }
    let presented = !matches!(kind, Kind::None);

    let resp = match send_connect(params).await {
        Ok(resp) => resp,
        Err(ConnectSendError) => return NativeOutcome::Failed(NativeFailure::Connect),
    };
    if !is_current() {
        return NativeOutcome::Superseded;
    }

    if resp.get("authorized").and_then(Value::as_bool) != Some(true) {
        return NativeOutcome::NeedsToken {
            was_rejected: presented,
        };
    }

    match kind {
        Kind::None => NativeOutcome::Credentialless,
        Kind::Stored { credential } => {
            let echoed = nonblank(&resp, "device_id");
            let user = nonblank(&resp, "user_id");
            match (echoed, user) {
                (Some(id), Some(user_id)) if id == credential.device_id => {
                    NativeOutcome::Protected { user_id }
                }
                _ => NativeOutcome::Failed(NativeFailure::Identity),
            }
        }
        Kind::Bootstrap { ticket, device_id } => {
            let echoed = nonblank(&resp, "device_id");
            let user = nonblank(&resp, "user_id");
            let token = nonblank(&resp, "device_token");
            let (Some(echoed), Some(user_id), Some(token)) = (echoed, user, token) else {
                return NativeOutcome::Failed(NativeFailure::Identity);
            };
            // The device id must be the one we just minted, and the "new token"
            // must not be the one-shot ticket echoed back.
            if echoed != device_id || token == ticket {
                return NativeOutcome::Failed(NativeFailure::Identity);
            }
            let Ok(credential) = DeviceConnectCredential::new(device_id, token) else {
                return NativeOutcome::Failed(NativeFailure::Identity);
            };
            // Same receipt before the OS write and before accepting.
            if !is_current() {
                return NativeOutcome::Superseded;
            }
            if let Err(e) = save_credential(ipc, lease.lease_id, &credential).await {
                return NativeOutcome::Failed(NativeFailure::Save(e));
            }
            if !is_current() {
                return NativeOutcome::Superseded;
            }
            NativeOutcome::Protected { user_id }
        }
    }
}

/// A caller counts as protected only with a non-blank Gateway-derived User.
#[must_use]
pub fn is_protected_user(user: Option<&str>) -> bool {
    user.is_some_and(|u| !u.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::cell::{Cell, RefCell};

    const URL: &str = "wss://gw.example/ws";
    const SECRET: &str = "aleph-dt-SENTINEL-secret";
    const TICKET: &str = "aleph-bt-SENTINEL-ticket";

    struct FakeIpc {
        load: Result<Value, BridgeError>,
        save: Result<Value, BridgeError>,
        calls: RefCell<Vec<(&'static str, Value)>>,
        on_save: Box<dyn Fn()>,
    }

    impl FakeIpc {
        fn new(credential: Option<(&str, &str)>) -> Self {
            let cred = credential
                .map(|(i, t)| json!({"device_id": i, "device_token": t}))
                .unwrap_or(Value::Null);
            Self {
                load: Ok(json!({"gateway_url": URL, "lease_id": 7, "credential": cred})),
                save: Ok(Value::Null),
                calls: RefCell::new(Vec::new()),
                on_save: Box::new(|| {}),
            }
        }
        fn saves(&self) -> Vec<Value> {
            self.calls
                .borrow()
                .iter()
                .filter(|(c, _)| *c == COMMAND_SAVE)
                .map(|(_, a)| a.clone())
                .collect()
        }
    }

    impl BridgeIpc for FakeIpc {
        fn invoke(
            &self,
            command: &'static str,
            args: Value,
        ) -> impl Future<Output = Result<Value, BridgeError>> {
            self.calls.borrow_mut().push((command, args));
            let reply = if command == COMMAND_LOAD {
                self.load.clone()
            } else {
                (self.on_save)();
                self.save.clone()
            };
            std::future::ready(reply)
        }
    }

    fn run(
        ipc: &FakeIpc,
        auth: &NativeAuthState,
        url: &str,
        is_current: &dyn Fn() -> bool,
        reply: Result<Value, ConnectSendError>,
        sent: &RefCell<Vec<Value>>,
    ) -> NativeOutcome {
        let mint = || "panel-NEW".to_string();
        block_on(native_handshake(
            ipc,
            HandshakeRequest {
                dialed_url: url,
                device_name: "Panel",
                auth,
                new_device_id: &mint,
            },
            is_current,
            |params| {
                sent.borrow_mut().push(params);
                std::future::ready(reply)
            },
        ))
    }

    fn always() -> bool {
        true
    }

    fn ok_resp(device_id: &str, user: Option<&str>, token: Option<&str>) -> Value {
        let mut v = json!({"authorized": true, "needs_token": false, "device_id": device_id});
        if let Some(u) = user {
            v["user_id"] = json!(u);
        }
        if let Some(t) = token {
            v["device_token"] = json!(t);
        }
        v
    }

    #[test]
    fn load_decoding_is_strict_and_error_text_is_fixed() {
        for bad in [
            json!({"gateway_url": URL, "lease_id": 7}),
            json!({"gateway_url": URL, "lease_id": 0, "credential": null}),
            json!({"gateway_url": URL, "lease_id": 7, "credential": null, "account": "x"}),
            json!({"gateway_url": URL, "lease_id": 7,
                   "credential": {"device_id": "d", "device_token": SECRET, "role": "admin"}}),
        ] {
            let mut ipc = FakeIpc::new(None);
            ipc.load = Ok(bad);
            assert_eq!(
                block_on(load_lease(&ipc)).unwrap_err(),
                BridgeError::Malformed
            );
        }
    }

    #[test]
    fn save_requires_a_null_ack_and_carries_only_lease_and_credential() {
        let cred = DeviceConnectCredential::new("d", SECRET).unwrap();
        let mut ipc = FakeIpc::new(None);
        block_on(save_credential(&ipc, 9, &cred)).unwrap();
        let args = ipc.saves().remove(0);
        assert_eq!(
            args,
            json!({"lease_id": 9, "credential": {"device_id": "d", "device_token": SECRET}})
        );
        ipc.save = Ok(json!(true));
        assert_eq!(
            block_on(save_credential(&ipc, 9, &cred)).unwrap_err(),
            BridgeError::Malformed
        );
    }

    #[test]
    fn no_bridge_is_a_fixed_failure_and_no_connect_is_sent() {
        let mut ipc = FakeIpc::new(None);
        ipc.load = Err(BridgeError::Unavailable);
        let sent = RefCell::new(Vec::new());
        let auth = NativeAuthState::default();
        auth.stash_ticket(TICKET.into());
        let out = run(&ipc, &auth, URL, &always, Ok(json!({})), &sent);
        assert_eq!(
            out,
            NativeOutcome::Failed(NativeFailure::Bridge(BridgeError::Unavailable))
        );
        assert!(sent.borrow().is_empty());
        // The ticket was not consumed by a failed load, and nothing was saved.
        assert!(ipc.saves().is_empty());
        assert_eq!(auth.take_ticket().as_deref(), Some(TICKET));
    }

    #[test]
    fn malformed_or_foreign_lease_never_reaches_connect() {
        let sent = RefCell::new(Vec::new());
        let auth = NativeAuthState::default();
        let mut ipc = FakeIpc::new(None);
        ipc.load = Ok(json!({"nonsense": true}));
        assert!(matches!(
            run(&ipc, &auth, URL, &always, Ok(json!({})), &sent),
            NativeOutcome::Failed(NativeFailure::Bridge(BridgeError::Malformed))
        ));
        // Endpoint differs from the dialed one.
        let ipc = FakeIpc::new(None);
        assert_eq!(
            run(
                &ipc,
                &auth,
                "wss://other.example/ws",
                &always,
                Ok(json!({})),
                &sent
            ),
            NativeOutcome::Failed(NativeFailure::Endpoint)
        );
        // Insecure but equal endpoint (remote ws://) is refused too.
        let mut ipc = FakeIpc::new(None);
        ipc.load =
            Ok(json!({"gateway_url": "ws://gw.example/ws", "lease_id": 1, "credential": null}));
        assert_eq!(
            run(
                &ipc,
                &auth,
                "ws://gw.example/ws",
                &always,
                Ok(json!({})),
                &sent
            ),
            NativeOutcome::Failed(NativeFailure::Endpoint)
        );
        assert!(sent.borrow().is_empty());
    }

    #[test]
    fn endpoint_rule_is_wss_or_loopback_ws_without_userinfo() {
        assert!(endpoint_allowed("wss://gw.example/ws"));
        assert!(endpoint_allowed("ws://127.0.0.1:18790/ws"));
        assert!(endpoint_allowed("ws://localhost:1/ws"));
        assert!(endpoint_allowed("ws://[::1]:18790/ws"));
        assert!(!endpoint_allowed("ws://gw.example/ws"));
        assert!(!endpoint_allowed("ws://localhost:80@evil.example/ws"));
        assert!(!endpoint_allowed("wss://user@gw.example/ws"));
        assert!(!endpoint_allowed("http://127.0.0.1/ws"));
        assert!(!endpoint_allowed(""));
    }

    #[test]
    fn endpoint_matches_is_strict_and_never_aliases() {
        assert!(endpoint_matches(
            "wss://gw.example/ws",
            "wss://gw.example/ws"
        ));
        assert!(endpoint_matches(
            "wss://GW.example:443/ws",
            "wss://gw.example/ws"
        ));
        assert!(!endpoint_matches(
            "wss://gw.example:8443/ws",
            "wss://gw.example/ws"
        ));
        assert!(!endpoint_matches(
            "wss://gw.example/ws",
            "wss://gw.example/ws2"
        ));
        assert!(!endpoint_matches(
            "wss://gw.example/ws?a=1",
            "wss://gw.example/ws"
        ));
        assert!(!endpoint_matches(
            "wss://gw.example/ws#x",
            "wss://gw.example/ws"
        ));
        assert!(!endpoint_matches(
            "ws://localhost:8080/ws",
            "ws://127.0.0.1:8080/ws"
        ));
        assert!(!endpoint_matches(
            "ws://localhost:81/ws",
            "ws://localhost:80/ws"
        ));
        assert!(!endpoint_matches(
            "ws://localhost:80@evil/ws",
            "ws://localhost:80@evil/ws"
        ));
        assert!(!endpoint_matches(
            "wss://u:p@gw.example/ws",
            "wss://u:p@gw.example/ws"
        ));
        assert!(!endpoint_matches(
            "wss://gw.example/ws",
            "ws://gw.example/ws"
        ));
        assert!(!endpoint_matches("not a url", "not a url"));
    }

    #[test]
    fn stored_credential_connects_with_its_own_device_id_and_saves_nothing() {
        let ipc = FakeIpc::new(Some(("stored-dev", SECRET)));
        let sent = RefCell::new(Vec::new());
        let auth = NativeAuthState::default();
        let out = run(
            &ipc,
            &auth,
            URL,
            &always,
            Ok(ok_resp("stored-dev", Some("alice"), None)),
            &sent,
        );
        assert_eq!(
            out,
            NativeOutcome::Protected {
                user_id: "alice".into()
            }
        );
        let params = sent.borrow()[0].clone();
        assert_eq!(params["device_id"], "stored-dev");
        assert_eq!(params["device_token"], SECRET);
        assert!(params.get("bootstrap_ticket").is_none());
        assert!(ipc.saves().is_empty());
    }

    #[test]
    fn stored_credential_with_weak_echo_or_rebound_device_is_rejected() {
        for resp in [
            ok_resp("stored-dev", None, None),
            ok_resp("stored-dev", Some("  "), None),
            ok_resp("someone-else", Some("alice"), None),
            json!({"authorized": true, "user_id": "alice"}),
        ] {
            let ipc = FakeIpc::new(Some(("stored-dev", SECRET)));
            let sent = RefCell::new(Vec::new());
            let out = run(
                &ipc,
                &NativeAuthState::default(),
                URL,
                &always,
                Ok(resp),
                &sent,
            );
            assert_eq!(out, NativeOutcome::Failed(NativeFailure::Identity));
            assert!(ipc.saves().is_empty());
        }
    }

    #[test]
    fn bootstrap_uses_a_new_id_saves_only_the_server_token_then_protects() {
        let ipc = FakeIpc::new(Some(("old-dev", "old-token")));
        let sent = RefCell::new(Vec::new());
        let auth = NativeAuthState::default();
        auth.stash_ticket(TICKET.into());
        let out = run(
            &ipc,
            &auth,
            URL,
            &always,
            Ok(ok_resp("panel-NEW", Some("alice"), Some(SECRET))),
            &sent,
        );
        assert_eq!(
            out,
            NativeOutcome::Protected {
                user_id: "alice".into()
            }
        );
        let params = sent.borrow()[0].clone();
        assert_eq!(params["device_id"], "panel-NEW");
        assert_eq!(params["bootstrap_ticket"], TICKET);
        assert!(
            params.get("device_token").is_none(),
            "ticket wins over stored"
        );
        let saves = ipc.saves();
        assert_eq!(saves.len(), 1);
        assert_eq!(
            saves[0],
            json!({"lease_id": 7, "credential": {"device_id": "panel-NEW", "device_token": SECRET}})
        );
        assert!(
            !saves[0].to_string().contains(TICKET),
            "ticket never persisted"
        );
        assert_eq!(auth.take_ticket(), None, "ticket is single use");
    }

    #[test]
    fn bootstrap_with_a_weak_or_inconsistent_reply_never_saves() {
        let cases = [
            ok_resp("panel-NEW", Some("alice"), None), // no token
            ok_resp("panel-NEW", None, Some(SECRET)),  // no user
            ok_resp("panel-OTHER", Some("alice"), Some(SECRET)), // id not ours
            ok_resp("panel-NEW", Some("alice"), Some(TICKET)), // ticket echoed
            ok_resp("panel-NEW", Some("alice"), Some("   ")), // blank token
        ];
        for resp in cases {
            let ipc = FakeIpc::new(None);
            let auth = NativeAuthState::default();
            auth.stash_ticket(TICKET.into());
            let sent = RefCell::new(Vec::new());
            let out = run(&ipc, &auth, URL, &always, Ok(resp), &sent);
            assert_eq!(out, NativeOutcome::Failed(NativeFailure::Identity));
            assert!(ipc.saves().is_empty());
        }
    }

    #[test]
    fn protected_only_after_the_save_acks() {
        let mut ipc = FakeIpc::new(None);
        ipc.save = Err(BridgeError::Rejected);
        let auth = NativeAuthState::default();
        auth.stash_ticket(TICKET.into());
        let sent = RefCell::new(Vec::new());
        let out = run(
            &ipc,
            &auth,
            URL,
            &always,
            Ok(ok_resp("panel-NEW", Some("alice"), Some(SECRET))),
            &sent,
        );
        assert_eq!(
            out,
            NativeOutcome::Failed(NativeFailure::Save(BridgeError::Rejected))
        );
        // A non-null ACK is not an ACK either.
        let mut ipc = FakeIpc::new(None);
        ipc.save = Ok(json!({"ok": true}));
        let auth = NativeAuthState::default();
        auth.stash_ticket(TICKET.into());
        let out = run(
            &ipc,
            &auth,
            URL,
            &always,
            Ok(ok_resp("panel-NEW", Some("alice"), Some(SECRET))),
            &sent,
        );
        assert_eq!(
            out,
            NativeOutcome::Failed(NativeFailure::Save(BridgeError::Malformed))
        );
    }

    #[test]
    fn stale_before_the_save_never_calls_the_backend() {
        let flag = Cell::new(true);
        let current = || flag.get();
        let ipc = FakeIpc::new(None);
        let auth = NativeAuthState::default();
        auth.stash_ticket(TICKET.into());
        let mint = || "panel-NEW".to_string();
        let out = block_on(native_handshake(
            &ipc,
            HandshakeRequest {
                dialed_url: URL,
                device_name: "Panel",
                auth: &auth,
                new_device_id: &mint,
            },
            &current,
            |_params| {
                // A disconnect / new attempt / endpoint change lands while the
                // Gateway call is in flight.
                flag.set(false);
                std::future::ready(Ok(ok_resp("panel-NEW", Some("alice"), Some(SECRET))))
            },
        ));
        assert_eq!(out, NativeOutcome::Superseded);
        assert!(ipc.saves().is_empty(), "no OS write for a stale attempt");
    }

    #[test]
    fn stale_after_the_save_is_not_accepted() {
        let flag = Cell::new(true);
        let current = || flag.get();
        let mut ipc = FakeIpc::new(None);
        // SAFETY of the test: the closure only flips a Cell owned by this frame.
        let flag_ref: &'static Cell<bool> = unsafe { &*(&flag as *const Cell<bool>) };
        ipc.on_save = Box::new(move || flag_ref.set(false));
        let auth = NativeAuthState::default();
        auth.stash_ticket(TICKET.into());
        let mint = || "panel-NEW".to_string();
        let out = block_on(native_handshake(
            &ipc,
            HandshakeRequest {
                dialed_url: URL,
                device_name: "Panel",
                auth: &auth,
                new_device_id: &mint,
            },
            &current,
            |_params| std::future::ready(Ok(ok_resp("panel-NEW", Some("alice"), Some(SECRET)))),
        ));
        assert_eq!(out, NativeOutcome::Superseded);
        assert_eq!(ipc.saves().len(), 1);
    }

    #[test]
    fn stale_before_connect_sends_nothing_and_keeps_the_ticket() {
        let ipc = FakeIpc::new(None);
        let auth = NativeAuthState::default();
        auth.stash_ticket(TICKET.into());
        let sent = RefCell::new(Vec::new());
        let out = run(&ipc, &auth, URL, &|| false, Ok(json!({})), &sent);
        assert_eq!(out, NativeOutcome::Superseded);
        assert!(sent.borrow().is_empty());
        assert_eq!(auth.take_ticket().as_deref(), Some(TICKET));
    }

    #[test]
    fn credentialless_local_compat_is_never_protected() {
        let ipc = FakeIpc::new(None);
        let auth = NativeAuthState::default();
        let sent = RefCell::new(Vec::new());
        // Even a reply that echoes an owner identity stays unprotected.
        let out = run(
            &ipc,
            &auth,
            URL,
            &always,
            Ok(ok_resp("implicit", Some("owner"), None)),
            &sent,
        );
        assert_eq!(out, NativeOutcome::Credentialless);
        let params = sent.borrow()[0].clone();
        assert!(params.get("device_token").is_none() && params.get("bootstrap_ticket").is_none());
        assert_eq!(params["device_id"], "panel-NEW");
        assert!(ipc.saves().is_empty());
    }

    #[test]
    fn unauthorized_walls_and_remembers_whether_a_credential_was_presented() {
        let sent = RefCell::new(Vec::new());
        let walled = json!({"authorized": false, "needs_token": true});
        let ipc = FakeIpc::new(None);
        assert_eq!(
            run(
                &ipc,
                &NativeAuthState::default(),
                URL,
                &always,
                Ok(walled.clone()),
                &sent
            ),
            NativeOutcome::NeedsToken {
                was_rejected: false
            }
        );
        let ipc = FakeIpc::new(Some(("d", SECRET)));
        assert_eq!(
            run(
                &ipc,
                &NativeAuthState::default(),
                URL,
                &always,
                Ok(walled),
                &sent
            ),
            NativeOutcome::NeedsToken { was_rejected: true }
        );
    }

    #[test]
    fn failures_and_outcomes_never_contain_secrets() {
        let mut ipc = FakeIpc::new(None);
        ipc.save = Err(BridgeError::Rejected);
        let auth = NativeAuthState::default();
        auth.stash_ticket(TICKET.into());
        let sent = RefCell::new(Vec::new());
        let out = run(
            &ipc,
            &auth,
            URL,
            &always,
            Ok(ok_resp("panel-NEW", Some("alice"), Some(SECRET))),
            &sent,
        );
        let NativeOutcome::Failed(failure) = &out else {
            panic!("expected failure");
        };
        for text in [format!("{out:?}"), failure.message().to_string()] {
            assert!(!text.contains(SECRET) && !text.contains(TICKET), "{text}");
        }
        for f in [
            NativeFailure::Bridge(BridgeError::Unavailable),
            NativeFailure::Bridge(BridgeError::Rejected),
            NativeFailure::Endpoint,
            NativeFailure::Connect,
            NativeFailure::Identity,
            NativeFailure::Save(BridgeError::Rejected),
        ] {
            assert!(!f.message().is_empty());
        }
    }

    #[test]
    fn connect_transport_errors_become_a_payloadless_connect_failure() {
        let ipc = FakeIpc::new(None);
        let sent = RefCell::new(Vec::new());
        let out = run(
            &ipc,
            &NativeAuthState::default(),
            URL,
            &always,
            Err(ConnectSendError),
            &sent,
        );
        assert_eq!(out, NativeOutcome::Failed(NativeFailure::Connect));
    }

    #[test]
    fn a_new_attempt_or_invalidate_makes_older_receipts_stale() {
        let auth = NativeAuthState::default();
        let a = auth.begin().unwrap();
        assert!(a.is_current());
        let b = auth.begin().unwrap();
        assert!(!a.is_current() && b.is_current());
        auth.invalidate();
        assert!(!b.is_current());
        assert!(!attempt_current(&None));
        assert!(attempt_current(&auth.begin()));
    }

    #[test]
    fn an_exhausted_clock_fails_closed() {
        let clock = Arc::new(AttemptClock(AtomicU64::new(u64::MAX - 1)));
        assert!(AttemptClock::begin(&clock).is_none());
        clock.invalidate();
        let stale = AttemptReceipt {
            clock: Arc::clone(&clock),
            id: u64::MAX,
        };
        assert!(!stale.is_current());
    }

    #[test]
    fn ticket_comes_from_the_query_only_when_nonempty() {
        assert_eq!(ticket_from_search("?x=1&bt=abc"), Some("abc".into()));
        assert_eq!(ticket_from_search("bt=abc"), Some("abc".into()));
        assert_eq!(ticket_from_search("?bt="), None);
        assert_eq!(ticket_from_search("?token=abc"), None);
    }

    #[test]
    fn a_missing_marker_does_not_downgrade_a_native_shell() {
        assert!(!native_expected_from(false, false, false));
        assert!(native_expected_from(true, false, false));
        assert!(native_expected_from(false, true, false));
        assert!(native_expected_from(false, false, true));
    }

    #[test]
    fn protected_user_must_be_nonblank() {
        assert!(is_protected_user(Some("alice")));
        assert!(!is_protected_user(Some("  ")));
        assert!(!is_protected_user(None));
    }

    #[test]
    fn host_ipc_is_unavailable_not_a_fallback() {
        assert_eq!(
            block_on(TauriIpc.invoke(COMMAND_LOAD, Value::Null)).unwrap_err(),
            BridgeError::Unavailable
        );
    }
}
