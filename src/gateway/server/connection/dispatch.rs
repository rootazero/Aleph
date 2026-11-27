//! Dispatch phase — JSON-RPC request parsing + middleware invocation.
//!
//! The ordinary executor revalidates a live peer here before using a handler
//! or returning a cached result. `dispatch_snapshot` then installs caller
//! scope and delegates through the existing middleware chain.

use std::{collections::HashMap, sync::Arc};

use aleph_protocol::panel_voice::{
    is_native_capture_method, VOICE_TURN_CANCEL_METHOD, VOICE_TURN_SYNTHESIZE_METHOD,
    VOICE_TURN_TRANSCRIBE_METHOD,
};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::gateway::handlers::voice_turn::{VoiceIngress, VoiceRequestContext, VoiceTurnScopes};

use crate::gateway::middleware::MiddlewareChain;
use crate::gateway::panel_project_context::{PanelCallerContext, PanelRequestContext};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, ADMIN_REQUIRED_MESSAGE, AUTH_REQUIRED, PARSE_ERROR,
    PERMISSION_DENIED,
};
use crate::gateway::server::ConnectionState;

/// Scope `process_request` with the caller-identity task-locals + P1 scope
/// attribution that must surround every dispatched request. Single source of
/// truth shared by ordinary dispatch and compatibility callers. See
/// `src/gateway/CLAUDE.md` for the required caller scope around
/// `process_request`. `scope::with_scope` is the outermost
/// (4th) layer: a `caller_user` seeds a personal-scope attribution,
/// observable via `scope::current_scope` for the lifetime of this dispatch
/// (spec P1 §5).
///
/// `caller_verified_user` is the handshake-proved user (see
/// `CALLER_VERIFIED_USER`); `None` for every connection that did not prove one.
pub async fn dispatch_with_caller_context(
    text: &str,
    mc: &MiddlewareChain,
    caller_role: Option<String>,
    caller_user: Option<String>,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
    caller_verified_user: Option<String>,
) -> String {
    dispatch_snapshot(
        text,
        mc,
        CallerSnapshot {
            role: caller_role,
            user: caller_user,
            verified_user: caller_verified_user,
            device_id: None,
        },
        caller_is_loopback,
        caller_conn_id,
        None,
    )
    .await
}

async fn dispatch_snapshot(
    text: &str,
    mc: &MiddlewareChain,
    snapshot: CallerSnapshot,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
    voice_context: Option<VoiceRequestContext>,
) -> String {
    let context = mc.panel_context(PanelCallerContext::authenticated(
        viewer_principal(&snapshot),
        snapshot.device_id.clone(),
    ));
    crate::gateway::caller_identity::with_verified_caller_identity(
        snapshot.role.clone(),
        snapshot.user.clone(),
        caller_is_loopback,
        caller_conn_id,
        snapshot.verified_user.clone(),
        process_request_with_context(text, mc, context, voice_context, Some(&snapshot)),
    )
    .await
}

/// One consistent read of a connection's per-request identity (role, user and
/// handshake-verified user all come from the SAME `ConnectionState` read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerSnapshot {
    pub role: Option<String>,
    pub user: Option<String>,
    pub verified_user: Option<String>,
    pub device_id: Option<String>,
}

impl CallerSnapshot {
    fn from_state(state: &ConnectionState) -> Self {
        Self {
            role: Some(state.caller_role.clone()),
            user: state.caller_user.clone(),
            verified_user: state.verified_user.clone(),
            device_id: state.device_id.clone(),
        }
    }

    /// Read the live connection state. A missing entry (pre-handshake, probe
    /// or already torn down) defaults by network position exactly as the old
    /// per-field reads did -- loopback operator/owner, remote guest -- and
    /// NEVER carries a verified user (no fallback).
    pub async fn read(
        connections: &RwLock<HashMap<String, ConnectionState>>,
        conn_id: &str,
        client_is_local: bool,
    ) -> Self {
        let conns = connections.read().await;
        match conns.get(conn_id) {
            Some(state) => Self::from_state(state),
            None => Self {
                role: Some(if client_is_local { "operator" } else { "guest" }.to_string()),
                user: client_is_local
                    .then(|| crate::gateway::security::store::OWNER_USER_ID.to_string()),
                verified_user: None,
                device_id: None,
            },
        }
    }
}

