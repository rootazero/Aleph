//! Native OS-keyring Bridge for the Panel (Task 41, phase 1).
//!
//! Thin I/O only. The Panel (served by the Gateway, hence a *remote* origin to
//! Tauri) may call exactly three fixed commands:
//!
//! * `panel_credential_load`   -> [`PanelCredentialLease`]
//! * `panel_credential_save`   (`lease_id`, `credential`) -> `()`
//! * `panel_credential_delete` (`lease_id`) -> `bool`
//!
//! Authorization is layered; every layer must pass, none is a substitute:
//!
//! 1. **Tauri ACL** — `permissions/keyring.toml` defines one
//!    `allow-protected-panel-credential-*` permission per command (each
//!    `commands.allow` is a single method; the `protected-` infix keeps them
//!    clear of the `allow-panel-credential-*` ids that `build.rs`'s
//!    `AppManifest` auto-generates for the same commands). At startup
//!    [`install`] adds a *runtime* capability that is `local(false)`,
//!    `window("main")`, remote-URL = the **native-configured** Gateway origin
//!    only, carrying a [`GatewayCredentialScope`] bound to that origin. Tauri
//!    evaluates remote ACL against `InvokeRequest.url`, so a page on another
//!    origin (or the `tauri://localhost` connect page) never reaches us.
//!    A target change needs a restart to get a new capability (see
//!    [`commit_target_change`] for the in-process half). The auto-generated
//!    `allow-*` ids are granted to nobody except the nine legacy commands in
//!    `capabilities/legacy-local-ipc.json` (local origins only).
//! 2. **Resolved scope check** — [`authorize`]: the resolved
//!    `CommandScope<GatewayCredentialScope>` must be **non-empty** (Tauri's
//!    `CommandScope::matches` returns `true` for an empty allow list, so we
//!    never use it), no deny entry may name the native origin (an unparsable
//!    deny entry fails closed), and some allow entry must equal the **current
//!    native** origin.
//! 3. **Live state check** — the calling webview *and* window labels are
//!    `main`, and the main webview's *current* URL is served by the current
//!    native [`ConnectionTarget`] (`serves_origin`).
//!
//! The keyring account is never taken from the Panel: it is derived from the
//! native configuration (`notify::ws_url(target)`), and the client store
//! enforces its own transport rule (wss, or ws to loopback only). This module
//! never accepts an account, User, role, ticket or generic RPC, and never
//! decides who a User is — the Gateway stays the only authority for that.
//!
//! Concurrency: all OS-store I/O runs in `spawn_blocking`. ONE gate (a
//! `Mutex<State>` holding the lease table *and* the target epoch) is held for
//! the *whole* operation (check -> I/O -> re-check). The target marker writer
//! (`connection::save_target`, which every target change — set, clear, connect
//! — funnels through) takes the same gate via [`commit_target_change`], bumps
//! the epoch and writes the file inside it. Consequences:
//!
//! * a target switch can never commit while a keyring operation is between
//!   its check and its I/O result (the writer waits for the gate);
//! * once a switch has committed, any older lease — including after an
//!   A -> B -> A round trip, or a rewrite of the same target — fails the
//!   epoch check before the backend is touched;
//! * the epoch is `checked_add`ed; on overflow the bridge fails closed
//!   forever ([`BridgeError::LeaseExhausted`], restart to reset).
//!
//! The gate is held only around synchronous work: never across an `.await`,
//! never across `Webview::url()` (read before the worker starts and again
//! after it returns), and never while emitting events or navigating (those run
//! after `save_target` returns). The gate is held across OS-store I/O, and an
//! OS unlock/credential prompt has no upper bound, so a target write can wait
//! on the provider for as long as the user (or the OS) takes. That wait is
//! never on the main thread: `set_connection_target` / `clear_connection_target`
//! are async commands that run `save_target` in `spawn_blocking`. No gate
//! holder ever needs the main thread, so this is a wait, not a deadlock. A new
//! `load` replaces the active lease.
//!
//! Post-I/O re-check: after the worker returns, [`KeyringBridge::recheck_request`]
//! re-takes the gate and re-validates the operation's OWN lease id (active
//! lease id + epoch + native target + a freshly captured webview URL) before
//! the result (credential / ACK) is handed to the Panel. A failed re-check only
//! compare-clears that lease; it never kills a newer lease installed by a
//! later `load`. Note the native effect of `save` / `delete` cannot be
//! cancelled: an `Err` returned from a re-check (or from the in-gate post-I/O
//! check) means "the result was withheld", NOT "nothing was written" — the
//! store write/removal may already have committed. No compensating rollback is
//! attempted (it could overwrite a newer user's data).
//!
//! Residual: the epoch only observes writes made through `save_target`. A
//! *third party* editing `~/.aleph/<prefix>-target` while a keyring operation
//! is in flight is caught by the post-I/O native re-read (the lease's native
//! origin/ws_url must still match) and fails closed, but a transient external
//! A -> B -> A edit inside one operation is not observable.
//!
//! Errors are fixed, secret-free codes ([`BridgeError`]); backend errors and
//! payloads are never forwarded or logged.

use std::sync::{Arc, Mutex, OnceLock};

use aleph_protocol::native_credentials::{DeviceConnectCredential, PanelCredentialLease};
use serde::{Deserialize, Serialize};
use tauri::ipc::{CapabilityBuilder, CommandScope};
use tauri::{Manager, Url};

use crate::connection::ConnectionTarget;

/// Only webview/window allowed to hold the Bridge.
const MAIN_LABEL: &str = "main";
/// Capability id of the runtime capability added by [`install`].
const CAPABILITY_ID: &str = "panel-keyring";
/// ACL permission identifiers (defined in `permissions/keyring.toml`). The
/// `protected-` infix is deliberate: `build.rs` registers the same three
/// commands in the `AppManifest`, which auto-generates `allow-panel-credential-*`.
const PERMISSIONS: [&str; 3] = [
    "allow-protected-panel-credential-load",
    "allow-protected-panel-credential-save",
    "allow-protected-panel-credential-delete",
];

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Fixed, secret-free failure codes returned to the Panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeError {
    /// ACL scope / label / URL / origin policy refused the call.
    Denied,
    /// save/delete without any active lease.
    NoLease,
    /// The presented lease id is not the active lease (superseded).
    StaleLease,
    /// The native target changed while the lease/operation was live.
    TargetChanged,
    /// The credential payload failed validation.
    Invalid,
    /// The OS keyring (or its transport rule) failed; details withheld.
    Storage,
    /// Lease-id or target-epoch space exhausted (fail closed; restart to
    /// reset).
    LeaseExhausted,
    /// Internal invariant failure (poisoned lock, join error, ...).
    Internal,
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Denied => "credential bridge: denied",
            Self::NoLease => "credential bridge: no active lease",
            Self::StaleLease => "credential bridge: stale lease",
            Self::TargetChanged => "credential bridge: connection target changed",
            Self::Invalid => "credential bridge: invalid credential",
            Self::Storage => "credential bridge: storage failure",
            Self::LeaseExhausted => "credential bridge: lease/epoch space exhausted",
            Self::Internal => "credential bridge: internal error",
        })
    }
}

