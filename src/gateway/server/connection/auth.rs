//! Auth phase — connection identity stamping + login wall.
//!
//! Owns the pure functions that resolve *who* a connection is and *what*
//! authority it carries. The actual `connect` handshake still runs inside
//! the dispatch loop in `connection/mod.rs` — these are the verdict
//! builders it calls and the wall predicate every direction (request arm,
//! event-forward arm, metrics gauge) re-derives the answer from. All items
//! are `pub(crate)` so `handler.rs` can re-export them for the test
//! module (`use super::*;`).

/// The guest login wall: may a connection stamped with `role` send `method`?
///
/// The wall is the *guest* wall and nothing else — it separates "this
/// connection presented a credential" from "it did not." Both authorized roles
/// pass every method: `"operator"` (loopback, legacy shared token, or a device
/// bound to an `admin` user) and `"member"` (a device bound to a `member`-role
/// user). The admin/member split for server-global methods is decided further
/// in, at the `process_request` chokepoint (`method_admin.rs`) — teaching this
/// predicate about it would put the same decision in two places.
///
/// Anything else — `"guest"`, an unrecognized role string, or absent
/// connection state — may only send `connect` to authorize (fail closed).
///
/// ## The wall has TWO consumers, and it had one for too long
///
/// A connection has two directions, and until 2026-08-08 this predicate was
/// evaluated on only one of them. The request arm consults it before dispatch;
/// the *event-forward* arm's verdict was `scope_allowed && audience_allows &&
/// should_receive && event_admits` — four terms, none of them authentication.
/// Every one of them passes for a connection that has never authorized:
/// `ConnectionState` is inserted into `ctx.connections` the moment the socket is
/// accepted (`permissions: []`, `caller_user: None`, `caller_role: "guest"`),
/// `can_receive` allows any topic no rule names, `should_receive` returns `true`
/// when the connection registered no filter at all, and `event_admits` short-
/// circuits on `SessionIdentity::Global` *before* it reads `caller_user`. A bare
/// remote WebSocket that sent nothing therefore received every `Global` frame —
/// including `pty.screen`, whose RPC face has been in `ADMIN_PREFIXES` all along.
///
/// So: **an authorization predicate belongs on every direction a connection
/// carries data, not on the one where the caller asks a question.** The event
/// arm now evaluates this same function on the same `caller_role` field, which
/// also means `restamp_live_connections` closes both planes at once — a
/// deactivated user's socket stops receiving in the same instant it stops being
/// served.
///
/// Pure so the wall's own logic is host-testable. The
/// `resolve_stamped_identity` tests below cover *what role gets stamped*; the
/// class of bug this function exists to prevent lives in the *predicate* — a
/// correctly-stamped `"member"` being refused every method and then
/// flood-guard-kicked as an abuser stays green under any test that scopes
/// task-locals below the wall.
///
/// A third consumer reads it with `method: ""`:
/// `metrics_endpoint::count_authenticated`. "Does this connection hold
/// authority" must have ONE derivation in the gateway, so the gauge moves when
/// `restamp_live_connections` demotes someone, exactly as the two delivery
/// planes do.
#[must_use]
pub fn wall_admits(role: Option<&str>, method: &str) -> bool {
    matches!(role, Some("operator" | "member")) || method == "connect"
}

/// The authorization verdict echoed back in a `connect` response:
/// `(role, authorized, needs_token)`.
///
/// Derived from the **resolved** identity, never from the raw credential
/// verdict alone. "Was the credential valid" and "does this connection hold
/// any authority" are different questions, and P0 made them come apart: a
/// device token that is still valid but whose bound user was deactivated (or
/// whose `user_id` dangles) is a valid credential that grants nothing. It must
/// be reported with the shape the Panel already knows — `("guest", false,
/// true)`, i.e. the login wall — rather than a new close reason or verdict
/// word no client parses.
///
/// Pure so the exact wire triple is host-testable; the surrounding JSON
/// insertion has no seam (it edits a response inside the live WS loop).
#[must_use]
pub fn connect_verdict(credential_ok: bool, resolved_role: &str) -> (&str, bool, bool) {
    let holds_authority = credential_ok && resolved_role != "guest";
    (resolved_role, holds_authority, !holds_authority)
}

/// Resolve the `(caller_role, caller_user)` pair stamped onto
/// `ConnectionState` at a `connect` handshake, given the authorization
/// verdict `resolve_connect_auth` already decided. Pure — host-testable
/// without a live WS socket, unlike the handshake it's extracted from.
///
/// `authorized == false` stays walled (guest, no user) exactly as before
/// per-user resolution existed. `authorized == true` resolves the bound
/// device's user via [`resolve_connection_identity`](crate::gateway::handlers::connect::resolve_connection_identity)
/// when a security store is available — loopback and legacy unbound-device
/// paths still resolve to the implicit owner as operator (zero-change
/// guarantee), but a device bound to a deactivated user is walled here even
/// though its token was valid.
///
/// With **no store wired** (probe/test server) the arm splits on whether the
/// connection is device-bound. No device and no store is the pre-P0 shape
/// (loopback / legacy shared token) and keeps resolving to the implicit owner
/// as operator — unchanged from before per-user resolution existed. A device
/// *is* presented but there is no store to resolve it against ⇒ `("guest",
/// None)`, fail-closed, mirroring the ruled `Err` semantics inside
/// `resolve_connection_identity`: a binding lookup that could not be
/// performed must never be read as "unbound, therefore owner" — that is the
/// one input a remote caller controls, and it would otherwise buy full
/// operator authority on any deployment whose store failed to wire.
pub fn resolve_stamped_identity(
    authorized: bool,
    is_loopback: bool,
    device_id: Option<&str>,
    store: Option<&crate::gateway::security::store::SecurityStore>,
) -> (Option<String>, &'static str) {
    if !authorized {
        return (None, "guest");
    }
    match store {
        Some(store) => crate::gateway::handlers::connect::resolve_connection_identity(
            is_loopback,
            device_id,
            store,
        ),
        // Device-bound but unresolvable: fail closed (see doc above).
        None if device_id.is_some() => (None, "guest"),
        None => (
            Some(crate::gateway::security::store::OWNER_USER_ID.to_string()),
            "operator",
        ),
    }
}

/// What a cluster node claims about itself in its `connect` frame.
pub struct NodeConnectClaim {
    /// The node's persisted `node_id`, or `None` on its very first boot (it has
    /// nothing to present yet — `cluster::admit_node` hands one back).
    pub presented_id: Option<String>,
    pub device_name: String,
}

/// LAN-trust cluster-node detection at `connect` time.
///
/// Token roles are gone, so a cluster node announces itself by request shape:
/// the node client (`aleph-server node`) always sends `commands` + `tags` in its
/// connect params, which no other client does. Returns `None` for every non-node
/// connect. Unlike the old `node_connect_identity`, this does NOT invent an id
/// from `device_name`/`conn_id` — identity is resolved against the device store
/// by [`crate::cluster::admit_node`], so a node's id is stable across reconnects
/// and a revoked node can be told apart from a brand-new one.
pub fn node_connect_claim(params: Option<&serde_json::Value>) -> Option<NodeConnectClaim> {
    let p = params?;
    if p.get("commands").is_none() && p.get("tags").is_none() {
        return None;
    }
    Some(NodeConnectClaim {
        presented_id: p
            .get("device_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
        device_name: p
            .get("device_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
    })
}