type ConnectionTable = RwLock<HashMap<String, ConnectionState>>;

/// Process-local origin of a live paired Panel request. Not a permission grant:
/// final effect authorization must hold the authority guards through delivery.
#[derive(Clone)]
pub(crate) struct PairedShellOrigin {
    connections: Arc<ConnectionTable>,
    conn_id: String,
    store: Arc<crate::gateway::security::SecurityStore>,
    token_id: String,
    device_id: String,
    user_id: String,
    role: crate::gateway::security::store::UserRole,
}

#[cfg(target_os = "linux")]
pub(crate) struct PairedShellGuard<'a> {
    // Drop the database reservation before releasing the connection guard.
    account: crate::gateway::security::store::ShellAccountGuard<'a>,
    _connection: tokio::sync::RwLockReadGuard<'a, HashMap<String, ConnectionState>>,
}

#[cfg(target_os = "linux")]
impl PairedShellGuard<'_> {
    pub(crate) fn role_at_effect(&self) -> Option<crate::gateway::security::store::UserRole> {
        self.account.role_at_effect()
    }

    pub(crate) fn bound_attempt(
        &self,
        attempt: crate::sandbox::shell_input::ShellPipeAttempt,
    ) -> Result<
        crate::sandbox::shell_input::ShellPipeAttempt,
        crate::sandbox::shell_input::ShellEffectDenied,
    > {
        let expiry = self
            .account
            .expiry()
            .ok_or(crate::sandbox::shell_input::ShellEffectDenied::Expired)?;
        Ok(attempt.with_account_expiry(expiry))
    }
}

impl PairedShellOrigin {
    fn matches_live_connection(&self, state: &ConnectionState) -> bool {
        !state.first_message
            && state.caller_role == self.role.wire_role()
            && state.verified_user.as_deref() == Some(&self.user_id)
            && state.caller_user.as_deref() == Some(&self.user_id)
            && state.device_id.as_deref() == Some(&self.device_id)
            && state.verified_device_token_id.as_deref() == Some(&self.token_id)
    }

    #[cfg(test)]
    pub(crate) async fn still_connected(&self) -> bool {
        let table = self.connections.read().await;
        let Some(state) = table.get(&self.conn_id) else {
            return false;
        };
        self.matches_live_connection(state)
            && matches!(
                self.store.active_device_principal(&self.token_id, &self.device_id),
                Ok(Some((user, role))) if user == self.user_id && role.wire_role() == state.caller_role
            )
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn same_connection(&self, current: &Self) -> bool {
        Arc::ptr_eq(&self.connections, &current.connections)
            && Arc::ptr_eq(&self.store, &current.store)
            && self.conn_id == current.conn_id
            && self.token_id == current.token_id
            && self.device_id == current.device_id
            && self.user_id == current.user_id
            && self.role == current.role
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn user_id(&self) -> &str {
        &self.user_id
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn try_effect_guard(
        &self,
    ) -> Result<PairedShellGuard<'_>, crate::sandbox::shell_input::ShellEffectDenied> {
        use crate::sandbox::shell_input::ShellEffectDenied;

        let connection = self
            .connections
            .try_read()
            .map_err(|_| ShellEffectDenied::Busy)?;
        let state = connection
            .get(&self.conn_id)
            .ok_or(ShellEffectDenied::Stale)?;
        if !self.matches_live_connection(state) {
            return Err(ShellEffectDenied::Stale);
        }
        // Device revocation already orders its two authorities this way.
        let account = self
            .store
            .try_shell_account_guard(&self.token_id, &self.device_id, &self.user_id, self.role)
            .map_err(|error| match error.sqlite_error_code() {
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                    ShellEffectDenied::Busy
                }
                _ => ShellEffectDenied::Stale,
            })?
            .ok_or(ShellEffectDenied::Stale)?;
        Ok(PairedShellGuard {
            account,
            _connection: connection,
        })
    }
}

tokio::task_local! {
    pub(crate) static PAIRED_SHELL_ORIGIN: Option<PairedShellOrigin>;
}

pub(crate) fn current_paired_shell_origin() -> Option<PairedShellOrigin> {
    PAIRED_SHELL_ORIGIN.try_with(Clone::clone).ok().flatten()
}

#[derive(Clone)]
pub(super) struct DispatchPeer {
    pub(super) connections: Arc<ConnectionTable>,
    pub(super) conn_id: String,
    legacy_loopback: bool,
    bound: Option<BoundPeer>,
    paired_panel_store: Option<Arc<crate::gateway::security::SecurityStore>>,
}

#[derive(Clone)]
struct BoundPeer {
    expected: CallerSnapshot,
    authority: BoundAuthority,
    cancellation: CancellationToken,
    live_events: bool,
    /// This connection's whole-turn scopes; shared by every request it sends.
    turns: Arc<VoiceTurnScopes>,
}

#[derive(Clone)]
enum BoundAuthority {
    Web(super::web_v1::WebBinding),
    #[cfg(any(unix, windows))]
    Desktop(Arc<crate::gateway::security::SecurityStore>),
    #[cfg(test)]
    Fixture,
}

#[cfg(any(unix, windows))]
pub(super) fn active_desktop_owner(
    store: &crate::gateway::security::SecurityStore,
) -> Option<&'static str> {
    use crate::gateway::security::store::{UserStatus, OWNER_USER_ID};
    let owner = store.get_user(OWNER_USER_ID).ok()??;
    (owner.status == UserStatus::Active).then(|| owner.role.wire_role())
}