impl std::error::Error for BridgeError {}

// ---------------------------------------------------------------------------
// Scope + pure policy
// ---------------------------------------------------------------------------

/// Command scope object: the Gateway origin a permission is bound to
/// (`scheme://host[:port]`, no path). Serialized into the runtime capability by
/// [`install`] and resolved back into `CommandScope<GatewayCredentialScope>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayCredentialScope {
    pub gateway_origin: String,
}

/// Canonical origin string, or `None` for anything that is not exactly a bare
/// http(s) tuple origin (paths, queries, opaque origins are all rejected).
fn canonical_origin(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.origin().is_tuple() {
        return None;
    }
    let canonical = url.origin().ascii_serialization();
    (canonical == raw).then_some(canonical)
}

/// What the native configuration says the keyring is for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Native {
    origin: String,
    ws_url: String,
}

fn native_of(target: &ConnectionTarget) -> Native {
    let origin = match target {
        ConnectionTarget::Local => crate::PANEL_URL.to_string(),
        ConnectionTarget::Remote(url) => url.origin().ascii_serialization(),
    };
    Native {
        origin,
        ws_url: crate::notify::ws_url(target),
    }
}

/// Everything about one IPC call that the policy needs, captured *before* the
/// blocking worker starts.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    webview_label: String,
    window_label: String,
    url: Url,
    allows: Vec<GatewayCredentialScope>,
    denies: Vec<GatewayCredentialScope>,
}

impl Request {
    fn from_tauri(
        webview: &tauri::Webview,
        scope: &CommandScope<GatewayCredentialScope>,
    ) -> Result<Self, BridgeError> {
        Ok(Self {
            webview_label: webview.label().to_string(),
            window_label: webview.window().label().to_string(),
            url: webview.url().map_err(|_| BridgeError::Denied)?,
            allows: scope.allows().iter().map(|s| s.as_ref().clone()).collect(),
            denies: scope.denies().iter().map(|s| s.as_ref().clone()).collect(),
        })
    }
}

/// Pure policy: may this call touch the keyring for `target`?
fn authorize(req: &Request, target: &ConnectionTarget) -> Result<Native, BridgeError> {
    if req.webview_label != MAIN_LABEL || req.window_label != MAIN_LABEL {
        return Err(BridgeError::Denied);
    }
    // `CommandScope::matches` is true for an empty allow list: never use it.
    if req.allows.is_empty() {
        return Err(BridgeError::Denied);
    }
    let native = native_of(target);
    // Deny wins; an unparsable deny entry cannot be proven unrelated -> deny.
    for deny in &req.denies {
        match canonical_origin(&deny.gateway_origin) {
            Some(origin) if origin != native.origin => {}
            _ => return Err(BridgeError::Denied),
        }
    }
    // Unparsable allow entries are simply ignored (they can only narrow).
    let allowed = req
        .allows
        .iter()
        .filter_map(|a| canonical_origin(&a.gateway_origin))
        .any(|origin| origin == native.origin);
    if !allowed || !target.serves_origin(&req.url) {
        return Err(BridgeError::Denied);
    }
    Ok(native)
}

// ---------------------------------------------------------------------------
// Backend + bridge state
// ---------------------------------------------------------------------------

/// Synchronous credential storage keyed by the native-derived WebSocket URL.
/// Implementations must return secret-free failures only.
pub trait CredentialBackend: Send + Sync + 'static {
    fn load(&self, ws_url: &str) -> Result<Option<DeviceConnectCredential>, BridgeError>;
    fn save(&self, ws_url: &str, credential: &DeviceConnectCredential) -> Result<(), BridgeError>;
    fn delete(&self, ws_url: &str) -> Result<bool, BridgeError>;
}

/// Production backend: the shared client OS-keyring store through its
/// `*_for_connect` adapters, which enforce the wss-or-loopback transport rule
/// before any OS I/O and refuse blank values/tickets.
pub struct ClientStoreBackend;

impl CredentialBackend for ClientStoreBackend {
    fn load(&self, ws_url: &str) -> Result<Option<DeviceConnectCredential>, BridgeError> {
        aleph_client::DeviceCredentialStore::for_gateway(ws_url)
            .and_then(|s| s.load_for_connect())
            .map_err(|_| BridgeError::Storage)
    }
    fn save(&self, ws_url: &str, credential: &DeviceConnectCredential) -> Result<(), BridgeError> {
        aleph_client::DeviceCredentialStore::for_gateway(ws_url)
            .and_then(|s| s.save_for_connect(credential))
            .map_err(|_| BridgeError::Storage)
    }
    fn delete(&self, ws_url: &str) -> Result<bool, BridgeError> {
        aleph_client::DeviceCredentialStore::for_gateway(ws_url)
            .and_then(|s| s.delete_for_connect())
            .map_err(|_| BridgeError::Storage)
    }
}

/// QA-only backend (`qa-owned-root`): a process-local map that is never
/// persisted and never touches the OS keyring. Chosen by [`KeyringBridge::production`]
/// instead of [`ClientStoreBackend`] in that build only; origin/scope/lease/epoch
/// authorization above the backend is byte-for-byte the production path.
#[cfg(feature = "qa-owned-root")]
#[derive(Default)]
pub struct InMemoryQaBackend {
    store: Mutex<std::collections::HashMap<String, DeviceConnectCredential>>,
}

#[cfg(feature = "qa-owned-root")]
impl CredentialBackend for InMemoryQaBackend {
    fn load(&self, ws_url: &str) -> Result<Option<DeviceConnectCredential>, BridgeError> {
        let store = self.store.lock().map_err(|_| BridgeError::Storage)?;
        Ok(store.get(ws_url).cloned())
    }
    fn save(&self, ws_url: &str, credential: &DeviceConnectCredential) -> Result<(), BridgeError> {
        let mut store = self.store.lock().map_err(|_| BridgeError::Storage)?;
        store.insert(ws_url.to_string(), credential.clone());
        Ok(())
    }
    fn delete(&self, ws_url: &str) -> Result<bool, BridgeError> {
        let mut store = self.store.lock().map_err(|_| BridgeError::Storage)?;
        Ok(store.remove(ws_url).is_some())
    }
}