impl DispatchPeer {
    pub(super) fn legacy(
        connections: Arc<ConnectionTable>,
        conn_id: String,
        legacy_loopback: bool,
    ) -> Self {
        Self {
            connections,
            conn_id,
            legacy_loopback,
            bound: None,
            paired_panel_store: None,
        }
    }

    pub(super) fn panel(
        connections: Arc<ConnectionTable>,
        conn_id: String,
        legacy_loopback: bool,
        store: Option<Arc<crate::gateway::security::SecurityStore>>,
    ) -> Self {
        Self {
            paired_panel_store: store,
            ..Self::legacy(connections, conn_id, legacy_loopback)
        }
    }

    async fn paired_shell_origin(&self, start: &CallerSnapshot) -> Option<PairedShellOrigin> {
        let store = self.paired_panel_store.clone()?;
        let table = self.connections.read().await;
        let state = table.get(&self.conn_id)?;
        let user_id = state.verified_user.as_ref()?;
        use crate::gateway::security::store::UserRole;
        let role = match state.caller_role.as_str() {
            "operator" => UserRole::Admin,
            "member" => UserRole::Member,
            _ => return None,
        };
        if state.first_message
            || state.caller_user.as_ref() != Some(user_id)
            || &CallerSnapshot::from_state(state) != start
        {
            return None;
        }
        Some(PairedShellOrigin {
            connections: self.connections.clone(),
            conn_id: self.conn_id.clone(),
            store,
            token_id: state.verified_device_token_id.clone()?,
            device_id: state.device_id.clone()?,
            user_id: user_id.clone(),
            role,
        })
    }

    pub(super) fn web(
        connections: Arc<ConnectionTable>,
        conn_id: String,
        expected: CallerSnapshot,
        binding: super::web_v1::WebBinding,
        cancellation: CancellationToken,
        live_events: bool,
    ) -> Self {
        Self {
            bound: Some(BoundPeer {
                expected,
                authority: BoundAuthority::Web(binding),
                cancellation,
                live_events,
                turns: Default::default(),
            }),
            ..Self::legacy(connections, conn_id, false)
        }
    }

    #[cfg(any(unix, windows))]
    pub(super) fn desktop(
        connections: Arc<ConnectionTable>,
        conn_id: String,
        owner_store: Arc<crate::gateway::security::SecurityStore>,
        cancellation: CancellationToken,
        live_events: bool,
    ) -> Self {
        Self {
            bound: Some(BoundPeer {
                expected: CallerSnapshot {
                    role: None,
                    user: Some(crate::gateway::security::store::OWNER_USER_ID.to_owned()),
                    verified_user: None,
                    device_id: None,
                },
                authority: BoundAuthority::Desktop(owner_store),
                cancellation,
                live_events,
                turns: Default::default(),
            }),
            ..Self::legacy(connections, conn_id, false)
        }
    }

    // Test fixture for a generic established peer, not an IPC admission path.
    #[cfg(test)]
    pub(super) fn bound(
        connections: Arc<ConnectionTable>,
        conn_id: String,
        expected: CallerSnapshot,
    ) -> Self {
        Self {
            bound: Some(BoundPeer {
                expected,
                authority: BoundAuthority::Fixture,
                cancellation: CancellationToken::new(),
                live_events: false,
                turns: Default::default(),
            }),
            ..Self::legacy(connections, conn_id, false)
        }
    }

    pub(super) async fn snapshot(&self) -> Option<CallerSnapshot> {
        self.view().snapshot().await
    }

    pub(super) fn is_voice_bound(&self) -> bool {
        let authority = self.bound.as_ref().map(|bound| &bound.authority);
        #[cfg(any(unix, windows))]
        if matches!(authority, Some(BoundAuthority::Desktop(_))) {
            return true;
        }
        matches!(authority, Some(BoundAuthority::Web(_)))
    }

    pub(super) fn is_desktop_voice_bound(&self) -> bool {
        #[cfg(any(unix, windows))]
        {
            self.bound
                .as_ref()
                .is_some_and(|bound| matches!(&bound.authority, BoundAuthority::Desktop(_)))
        }
        #[cfg(not(any(unix, windows)))]
        false
    }

    #[cfg(windows)]
    pub(super) fn has_offline_voice(&self) -> bool {
        self.bound
            .as_ref()
            .is_some_and(|peer| peer.turns.has_offline())
    }

    #[cfg(any(unix, windows))]
    pub(super) async fn drain_desktop_voice(&self) {
        if let Some(bound) = &self.bound {
            bound.cancellation.cancel();
            bound.turns.drain_offline().await;
        }
    }

    fn view(&self) -> PeerView<'_> {
        PeerView {
            connections: &self.connections,
            conn_id: &self.conn_id,
            legacy_loopback: self.legacy_loopback,
            bound: self.bound.as_ref(),
        }
    }
}

struct PeerView<'a> {
    connections: &'a ConnectionTable,
    conn_id: &'a str,
    legacy_loopback: bool,
    bound: Option<&'a BoundPeer>,
}

impl<'a> PeerView<'a> {
    fn legacy(connections: &'a ConnectionTable, conn_id: &'a str, legacy_loopback: bool) -> Self {
        Self {
            connections,
            conn_id,
            legacy_loopback,
            bound: None,
        }
    }

    async fn snapshot(&self) -> Option<CallerSnapshot> {
        if let Some(bound) = self.bound {
            let live = {
                let table = self.connections.read().await;
                let state = table.get(self.conn_id)?;
                if state.first_message
                    || !matches!(state.caller_role.as_str(), "member" | "operator")
                {
                    return None;
                }
                CallerSnapshot::from_state(state)
            };
            // Role remains live for the existing admin gate. The bound user,
            // verified user and device cannot drift within a connection.
            if !live
                .user
                .as_deref()
                .is_some_and(|user| !user.trim().is_empty())
                || live.user != bound.expected.user
                || live.verified_user != bound.expected.verified_user
                || live.device_id != bound.expected.device_id
            {
                return None;
            }
            let role = match &bound.authority {
                BoundAuthority::Web(binding) => binding.current_role()?,
                #[cfg(any(unix, windows))]
                BoundAuthority::Desktop(store) => active_desktop_owner(store)?,
                #[cfg(test)]
                BoundAuthority::Fixture => return Some(live),
            };
            return Some(CallerSnapshot {
                role: Some(role.to_owned()),
                ..live
            });
        }
        Some(CallerSnapshot::read(self.connections, self.conn_id, self.legacy_loopback).await)
    }