/// QA-only OS backend (`qa-owned-root`, `--qa-credential-backend os`): the same
/// shared client store and `*_for_connect` transport rules as
/// [`ClientStoreBackend`], but filed under the random QA-only service
/// `aleph-qa.device-credential.<nonce>` so it can never read, overwrite or
/// delete a production credential. The client re-validates the namespace.
#[cfg(feature = "qa-owned-root")]
pub struct QaOsKeyringBackend {
    service: String,
}

#[cfg(feature = "qa-owned-root")]
impl QaOsKeyringBackend {
    fn new(service: String) -> Self {
        Self { service }
    }
}

#[cfg(feature = "qa-owned-root")]
impl CredentialBackend for QaOsKeyringBackend {
    fn load(&self, ws_url: &str) -> Result<Option<DeviceConnectCredential>, BridgeError> {
        aleph_client::DeviceCredentialStore::for_gateway_in_service(ws_url, &self.service)
            .and_then(|s| s.load_for_connect())
            .map_err(|_| BridgeError::Storage)
    }
    fn save(&self, ws_url: &str, credential: &DeviceConnectCredential) -> Result<(), BridgeError> {
        aleph_client::DeviceCredentialStore::for_gateway_in_service(ws_url, &self.service)
            .and_then(|s| s.save_for_connect(credential))
            .map_err(|_| BridgeError::Storage)
    }
    fn delete(&self, ws_url: &str) -> Result<bool, BridgeError> {
        aleph_client::DeviceCredentialStore::for_gateway_in_service(ws_url, &self.service)
            .and_then(|s| s.delete_for_connect())
            .map_err(|_| BridgeError::Storage)
    }
}

#[derive(Debug, Clone)]
struct Lease {
    id: u64,
    native: Native,
    epoch: u64,
}

/// The single gate: lease table + target epoch, behind one mutex shared with
/// the target-marker writer (see the module docs).
#[derive(Debug)]
struct State {
    next_lease_id: u64,
    active: Option<Lease>,
    /// Bumped (under the gate) by every committed target write.
    epoch: u64,
    /// Set when the epoch would overflow: the bridge is dead until restart.
    exhausted: bool,
}

impl State {
    fn new() -> Self {
        Self {
            next_lease_id: 1,
            active: None,
            epoch: 0,
            exhausted: false,
        }
    }

    /// Record that the native target is being (re)written. Never panics and
    /// never refuses the write itself: on overflow only the *bridge* fails
    /// closed, permanently.
    fn invalidate_for_target_change(&mut self) {
        match self.epoch.checked_add(1) {
            Some(next) => self.epoch = next,
            None => self.exhausted = true,
        }
    }
}

type Gate = Arc<Mutex<State>>;
type TargetSource = Arc<dyn Fn() -> ConnectionTarget + Send + Sync>;

/// Managed Tauri state. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct KeyringBridge {
    backend: Arc<dyn CredentialBackend>,
    target: TargetSource,
    gate: Gate,
}

/// The process-wide gate shared by the production bridge and the target
/// writer ([`commit_target_change`]).
fn production_gate() -> Gate {
    static GATE: OnceLock<Gate> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(Mutex::new(State::new())))
        .clone()
}

/// Run `write` (the target-marker file write) inside `gate`, after bumping the
/// epoch. Holding the gate means no keyring operation is mid-flight and none
/// can start until the new target is on disk; older leases are dead afterwards.
/// A poisoned gate (a keyring worker panicked) never blocks the write: the
/// bridge is marked exhausted instead.
fn commit_on<T>(gate: &Mutex<State>, write: impl FnOnce() -> T) -> T {
    let mut st = match gate.lock() {
        Ok(st) => st,
        Err(poisoned) => {
            let mut st = poisoned.into_inner();
            st.exhausted = true;
            st
        }
    };
    st.invalidate_for_target_change();
    write()
}

/// Hook for `connection::save_target` — the one function every target change
/// (set / clear / connect-to) writes the marker through. Synchronous; the
/// caller emits events / navigates only AFTER this returns.
pub(crate) fn commit_target_change<T>(write: impl FnOnce() -> T) -> T {
    commit_on(&production_gate(), write)
}

impl KeyringBridge {
    /// Bridge with its own gate (tests; production uses the shared one).
    #[cfg(test)]
    pub(crate) fn new(backend: Arc<dyn CredentialBackend>, target: TargetSource) -> Self {
        Self::with_gate(backend, target, Arc::new(Mutex::new(State::new())))
    }

    fn with_gate(backend: Arc<dyn CredentialBackend>, target: TargetSource, gate: Gate) -> Self {
        Self {
            backend,
            target,
            gate,
        }
    }

    fn production() -> Self {
        // QA build: process-local in-memory store by default; only an explicit,
        // proven `--qa-credential-backend os` reaches the real OS keyring, and
        // then only under the QA-only service namespace. A QA build never falls
        // back to `ClientStoreBackend` (the production service).
        #[cfg(feature = "qa-owned-root")]
        let backend: Arc<dyn CredentialBackend> = match crate::qa_root::credential_backend() {
            Some(crate::qa_root::CredentialBackendChoice::Os { service }) => {
                Arc::new(QaOsKeyringBackend::new(service.clone()))
            }
            Some(crate::qa_root::CredentialBackendChoice::Memory) | None => {
                Arc::new(InMemoryQaBackend::default())
            }
        };
        #[cfg(not(feature = "qa-owned-root"))]
        let backend: Arc<dyn CredentialBackend> = Arc::new(ClientStoreBackend);
        Self::with_gate(
            backend,
            Arc::new(crate::connection::load_target),
            production_gate(),
        )
    }