    // Caller must already have passed `validate_live`. Neither loopback nor a
    // Panel project selector establishes the transport of the invocation.
    fn voice_context_after_validation(&self, method: &str) -> Option<VoiceRequestContext> {
        let bound = self.bound?;
        let voice_snapshot = method == aleph_protocol::panel::PANEL_UI_CAPABILITY_SNAPSHOT_METHOD
            && bound.live_events;
        if !voice_snapshot
            && !matches!(
                method,
                VOICE_TURN_TRANSCRIBE_METHOD
                    | VOICE_TURN_SYNTHESIZE_METHOD
                    | VOICE_TURN_CANCEL_METHOD
            )
            && !is_native_capture_method(method)
        {
            return None;
        }
        let (ingress, budget) = match &bound.authority {
            BoundAuthority::Web(_) => (VoiceIngress::WebV1, super::web_v1::MAX_MESSAGE),
            #[cfg(any(unix, windows))]
            BoundAuthority::Desktop(_) => (
                VoiceIngress::DesktopIpc,
                aleph_client::local_ipc::MAX_FRAME_BYTES,
            ),
            #[cfg(test)]
            BoundAuthority::Fixture => return None,
        };
        Some(VoiceRequestContext::new(
            ingress,
            self.conn_id.to_owned(),
            budget as usize,
            bound.cancellation.child_token(),
            bound.turns.clone(),
        ))
    }
}

/// RPC methods that mint/revoke/decide/list per-user browser viewer
/// capabilities and resources. They require a handshake-VERIFIED user, not
/// the stamped (possibly implicit-owner) one.
pub const BROWSER_VIEWER_METHODS: [&str; 4] = [
    "browser.viewer.grant.issue",
    "browser.viewer.grant.revoke",
    "browser.viewer.resource.decide",
    "browser.viewer.resource.list",
];

#[must_use]
pub fn is_browser_viewer_method(method: &str) -> bool {
    BROWSER_VIEWER_METHODS.contains(&method)
}

/// The single viewer-authority predicate (pure). `Some(verified)` iff the role
/// is member/operator, the stamped user is non-empty, and the verified user is
/// non-empty and equal to it. Mirrors
/// `caller_identity::current_caller_verified_user`.
#[must_use]
pub fn viewer_principal(snapshot: &CallerSnapshot) -> Option<String> {
    crate::gateway::caller_identity::verified_caller_user(
        snapshot.role.as_deref(),
        snapshot.user.as_deref(),
        snapshot.verified_user.as_deref(),
    )
}

/// Generic denial -- no credential, user or reason echoed
#[must_use]
pub fn viewer_denied_response(id: Option<serde_json::Value>) -> String {
    serde_json::to_string(&JsonRpcResponse::error(id, PERMISSION_DENIED, "denied"))
        .unwrap_or_default()
}
/// Canonical login/admin denial under the currently scoped caller identity.
fn authorization_error(method: &str, id: Option<serde_json::Value>) -> Option<String> {
    let role = crate::gateway::caller_identity::current_caller_role();
    let message = if !super::auth::wall_admits(role.as_deref(), method) {
        super::auth::LOGIN_WALL_MESSAGE
    } else if crate::gateway::method_admin::method_requires_admin(method)
        && crate::gateway::caller_identity::caller_is_member()
    {
        ADMIN_REQUIRED_MESSAGE
    } else {
        return None;
    };
    Some(
        serde_json::to_string(&JsonRpcResponse::error(id, AUTH_REQUIRED, message))
            .unwrap_or_default(),
    )
}

/// Re-validate login, admin authority and identity against LIVE connection state.
/// Used before cached/waiting results return and after a lane permit is granted.
/// Viewer methods additionally require the same handshake-verified principal.
pub async fn revalidate_for_dispatch(
    connections: &RwLock<HashMap<String, ConnectionState>>,
    conn_id: &str,
    client_is_local: bool,
    method: &str,
    start: &CallerSnapshot,
) -> Option<CallerSnapshot> {
    validate_live(
        &PeerView::legacy(connections, conn_id, client_is_local),
        method,
        start,
        None,
    )
    .await
    .ok()
}

pub(super) async fn revalidate_peer(
    peer: &DispatchPeer,
    method: &str,
    start: &CallerSnapshot,
) -> bool {
    validate_live(&peer.view(), method, start, None)
        .await
        .is_ok()
}

async fn validate_live(
    peer: &PeerView<'_>,
    method: &str,
    start: &CallerSnapshot,
    id: Option<serde_json::Value>,
) -> Result<CallerSnapshot, String> {
    let Some(live) = peer.snapshot().await else {
        return Err(viewer_denied_response(id));
    };
    let viewer_method = is_browser_viewer_method(method);
    let denial = crate::gateway::caller_identity::with_caller_identity(
        live.role.clone(),
        live.user.clone(),
        peer.legacy_loopback,
        Some(peer.conn_id.to_owned()),
        async { authorization_error(method, id.clone()) },
    )
    .await;
    if let Some(denial) = denial {
        // Viewer denials deliberately do not reveal wall or admin details.
        return Err(if viewer_method {
            viewer_denied_response(id)
        } else {
            denial
        });
    }
    let same_authority = if crate::gateway::handlers::panel_capabilities::is_context_method(method)
    {
        viewer_principal(&live) == viewer_principal(start) && live.device_id == start.device_id
    } else if viewer_method {
        viewer_principal(start)
            .is_some_and(|expected| viewer_principal(&live).as_deref() == Some(expected.as_str()))
    } else {
        true
    };
    if live.user != start.user || !same_authority {
        return Err(viewer_denied_response(id));
    }
    Ok(live)
}

/// UI effects may wait outside the connection loop. Never use the loopback
/// fallback for a missing/closed connection, nor execute under a changed login.
/// The record and its identity are read under one lock after queue/permit waits.
pub(super) async fn live_ui_caller(
    connections: &RwLock<HashMap<String, ConnectionState>>,
    conn_id: &str,
    start: &CallerSnapshot,
) -> Option<CallerSnapshot> {
    let connections = connections.read().await;
    let state = connections.get(conn_id)?;
    if state.first_message
        || !super::auth::wall_admits(Some(&state.caller_role), "capabilities.ui.invoke")
    {
        return None;
    }
    let live = CallerSnapshot::from_state(state);
    (live.user.as_deref().is_some_and(|user| !user.is_empty()) && &live == start).then_some(live)
}

/// Legacy entry point, including the separately scheduled UI effect path.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_for_connection(
    text: &str,
    mc: &MiddlewareChain,
    connections: &RwLock<HashMap<String, ConnectionState>>,
    conn_id: &str,
    client_is_local: bool,
    method: &str,
    req_id: Option<serde_json::Value>,
    start: &CallerSnapshot,
) -> String {
    if method == "capabilities.ui.invoke" {
        return match live_ui_caller(connections, conn_id, start).await {
            Some(live) => {
                dispatch_snapshot(
                    text,
                    mc,
                    live,
                    client_is_local,
                    Some(conn_id.to_owned()),
                    None,
                )
                .await
            }
            None => viewer_denied_response(req_id),
        };
    }
    dispatch_ordinary(
        text,
        mc,
        &PeerView::legacy(connections, conn_id, client_is_local),
        method,
        req_id,
        start,
        None,
    )
    .await
}