    /// Same as [`commit_target_change`] on this bridge's gate.
    #[cfg(test)]
    pub(crate) fn commit_target_change<T>(&self, write: impl FnOnce() -> T) -> T {
        commit_on(&self.gate, write)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, BridgeError> {
        self.gate.lock().map_err(|_| BridgeError::Internal)
    }

    /// Is `lease` still bound to the live native config and epoch? Called with
    /// the gate held, so the epoch cannot move underneath the answer.
    fn lease_live(&self, st: &State, lease: &Lease) -> bool {
        !st.exhausted && lease.epoch == st.epoch && native_of(&(self.target)()) == lease.native
    }

    /// Blocking: begin a lease and read the credential.
    pub(crate) fn load_blocking(&self, req: &Request) -> Result<PanelCredentialLease, BridgeError> {
        let mut st = self.lock()?;
        st.active = None; // any older lease is dead from this point on
        if st.exhausted {
            return Err(BridgeError::LeaseExhausted);
        }
        let native = authorize(req, &(self.target)())?;
        let epoch = st.epoch;
        let id = st.next_lease_id;
        st.next_lease_id = id.checked_add(1).ok_or(BridgeError::LeaseExhausted)?;
        let credential = self.backend.load(&native.ws_url)?;
        let lease = Lease { id, native, epoch };
        if !self.lease_live(&st, &lease) {
            return Err(BridgeError::TargetChanged);
        }
        let out = PanelCredentialLease::new(lease.native.ws_url.clone(), id, credential)
            .map_err(|_| BridgeError::Storage)?;
        st.active = Some(lease);
        Ok(out)
    }

    /// Shared prologue for save/delete: authorize + match the active lease.
    fn leased(&self, st: &State, req: &Request, lease_id: u64) -> Result<Lease, BridgeError> {
        if st.exhausted {
            return Err(BridgeError::LeaseExhausted);
        }
        authorize(req, &(self.target)())?;
        let lease = st.active.clone().ok_or(BridgeError::NoLease)?;
        if lease.id != lease_id || lease_id == 0 {
            return Err(BridgeError::StaleLease);
        }
        if !self.lease_live(st, &lease) {
            return Err(BridgeError::TargetChanged);
        }
        Ok(lease)
    }

    /// Blocking: persist `credential` under `lease_id`.
    pub(crate) fn save_blocking(
        &self,
        req: &Request,
        lease_id: u64,
        credential: &DeviceConnectCredential,
    ) -> Result<(), BridgeError> {
        credential.validate().map_err(|_| BridgeError::Invalid)?;
        let mut st = self.lock()?;
        let lease = match self.leased(&st, req, lease_id) {
            Ok(l) => l,
            Err(e) => {
                if e == BridgeError::TargetChanged {
                    st.active = None;
                }
                return Err(e);
            }
        };
        self.backend.save(&lease.native.ws_url, credential)?;
        if !self.lease_live(&st, &lease) {
            st.active = None;
            return Err(BridgeError::TargetChanged);
        }
        Ok(())
    }

    /// Blocking: delete the stored credential under `lease_id`.
    pub(crate) fn delete_blocking(
        &self,
        req: &Request,
        lease_id: u64,
    ) -> Result<bool, BridgeError> {
        let mut st = self.lock()?;
        let lease = match self.leased(&st, req, lease_id) {
            Ok(l) => l,
            Err(e) => {
                if e == BridgeError::TargetChanged {
                    st.active = None;
                }
                return Err(e);
            }
        };
        let existed = self.backend.delete(&lease.native.ws_url)?;
        if !self.lease_live(&st, &lease) {
            st.active = None;
            return Err(BridgeError::TargetChanged);
        }
        Ok(existed)
    }

    /// Post-worker re-check for the operation that held `lease_id`. `post` is
    /// the request captured AFTER the worker returned (the page may have
    /// navigated meanwhile; `Webview::url()` is read by the caller, outside the
    /// gate). Under the gate this re-runs the full `leased` check (authorize,
    /// active id == `lease_id`, epoch, native target), so an A -> B -> A switch
    /// or a superseding `load` cannot let an old result through. On failure only
    /// the lease with this very id is cleared (compare-clear): a stale reply
    /// must never revoke a newer active lease. May block on the gate (it can be
    /// held across OS I/O): call from `spawn_blocking`.
    pub(crate) fn recheck_request(
        &self,
        post: Result<Request, BridgeError>,
        lease_id: u64,
    ) -> Result<(), BridgeError> {
        let mut st = self.lock()?;
        let outcome = post.and_then(|req| self.leased(&st, &req, lease_id).map(|_| ()));
        if outcome.is_err() && st.active.as_ref().is_some_and(|l| l.id == lease_id) {
            st.active = None;
        }
        outcome
    }
}

// ---------------------------------------------------------------------------
// Tauri commands + install
// ---------------------------------------------------------------------------

async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, BridgeError> + Send + 'static,
) -> Result<T, BridgeError> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| BridgeError::Internal)?
}