pub(super) async fn dispatch_for_peer(
    text: &str,
    mc: &MiddlewareChain,
    peer: &DispatchPeer,
    req: &JsonRpcRequest,
    start: &CallerSnapshot,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> String {
    let view = peer.view();
    let dispatch = dispatch_ordinary(text, mc, &view, &req.method, req.id.clone(), start, permit);
    if req.method == "chat.send" {
        let origin = peer.paired_shell_origin(start).await;
        return PAIRED_SHELL_ORIGIN.scope(origin, dispatch).await;
    }
    if req.method == "trace.tool_output" && peer.bound.is_some() {
        return crate::gateway::handlers::tool_output::V1_TOOL_OUTPUT
            .scope(true, dispatch)
            .await;
    }
    dispatch.await
}

async fn dispatch_ordinary(
    text: &str,
    mc: &MiddlewareChain,
    peer: &PeerView<'_>,
    method: &str,
    id: Option<serde_json::Value>,
    start: &CallerSnapshot,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> String {
    match validate_live(peer, method, start, id).await {
        Ok(live) => {
            let mut permit = permit;
            let voice_context = peer.voice_context_after_validation(method).map(|context| {
                if method == VOICE_TURN_TRANSCRIBE_METHOD {
                    context.with_query_permit(permit.take())
                } else {
                    context
                }
            });
            dispatch_snapshot(
                text,
                mc,
                live,
                peer.legacy_loopback,
                Some(peer.conn_id.to_owned()),
                voice_context,
            )
            .await
        }
        Err(denial) => denial,
    }
}

/// Idempotency principal. Viewer methods key on the verified user (`v:` prefix,
/// disjoint from any plain user id used by other methods); everything else is
/// unchanged.
#[must_use]
pub fn idempotency_principal(method: &str, snapshot: &CallerSnapshot) -> String {
    if is_browser_viewer_method(method) {
        return match viewer_principal(snapshot) {
            Some(v) => format!("v:{v}"),
            None => "v:<none>".to_string(),
        };
    }
    snapshot.user.as_deref().unwrap_or("<anon>").to_string()
}

/// Process a JSON-RPC request string
pub async fn process_request(text: &str, middleware_chain: &MiddlewareChain) -> String {
    process_request_with_context(text, middleware_chain, None, None, None).await
}

async fn process_request_with_context(
    text: &str,
    middleware_chain: &MiddlewareChain,
    panel_context: Option<PanelRequestContext>,
    mut voice_context: Option<VoiceRequestContext>,
    caller: Option<&CallerSnapshot>,
) -> String {
    // Parse the request
    let request: JsonRpcRequest = match serde_json::from_str(text) {
        Ok(req) => req,
        Err(e) => {
            return serde_json::to_string(&JsonRpcResponse::error(
                None,
                PARSE_ERROR,
                format!("Parse error: {e}"),
            ))
            .unwrap_or_default();
        }
    };

    // Multi-user role gate (spec §4.6): members cannot reach server-global
    // config/credential methods. One chokepoint covers both dispatch paths —
    // CALLER_ROLE is scoped around process_request at both call sites
    // (`do_lane_dispatch` and the idempotency `Proceed` arm). `None`
    // (internal/cron) and `"operator"` pass; `"guest"` never reaches here for
    // non-connect methods (the login wall above refuses it first).
    if crate::gateway::method_admin::method_requires_admin(&request.method)
        && crate::gateway::caller_identity::caller_is_member()
    {
        return serde_json::to_string(&JsonRpcResponse::error(
            request.id.clone(),
            AUTH_REQUIRED,
            // Shared with the Panel through `aleph_protocol` — the wording is
            // not local to this arm, because the cluster page keys its role
            // explanation off these exact words.
            ADMIN_REQUIRED_MESSAGE.to_string(),
        ))
        .unwrap_or_default();
    }

    // Distributed-trace context: honour an inbound W3C `traceparent` (carried
    // in params) or mint a fresh root trace, so every log/span emitted while
    // handling this request is correlatable. See `trace_context` for why this
    // is a lightweight propagation layer rather than a full OTel integration.
    let trace =
        crate::gateway::trace_context::TraceContext::from_request_params(request.params.as_ref());
    let span = tracing::info_span!(
        "rpc",
        trace_id = %trace.trace_id,
        span_id = %trace.span_id,
        method = %request.method,
    );

    const TRACEPARENT_FIELD: &str = "traceparent";
    let trace_header = trace.to_header();
    if let Some(context) = voice_context.as_mut() {
        // Appending one field to the existing object adds the serialized
        // single-field object length minus its braces plus a comma.
        let field = serde_json::json!({ (TRACEPARENT_FIELD): &trace_header });
        context.reserve_response_bytes(field.to_string().len() - 1);
    }

    #[cfg(feature = "native-qa")]
    let request_for_observer = request.clone();
    let response = {
        use tracing::Instrument;
        middleware_chain
            .serve_with_context(request, panel_context, voice_context)
            .instrument(span)
            .await
    };

    #[cfg(not(feature = "native-qa"))]
    let _ = caller;

    #[cfg(feature = "native-qa")]
    if let Some(caller) = caller {
        crate::gateway::native_qa::observe_rpc(
            &middleware_chain.native_qa_observer(),
            caller,
            &request_for_observer,
            &response,
        );
    }

    // Echo the trace context back so the caller / a downstream hop can continue
    // the trace (naming our span as the parent). `traceparent` is a non-standard
    // sibling of the JSON-RPC envelope fields; serde clients ignore unknown
    // fields, so this stays backward-compatible.
    let mut value = serde_json::to_value(&response).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            TRACEPARENT_FIELD.to_owned(),
            serde_json::Value::String(trace_header),
        );
    }
    serde_json::to_string(&value).unwrap_or_default()
}