/// QA-only (`qa-owned-root`): map the FINAL handler result (after the
/// post-operation lease re-check) to the fixed-shape trace outcome that
/// [`crate::qa_trace::record_blocking`] emits, so the trace says what the Panel
/// was given. Maps only to fixed shapes / snake_case error codes; no value is
/// read. Synchronous and borrow-only: the borrow ends before the handler awaits
/// the (blocking) trace write.
#[cfg(feature = "qa-owned-root")]
fn qa_outcome<T>(
    result: &Result<T, BridgeError>,
    ok: impl FnOnce(&T) -> crate::qa_trace::Outcome,
) -> crate::qa_trace::Outcome {
    match result {
        Ok(v) => ok(v),
        Err(e) => crate::qa_trace::Outcome::Err(match e {
            BridgeError::Denied => "denied",
            BridgeError::NoLease => "no_lease",
            BridgeError::StaleLease => "stale_lease",
            BridgeError::TargetChanged => "target_changed",
            BridgeError::Invalid => "invalid",
            BridgeError::Storage => "storage",
            BridgeError::LeaseExhausted => "lease_exhausted",
            BridgeError::Internal => "internal",
        }),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn panel_credential_load(
    webview: tauri::Webview,
    scope: CommandScope<GatewayCredentialScope>,
    bridge: tauri::State<'_, KeyringBridge>,
) -> Result<PanelCredentialLease, BridgeError> {
    #[cfg_attr(not(feature = "qa-owned-root"), allow(clippy::let_and_return))]
    let result = async {
        let req = Request::from_tauri(&webview, &scope)?;
        let b = bridge.inner().clone();
        let lease = run_blocking(move || b.load_blocking(&req)).await?;
        let lease_id = lease.lease_id;
        let post = Request::from_tauri(&webview, &scope);
        let b = bridge.inner().clone();
        run_blocking(move || b.recheck_request(post, lease_id)).await?;
        Ok::<PanelCredentialLease, BridgeError>(lease)
    }
    .await;
    #[cfg(feature = "qa-owned-root")]
    crate::qa_trace::record_blocking(
        crate::qa_trace::Command::Load,
        qa_outcome(&result, |lease| crate::qa_trace::Outcome::Ok {
            shape: "lease_object",
            credential_present: Some(lease.credential.is_some()),
        }),
    )
    .await;
    result
}

// `rename_all = "snake_case"` pins the wire argument names (`lease_id`,
// `credential`) the Panel will send; Tauri's default would be camelCase.
#[tauri::command(rename_all = "snake_case")]
pub async fn panel_credential_save(
    webview: tauri::Webview,
    scope: CommandScope<GatewayCredentialScope>,
    bridge: tauri::State<'_, KeyringBridge>,
    lease_id: u64,
    credential: DeviceConnectCredential,
) -> Result<(), BridgeError> {
    #[cfg_attr(not(feature = "qa-owned-root"), allow(clippy::let_and_return))]
    let result = async {
        let req = Request::from_tauri(&webview, &scope)?;
        let b = bridge.inner().clone();
        run_blocking(move || b.save_blocking(&req, lease_id, &credential)).await?;
        let post = Request::from_tauri(&webview, &scope);
        let b = bridge.inner().clone();
        run_blocking(move || b.recheck_request(post, lease_id)).await
    }
    .await;
    #[cfg(feature = "qa-owned-root")]
    crate::qa_trace::record_blocking(
        crate::qa_trace::Command::Save,
        qa_outcome(&result, |_| crate::qa_trace::Outcome::Ok {
            shape: "null",
            credential_present: None,
        }),
    )
    .await;
    result
}

#[tauri::command(rename_all = "snake_case")]
pub async fn panel_credential_delete(
    webview: tauri::Webview,
    scope: CommandScope<GatewayCredentialScope>,
    bridge: tauri::State<'_, KeyringBridge>,
    lease_id: u64,
) -> Result<bool, BridgeError> {
    #[cfg_attr(not(feature = "qa-owned-root"), allow(clippy::let_and_return))]
    let result = async {
        let req = Request::from_tauri(&webview, &scope)?;
        let b = bridge.inner().clone();
        let existed = run_blocking(move || b.delete_blocking(&req, lease_id)).await?;
        let post = Request::from_tauri(&webview, &scope);
        let b = bridge.inner().clone();
        run_blocking(move || b.recheck_request(post, lease_id)).await?;
        Ok::<bool, BridgeError>(existed)
    }
    .await;
    #[cfg(feature = "qa-owned-root")]
    crate::qa_trace::record_blocking(
        crate::qa_trace::Command::Delete,
        qa_outcome(&result, |_| crate::qa_trace::Outcome::Ok {
            shape: "bool",
            credential_present: None,
        }),
    )
    .await;
    result
}

/// Manage the bridge state and add the runtime capability bound to the
/// *current native* Gateway origin. Called once in `.setup` before
/// `build_main_window`. The capability is remote-only (`local(false)`),
/// `main`-only and carries the origin-bound scope; changing the target
/// requires a restart to mint a new capability.
///
/// Failure policy: the state is managed FIRST so the registered commands can
/// always resolve `State<KeyringBridge>`; if adding the capability then fails,
/// no window holds any grant for the three commands (Tauri's ACL denies them)
/// and the scope check in [`authorize`] would deny them anyway — protected
/// storage is simply unavailable, nothing falls back to a wider scope. The
/// caller logs the failure and startup continues.
pub fn install(app: &tauri::AppHandle) -> Result<(), BridgeError> {
    let bridge = KeyringBridge::production();
    app.manage(bridge.clone());
    let origin = native_of(&(bridge.target)()).origin;
    let scope = GatewayCredentialScope {
        gateway_origin: origin.clone(),
    };
    let mut cap = CapabilityBuilder::new(CAPABILITY_ID)
        .remote(format!("{origin}/*"))
        .window(MAIN_LABEL)
        .local(false);
    for permission in PERMISSIONS {
        cap = cap.permission_scoped(
            permission,
            vec![scope.clone()],
            Vec::<GatewayCredentialScope>::new(),
        );
    }
    app.add_capability(cap).map_err(|_| BridgeError::Internal)
}

/// Command names registered in both `generate_handler!` arms and in
/// `build.rs`'s `AppManifest` (a test pins the latter).
#[cfg(test)]
const HANDLER_NAMES: [&str; 3] = [
    aleph_protocol::native_credentials::COMMAND_LOAD,
    aleph_protocol::native_credentials::COMMAND_SAVE,
    aleph_protocol::native_credentials::COMMAND_DELETE,
];

// ---------------------------------------------------------------------------
// Tests (fake backend; no OS keyring, no Tauri runtime)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    #[derive(Default)]
    struct FakeBackend {
        store: Mutex<HashMap<String, DeviceConnectCredential>>,
        fail: Mutex<bool>,
        /// Number of backend calls (a rejected operation must leave it at 0).
        hits: AtomicUsize,
        /// Runs inside every backend call (simulates a barrier / mid-IO event).
        during_io: Mutex<Option<Box<dyn Fn() + Send>>>,
    }

    impl FakeBackend {
        fn hit(&self) -> Result<(), BridgeError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            if let Some(f) = self.during_io.lock().unwrap().as_ref() {
                f();
            }
            if *self.fail.lock().unwrap() {
                return Err(BridgeError::Storage);
            }
            Ok(())
        }
    }

    impl CredentialBackend for Arc<FakeBackend> {
        fn load(&self, ws: &str) -> Result<Option<DeviceConnectCredential>, BridgeError> {
            self.hit()?;
            Ok(self.store.lock().unwrap().get(ws).cloned())
        }
        fn save(&self, ws: &str, c: &DeviceConnectCredential) -> Result<(), BridgeError> {
            self.hit()?;
            self.store.lock().unwrap().insert(ws.to_string(), c.clone());
            Ok(())
        }
        fn delete(&self, ws: &str) -> Result<bool, BridgeError> {
            self.hit()?;
            Ok(self.store.lock().unwrap().remove(ws).is_some())
        }
    }

    struct Harness {
        bridge: KeyringBridge,
        backend: Arc<FakeBackend>,
        target: Arc<Mutex<ConnectionTarget>>,
    }

    /// Each harness owns an independent gate (production shares one).
    fn harness() -> Harness {
        let backend = Arc::new(FakeBackend::default());
        let target = Arc::new(Mutex::new(ConnectionTarget::Local));
        let t = target.clone();
        let bridge = KeyringBridge::new(
            Arc::new(backend.clone()),
            Arc::new(move || t.lock().unwrap().clone()),
        );
        Harness {
            bridge,
            backend,
            target,
        }
    }

    fn hits(h: &Harness) -> usize {
        h.backend.hits.load(Ordering::SeqCst)
    }

    fn remote_target() -> ConnectionTarget {
        ConnectionTarget::parse("https://gw.example.com:8443").unwrap()
    }

    fn scope(origin: &str) -> GatewayCredentialScope {
        GatewayCredentialScope {
            gateway_origin: origin.to_string(),
        }
    }

    fn local_req() -> Request {
        Request {
            webview_label: "main".into(),
            window_label: "main".into(),
            url: Url::parse("http://127.0.0.1:18790/settings").unwrap(),
            allows: vec![scope(crate::PANEL_URL)],
            denies: vec![],
        }
    }

    fn cred(token: &str) -> DeviceConnectCredential {
        DeviceConnectCredential::new("dev-1", token).unwrap()
    }

    #[test]
    fn empty_allow_list_is_denied() {
        let mut req = local_req();
        req.allows.clear();
        assert_eq!(
            authorize(&req, &ConnectionTarget::Local),
            Err(BridgeError::Denied)
        );
    }

    #[test]
    fn foreign_origin_scope_is_denied() {
        let mut req = local_req();
        req.allows = vec![scope("http://evil.example:18790")];
        assert_eq!(
            authorize(&req, &ConnectionTarget::Local),
            Err(BridgeError::Denied)
        );
    }

    #[test]
    fn scope_with_path_or_garbage_never_matches() {
        for bad in [
            "http://127.0.0.1:18790/",
            "http://127.0.0.1:18790/x",
            "not a url",
            "tauri://localhost",
        ] {
            let mut req = local_req();
            req.allows = vec![scope(bad)];
            assert_eq!(
                authorize(&req, &ConnectionTarget::Local),
                Err(BridgeError::Denied),
                "{bad}"
            );
        }
    }

    #[test]
    fn deny_wins_and_unparsable_deny_fails_closed() {
        let mut req = local_req();
        req.denies = vec![scope(crate::PANEL_URL)];
        assert_eq!(
            authorize(&req, &ConnectionTarget::Local),
            Err(BridgeError::Denied)
        );
        req.denies = vec![scope("???")];
        assert_eq!(
            authorize(&req, &ConnectionTarget::Local),
            Err(BridgeError::Denied)
        );
        req.denies = vec![scope("http://other.example:1")];
        assert!(authorize(&req, &ConnectionTarget::Local).is_ok());
    }

    #[test]
    fn wrong_label_or_foreign_webview_url_is_denied() {
        let mut req = local_req();
        req.webview_label = "popup".into();
        assert_eq!(
            authorize(&req, &ConnectionTarget::Local),
            Err(BridgeError::Denied)
        );
        let mut req = local_req();
        req.window_label = "other".into();
        assert_eq!(
            authorize(&req, &ConnectionTarget::Local),
            Err(BridgeError::Denied)
        );
        let mut req = local_req();
        req.url = Url::parse("http://evil.example/").unwrap();
        assert_eq!(
            authorize(&req, &ConnectionTarget::Local),
            Err(BridgeError::Denied)
        );
        req.url = Url::parse("tauri://localhost/index.html").unwrap();
        assert_eq!(
            authorize(&req, &ConnectionTarget::Local),
            Err(BridgeError::Denied)
        );
    }

    #[test]
    fn native_config_switch_makes_local_scope_foreign() {
        let remote = ConnectionTarget::parse("https://gw.example.com:8443").unwrap();
        // Local-bound scope + Local URL, but native config now says Remote.
        assert_eq!(authorize(&local_req(), &remote), Err(BridgeError::Denied));
    }

    #[test]
    fn load_save_delete_round_trip() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        assert!(lease.credential.is_none());
        assert_ne!(lease.lease_id, 0);
        h.bridge
            .save_blocking(&req, lease.lease_id, &cred("t1"))
            .unwrap();
        let again = h.bridge.load_blocking(&req).unwrap();
        assert_eq!(again.credential, Some(cred("t1")));
        assert_eq!(again.gateway_url, lease.gateway_url);
        assert!(h.bridge.delete_blocking(&req, again.lease_id).unwrap());
        assert!(!h.bridge.delete_blocking(&req, again.lease_id).unwrap());
    }

    #[test]
    fn save_and_delete_without_lease_are_rejected() {
        let h = harness();
        let req = local_req();
        assert_eq!(
            h.bridge.save_blocking(&req, 1, &cred("t")),
            Err(BridgeError::NoLease)
        );
        assert_eq!(h.bridge.delete_blocking(&req, 1), Err(BridgeError::NoLease));
        assert!(h.backend.store.lock().unwrap().is_empty());
    }

    #[test]
    fn superseded_and_zero_leases_are_rejected() {
        let h = harness();
        let req = local_req();
        let old = h.bridge.load_blocking(&req).unwrap().lease_id;
        let new = h.bridge.load_blocking(&req).unwrap().lease_id;
        assert_ne!(old, new);
        assert_eq!(
            h.bridge.save_blocking(&req, old, &cred("t")),
            Err(BridgeError::StaleLease)
        );
        assert_eq!(
            h.bridge.delete_blocking(&req, 0),
            Err(BridgeError::StaleLease)
        );
        assert!(h.bridge.save_blocking(&req, new, &cred("t")).is_ok());
    }

    #[test]
    fn target_change_invalidates_lease() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        // Config switched to a remote Gateway: lease is dead and nothing is written.
        *h.target.lock().unwrap() = ConnectionTarget::parse("https://gw.example.com:8443").unwrap();
        let err = h
            .bridge
            .save_blocking(&req, lease.lease_id, &cred("t"))
            .unwrap_err();
        assert_eq!(err, BridgeError::Denied); // Local-bound scope is foreign to the new target
        assert!(h.backend.store.lock().unwrap().is_empty());
    }

    #[test]
    fn a_to_b_to_a_switch_is_caught_by_epoch() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        let after_load = hits(&h);
        // Real writer path: A -> B -> A, config equal to the lease's native again.
        let t = h.target.clone();
        h.bridge
            .commit_target_change(|| *t.lock().unwrap() = remote_target());
        h.bridge
            .commit_target_change(|| *t.lock().unwrap() = ConnectionTarget::Local);
        assert_eq!(*h.target.lock().unwrap(), ConnectionTarget::Local);
        assert_eq!(
            h.bridge.save_blocking(&req, lease.lease_id, &cred("t")),
            Err(BridgeError::TargetChanged)
        );
        // lease was dropped: a retry with the same id is now NoLease.
        assert_eq!(
            h.bridge.save_blocking(&req, lease.lease_id, &cred("t")),
            Err(BridgeError::NoLease)
        );
        assert_eq!(
            h.bridge.delete_blocking(&req, lease.lease_id),
            Err(BridgeError::NoLease)
        );
        assert!(h.backend.store.lock().unwrap().is_empty());
        assert_eq!(hits(&h), after_load, "old lease reached the backend");
    }

    #[test]
    fn rewriting_the_same_target_still_kills_older_leases() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        let after_load = hits(&h);
        h.bridge.commit_target_change(|| ()); // e.g. connect_to("local") while Local
        assert_eq!(
            h.bridge.delete_blocking(&req, lease.lease_id),
            Err(BridgeError::TargetChanged)
        );
        assert_eq!(hits(&h), after_load);
        // A fresh load works again.
        assert!(h.bridge.load_blocking(&req).is_ok());
    }

    #[test]
    fn switch_committed_before_save_or_delete_never_reaches_the_backend() {
        for delete in [false, true] {
            let h = harness();
            let req = local_req();
            let lease = h.bridge.load_blocking(&req).unwrap();
            let after_load = hits(&h);
            let t = h.target.clone();
            h.bridge
                .commit_target_change(|| *t.lock().unwrap() = remote_target());
            let err = if delete {
                h.bridge.delete_blocking(&req, lease.lease_id).unwrap_err()
            } else {
                h.bridge
                    .save_blocking(&req, lease.lease_id, &cred("t"))
                    .unwrap_err()
            };
            assert!(
                matches!(err, BridgeError::Denied | BridgeError::TargetChanged),
                "{err:?}"
            );
            assert_eq!(
                hits(&h),
                after_load,
                "delete={delete}: backend touched after switch"
            );
            assert!(h.backend.store.lock().unwrap().is_empty());
        }
    }

    /// The core race: a target writer must queue behind an in-flight keyring
    /// operation (same gate) instead of committing mid-I/O.
    #[test]
    fn target_writer_cannot_commit_in_the_middle_of_backend_io() {
        for delete in [false, true] {
            let h = harness();
            let req = local_req();
            let lease = h.bridge.load_blocking(&req).unwrap();
            if delete {
                h.bridge
                    .save_blocking(&req, lease.lease_id, &cred("seed"))
                    .unwrap();
            }

            // Backend barrier: announce "inside I/O", wait to be released.
            let (in_io_tx, in_io_rx) = mpsc::channel::<()>();
            let (release_tx, release_rx) = mpsc::channel::<()>();
            let release_rx = Mutex::new(release_rx);
            let in_io_tx = Mutex::new(in_io_tx);
            *h.backend.during_io.lock().unwrap() = Some(Box::new(move || {
                in_io_tx.lock().unwrap().send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }));

            let committed = Arc::new(AtomicBool::new(false));
            let (about_tx, about_rx) = mpsc::channel::<()>();
            let outcome = std::thread::scope(|s| {
                let op = s.spawn(|| {
                    if delete {
                        h.bridge.delete_blocking(&req, lease.lease_id).map(|_| ())
                    } else {
                        h.bridge.save_blocking(&req, lease.lease_id, &cred("t"))
                    }
                });
                in_io_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("backend entered");

                let writer = s.spawn(|| {
                    about_tx.send(()).unwrap();
                    let t = h.target.clone();
                    let committed = committed.clone();
                    h.bridge.commit_target_change(move || {
                        committed.store(true, Ordering::SeqCst);
                        *t.lock().unwrap() = remote_target();
                    });
                });
                about_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                std::thread::sleep(Duration::from_millis(100));
                assert!(
                    !committed.load(Ordering::SeqCst),
                    "switch committed while backend I/O was in flight"
                );
                assert_eq!(*h.target.lock().unwrap(), ConnectionTarget::Local);

                release_tx.send(()).unwrap();
                let outcome = op.join().unwrap();
                writer.join().unwrap();
                outcome
            });

            // The operation linearised BEFORE the switch: it succeeded, its
            // effect is observable, and only then did the switch commit.
            assert_eq!(outcome, Ok(()), "delete={delete}");
            assert!(committed.load(Ordering::SeqCst));
            let stored = h.backend.store.lock().unwrap().len();
            assert_eq!(
                stored,
                usize::from(!delete),
                "delete={delete}: backend effect"
            );
            // ...and afterwards the old lease is dead before any further I/O.
            *h.backend.during_io.lock().unwrap() = None;
            let before = hits(&h);
            assert!(h
                .bridge
                .save_blocking(&req, lease.lease_id, &cred("x"))
                .is_err());
            assert!(h.bridge.delete_blocking(&req, lease.lease_id).is_err());
            assert_eq!(hits(&h), before);
        }
    }

    #[test]
    fn epoch_overflow_fails_closed_before_any_io() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        h.bridge.gate.lock().unwrap().epoch = u64::MAX;
        // The writer still performs the write (a failure here must not block
        // the operator's target switch)...
        let wrote = h.bridge.commit_target_change(|| true);
        assert!(wrote);
        assert!(h.bridge.gate.lock().unwrap().exhausted);
        // ...but the bridge is dead: every operation is refused pre-backend.
        let before = hits(&h);
        assert_eq!(
            h.bridge.load_blocking(&req),
            Err(BridgeError::LeaseExhausted)
        );
        assert_eq!(
            h.bridge.save_blocking(&req, lease.lease_id, &cred("t")),
            Err(BridgeError::LeaseExhausted)
        );
        assert_eq!(
            h.bridge.delete_blocking(&req, lease.lease_id),
            Err(BridgeError::LeaseExhausted)
        );
        assert_eq!(hits(&h), before);
        // And it stays dead.
        h.bridge.commit_target_change(|| ());
        assert_eq!(
            h.bridge.load_blocking(&req),
            Err(BridgeError::LeaseExhausted)
        );
    }

    #[test]
    fn poisoned_gate_never_blocks_the_target_write_and_fails_the_bridge_closed() {
        let h = harness();
        let gate = h.bridge.gate.clone();
        let _ = std::thread::spawn(move || {
            let _g = gate.lock().unwrap();
            panic!("keyring worker panicked");
        })
        .join();
        assert!(h.bridge.commit_target_change(|| 7) == 7);
        assert_eq!(
            h.bridge.load_blocking(&local_req()),
            Err(BridgeError::Internal)
        );
        assert_eq!(hits(&h), 0);
    }

    #[test]
    fn external_native_mutation_during_io_discards_the_result() {
        // NativeConfig edited behind our back (not via save_target) while the
        // backend runs: the post-I/O native re-read still fails closed.
        let h = harness();
        let t = h.target.clone();
        *h.backend.during_io.lock().unwrap() = Some(Box::new(move || {
            *t.lock().unwrap() = remote_target();
        }));
        let req = local_req();
        assert_eq!(
            h.bridge.load_blocking(&req),
            Err(BridgeError::TargetChanged)
        );
        *h.backend.during_io.lock().unwrap() = None;
        *h.target.lock().unwrap() = ConnectionTarget::Local;
        assert_eq!(
            h.bridge.save_blocking(&req, 1, &cred("t")),
            Err(BridgeError::NoLease)
        );
    }

    #[test]
    fn recheck_passes_for_the_live_lease() {
        let h = harness();
        let lease = h.bridge.load_blocking(&local_req()).unwrap();
        assert_eq!(
            h.bridge.recheck_request(Ok(local_req()), lease.lease_id),
            Ok(())
        );
    }

    #[test]
    fn recheck_after_a_to_b_to_a_rejects_the_old_load_result() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        let t = h.target.clone();
        h.bridge
            .commit_target_change(|| *t.lock().unwrap() = remote_target());
        h.bridge
            .commit_target_change(|| *t.lock().unwrap() = ConnectionTarget::Local);
        // Same native target as at load time, yet the epoch moved.
        assert_eq!(
            h.bridge.recheck_request(Ok(local_req()), lease.lease_id),
            Err(BridgeError::TargetChanged)
        );
        assert!(h.bridge.gate.lock().unwrap().active.is_none());
    }

    #[test]
    fn recheck_rejects_a_superseded_lease_and_keeps_the_newer_one() {
        let h = harness();
        let req = local_req();
        let old = h.bridge.load_blocking(&req).unwrap().lease_id;
        let new = h.bridge.load_blocking(&req).unwrap().lease_id;
        assert_eq!(
            h.bridge.recheck_request(Ok(local_req()), old),
            Err(BridgeError::StaleLease)
        );
        // The old reply's own denial must not clear lease 2 either.
        assert_eq!(
            h.bridge.recheck_request(Err(BridgeError::Denied), old),
            Err(BridgeError::Denied)
        );
        assert_eq!(
            h.bridge.gate.lock().unwrap().active.as_ref().map(|l| l.id),
            Some(new)
        );
        h.bridge.save_blocking(&req, new, &cred("t2")).unwrap();
        assert_eq!(h.backend.store.lock().unwrap().len(), 1);
    }

    #[test]
    fn recheck_denied_post_request_clears_only_its_own_lease() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        // Webview navigated away while the worker ran.
        let mut away = local_req();
        away.url = Url::parse("http://evil.example/").unwrap();
        assert_eq!(
            h.bridge.recheck_request(Ok(away), lease.lease_id),
            Err(BridgeError::Denied)
        );
        assert_eq!(
            h.bridge.save_blocking(&req, lease.lease_id, &cred("t")),
            Err(BridgeError::NoLease)
        );
    }

    /// An `Err` after backend I/O means "result withheld", not "no effect":
    /// the store write is already committed and is NOT rolled back.
    #[test]
    fn post_io_target_change_error_does_not_undo_the_committed_save() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        let t = h.target.clone();
        *h.backend.during_io.lock().unwrap() = Some(Box::new(move || {
            *t.lock().unwrap() = remote_target();
        }));
        assert_eq!(
            h.bridge.save_blocking(&req, lease.lease_id, &cred("t")),
            Err(BridgeError::TargetChanged)
        );
        assert_eq!(h.backend.store.lock().unwrap().len(), 1, "effect committed");
    }

    #[test]
    fn denied_load_still_revokes_prior_lease() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        let mut bad = local_req();
        bad.allows.clear();
        assert_eq!(h.bridge.load_blocking(&bad), Err(BridgeError::Denied));
        assert_eq!(
            h.bridge.save_blocking(&req, lease.lease_id, &cred("t")),
            Err(BridgeError::NoLease)
        );
    }

    #[test]
    fn backend_failure_is_fixed_and_secret_free() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        *h.backend.fail.lock().unwrap() = true;
        let err = h
            .bridge
            .save_blocking(&req, lease.lease_id, &cred("super-secret"))
            .unwrap_err();
        assert_eq!(err, BridgeError::Storage);
        let wire = serde_json::to_string(&err).unwrap();
        assert_eq!(wire, "\"storage\"");
        assert!(!format!("{err}{err:?}{wire}").contains("super-secret"));
    }

    #[test]
    fn blank_credential_is_invalid_before_any_io() {
        let h = harness();
        let req = local_req();
        let lease = h.bridge.load_blocking(&req).unwrap();
        let blank = DeviceConnectCredential {
            device_id: "d".into(),
            device_token: " ".into(),
        };
        assert_eq!(
            h.bridge.save_blocking(&req, lease.lease_id, &blank),
            Err(BridgeError::Invalid)
        );
    }

    #[test]
    fn lease_id_overflow_fails_closed() {
        let h = harness();
        h.bridge.gate.lock().unwrap().next_lease_id = u64::MAX;
        assert_eq!(
            h.bridge.load_blocking(&local_req()),
            Err(BridgeError::LeaseExhausted)
        );
        assert!(h.bridge.gate.lock().unwrap().active.is_none());
    }

    const LEGACY_COMMANDS: [&str; 9] = [
        "get_connection_target",
        "set_connection_target",
        "clear_connection_target",
        "is_lite_shell",
        "discover_servers",
        "connect_to",
        "get_pending_cert",
        "approve_cert",
        "reject_cert",
    ];

    fn auto_id(cmd: &str) -> String {
        format!("allow-{}", cmd.replace('_', "-"))
    }

    /// `build.rs` (AppManifest), `permissions/keyring.toml`, the two
    /// capabilities and `main.rs` are five hand-kept copies of one fact; pin
    /// them to each other so drift fails a test instead of the ACL.
    #[test]
    fn manifest_permissions_and_capabilities_agree() {
        let toml = include_str!("../permissions/keyring.toml");
        let build = include_str!("../build.rs");
        for (perm, cmd) in PERMISSIONS.iter().zip(HANDLER_NAMES) {
            assert!(toml.contains(&format!("identifier = \"{perm}\"")), "{perm}");
            assert!(
                toml.contains(&format!("commands.allow = [\"{cmd}\"]")),
                "{cmd}"
            );
            // Must not collide with the AppManifest auto-generated id.
            assert_ne!(*perm, auto_id(cmd));
            assert!(!toml.contains(&format!("identifier = \"{}\"", auto_id(cmd))));
        }
        for cmd in LEGACY_COMMANDS.iter().copied().chain(HANDLER_NAMES) {
            assert!(
                build.contains(&format!("\"{cmd}\"")),
                "build.rs manifest misses {cmd}"
            );
        }

        // Legacy capability: local-only, every window, exactly the nine ids.
        let cap: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/legacy-local-ipc.json")).unwrap();
        assert_eq!(cap["local"], true);
        assert!(
            cap.get("remote").is_none(),
            "legacy capability must stay local-only"
        );
        assert_eq!(cap["windows"], serde_json::json!(["*"]));
        let mut granted: Vec<String> = cap["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect();
        let mut want: Vec<String> = LEGACY_COMMANDS.iter().map(|c| auto_id(c)).collect();
        granted.sort();
        want.sort();
        assert_eq!(granted, want);

        // default.json stays Core-only: no custom-command grant, no widening.
        let default: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(
            default["remote"]["urls"],
            serde_json::json!(["http://127.0.0.1:18790/*"])
        );
        for p in default["permissions"].as_array().unwrap() {
            let p = p.as_str().unwrap();
            assert!(
                p.starts_with("core:") || p.starts_with("autostart:"),
                "default.json grants custom/other permission {p}"
            );
        }

        // Both capability files are actually loaded (tauri.conf lists them).
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            conf["app"]["security"]["capabilities"],
            serde_json::json!(["default", "legacy-local-ipc"])
        );

        // Both generate_handler! arms register the three new commands.
        let main = include_str!("main.rs");
        for cmd in HANDLER_NAMES {
            assert_eq!(
                main.matches(&format!("keyring_bridge::{cmd},")).count(),
                2,
                "{cmd}"
            );
        }
    }

    #[test]
    fn scope_rejects_unknown_fields() {
        assert!(serde_json::from_value::<GatewayCredentialScope>(
            serde_json::json!({"gateway_origin": "http://a:1", "account": "x"})
        )
        .is_err());
    }
}
