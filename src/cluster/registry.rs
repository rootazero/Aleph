//! Cluster node registry (center side).
//!
//! Tracks "which connected WS connections are registered nodes" and projects
//! them into a read-only "environment" view for `environments.list` rendering.
//! Consumes Phase 0a's [`ReverseRpcChannel`] — each `NodeSession` holds a channel
//! clone, and 0c's `node_invoke` dispatches down to the node through it.
//!
//! Redline: pure data structure, no LLM reasoning (R7), does not enter `src/harness/` (R10).

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sync_primitives::RwLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cluster::ReverseRpcChannel;

/// A command declared by a node (name + schema placeholder). The `schema`
/// field is a placeholder for the node's self-declared JSON schema; it is
/// stored verbatim, surfaced in `environments.list` / `node_list`, and
/// preserved across reconnect. **It is NOT validated against `args`** — the
/// allowlist (`CommandTable`) is the only authoritative gate on the
/// node side, and the center passes `args` straight through to the command's
/// `run`. Every shipped node command declares `{"type":"object"}` with no
/// `properties`, so the field is informational today.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandDescriptor {
    pub name: String,
    pub schema: Value,
}

/// A connected node session (center-side view).
pub struct NodeSession {
    /// = `device_id`, used directly as the environment id.
    pub node_id: String,
    /// Matches the key in 0a's `reverse_rpc` table; used for disconnect reconciliation.
    pub conn_id: String,
    /// Human-readable name (from the connect frame).
    pub device_name: String,
    /// Clone of the 0a channel — 0c's `node_invoke` dispatches through it.
    pub channel: ReverseRpcChannel,
    /// The node's self-declared command catalog; 0b only stores and displays it.
    pub declared_commands: Vec<CommandDescriptor>,
    /// Operator-assigned free-text labels (e.g. "gpu", "region=us"). Selection
    /// only — never an authorization gate (R7). Stored verbatim; not kv-parsed.
    pub tags: Vec<String>,
    /// The `aleph-server` build the node runs, as it declared in its connect
    /// frame. `None` = a node older than the version handshake. **Observation
    /// only**: a skewed node is never refused (see [`maybe_register_node`]).
    pub version: Option<String>,
    /// Registration timestamp (Unix seconds).
    pub connected_at: i64,
}

/// Structured result for node address resolution failure (replaces the old
/// `Option`, making ambiguity explicitly visible to callers). Maps openclaw
/// `node-match.ts` multi-level matching, but expressed as a type-safe enum —
/// making "ambiguity" a non-ignorable first-class state rather than a stringly error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// No online node matches this name/id.
    #[error("no online node matches")]
    NotFound,
    /// Multiple online nodes match — includes readable candidate labels
    /// (`name (short-id)`) for LLM disambiguation.
    #[error("ambiguous — matches: {}", .0.join(", "))]
    Ambiguous(Vec<String>),
    /// Internal state inconsistency: the id returned by match_id is missing from nodes_by_id.
    #[error("internal node lookup failed for '{name_or_id}'")]
    NodeNotFound { name_or_id: String },
}

/// Serialized external view for `environments.list` (thin rendering contract,
/// R4). Never contains credentials.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub status: &'static str,
    pub commands: Vec<CommandDescriptor>,
    pub tags: Vec<String>,
    pub connected_at: i64,
    /// Last-seen online timestamp (Unix seconds). Only meaningful for registered
    /// nodes with `status == "offline"` (online nodes are always `None`);
    /// `None` + offline = never connected since registration.
    #[serde(default)]
    pub last_seen_at: Option<i64>,
    /// The node's `aleph-server` build, from its connect frame. Always `None`
    /// for offline entries — the device store keeps no version column, and a
    /// remembered version would be a claim about a machine we cannot currently
    /// see. `None` on an *online* node = it predates the version handshake.
    #[serde(default)]
    pub version: Option<String>,
}

/// A matched online node for tag-selected fan-out: enough to dispatch over
/// reverse RPC and run the same per-node fail-fast check `node_invoke` uses.
/// `tags` is carried so the caller can build a "available tags" hint on a
/// zero-match. Cloneable; holds a `ReverseRpcChannel` clone.
#[derive(Clone, Debug)]
pub struct NodeMatch {
    pub node_id: String,
    pub name: String,
    pub channel: ReverseRpcChannel,
    pub declared_commands: Vec<CommandDescriptor>,
    pub tags: Vec<String>,
}

/// Byte-bounded prefix of `value` that never splits a UTF-8 scalar.
///
/// `&s[..n]` **panics** when byte `n` lands mid-character, and both call sites
/// slice a *node id* — which is not always a center-minted ASCII UUID. The
/// "unknown id" branch of [`crate::cluster::admit_node`] adopts whatever
/// `device_id` the peer presented in its connect frame, so a node dialling in
/// with a non-ASCII identity file would otherwise panic the connection task
/// (P7 UTF-8 safety). Truncating short is always safe here: these are display
/// affordances (ambiguity labels, device fingerprints), not identities.
pub(crate) fn truncate_on_char_boundary(value: &str, max_bytes: usize) -> &str {
    crate::utils::text_format::truncate_bytes(value, max_bytes)
}

/// A registered session plus the parse state of the catalog it declared.
struct RegisteredNode {
    /// The live session (authoritative).
    session: NodeSession,
    /// The node's connect frame carried a `commands` field that could not be
    /// parsed. **Not the same as "declared no commands"**: an empty catalog
    /// means "the node is authoritative" (the center skips its declaration
    /// check), while a malformed one means the center cannot know what the
    /// node permits — so it has to fail closed instead of reading the node as
    /// having declared nothing.
    ///
    /// Held beside the session rather than inside it: `NodeSession` is a public
    /// struct other modules construct directly, while the connect frame's
    /// parse state is known only here.
    catalog_malformed: bool,
}

#[derive(Default)]
struct RegistryInner {
    /// `node_id` → registered session (authoritative).
    nodes_by_id: HashMap<String, RegisteredNode>,
    /// `conn_id` → `node_id` (reverse lookup on disconnect).
    nodes_by_conn: HashMap<String, String>,
}

/// Node registry. Thread-safe; lock poisoning handled per P7
/// (`unwrap_or_else(|e| e.into_inner())`).
#[derive(Default)]
pub struct NodeRegistry {
    inner: RwLock<RegistryInner>,
}

impl NodeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a node session whose declared command catalog is readable (or
    /// absent). See [`register_session`](Self::register_session) for the
    /// variant that records a malformed catalog.
    pub fn register(&self, session: NodeSession) {
        self.register_session(session, false);
    }

    /// Register a node session. Reconnect with the same `node_id` → overwrites
    /// the old session, clears the old conn mapping, **and asks the old session's
    /// connection to close** (B1-01). Reusing the same `conn_id` under a
    /// different `node_id` drops the orphaned session's map entries but closes
    /// nothing (B1-03 — that session's channel is this very connection's).
    ///
    /// **The whole read-modify-write of both maps runs under ONE write guard.**
    /// It used to drop the guard mid-function to fire `close_connection()` and
    /// then re-acquire, which let two connections registering the same `node_id`
    /// concurrently (every WS connection is its own task; a network flap makes a
    /// node redial while its old socket is still live) both read the *same*
    /// previous session and both insert their own `conn_id`. The loser was
    /// neither closed nor overwritten, yet still resolved through
    /// [`node_identity_by_conn`](Self::node_identity_by_conn) — the anti-spoof
    /// stamp on every inbound node frame — under the winner's identity. The same
    /// window also let a concurrent [`forget`](Self::forget) report "evicted" and
    /// then be silently undone by the re-insert.
    ///
    /// Closes are collected and fired *after* the guard drops, the shape
    /// [`forget`](Self::forget) uses: not to avoid a deadlock —
    /// [`ReverseRpcChannel::close_connection`] is a non-blocking
    /// `Notify::notify_one` that touches no registry state — but to keep the
    /// critical section free of wakeup side effects.
    ///
    /// `catalog_malformed` records whether the connect frame's `commands`
    /// field failed to deserialise. The session is registered either way (the
    /// node is online and reachable), but a malformed catalog must not be read
    /// as "this node declares no commands" — that reading silently switches
    /// the center-side fail-fast check off for this node.
    fn register_session(&self, session: NodeSession, catalog_malformed: bool) {
        let node_id = session.node_id.clone();
        let conn_id = session.conn_id.clone();
        let mut to_close: Vec<ReverseRpcChannel> = Vec::new();
        {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            // (B1-03) If this `conn_id` is already mapped to a *different*
            // node_id, that older session is orphaned by the new mapping; drop
            // its entries from both tables.
            //
            // **Its channel is NOT closed**: `nodes_by_conn[conn_id] ==
            // prev_node_id` implies `prev.conn_id == conn_id`, so the orphaned
            // session's channel is *this* connection's channel (the center binds
            // one `with_close` channel per connection and stores clones of it —
            // they share one `Arc<Notify>`). Closing it would tear down the very
            // session the lines below register. Only the stale map entries go.
            if let Some(prev_node_id) = inner.nodes_by_conn.get(&conn_id).cloned() {
                if prev_node_id != node_id {
                    if let Some(prev) = inner.nodes_by_id.remove(&prev_node_id) {
                        inner.nodes_by_conn.remove(&prev.session.conn_id);
                        tracing::info!(
                            old_node_id = %prev_node_id,
                            new_node_id = %node_id,
                            conn_id = %conn_id,
                            "cluster node connection re-announced under a different node_id; \
                             dropping the old session's registry entries (its connection is this one)"
                        );
                    }
                }
            }
            // (B1-01) Reconnect with the same node_id: drop the old session's
            // conn→node mapping AND signal its connection to close. Without the
            // close signal, the dropped session's connection task keeps running,
            // and any `channel.clone()` still alive in another part of the program
            // can still `call()` a session the registry no longer knows about.
            // `remove` rather than `get`: the stale session must not be observable
            // for even an instant of this critical section.
            if let Some(prev) = inner.nodes_by_id.remove(&node_id) {
                if prev.session.conn_id != conn_id {
                    inner.nodes_by_conn.remove(&prev.session.conn_id);
                    tracing::info!(
                        node_id = %node_id,
                        old_conn_id = %prev.session.conn_id,
                        new_conn_id = %conn_id,
                        "cluster node reconnected under a fresh connection; closing old"
                    );
                    to_close.push(prev.session.channel.clone());
                }
            }
            inner.nodes_by_conn.insert(conn_id.clone(), node_id.clone());
            inner
                .nodes_by_id
                .insert(node_id.clone(), RegisteredNode { session, catalog_malformed });
        }
        for channel in to_close {
            channel.close_connection();
        }
        tracing::debug!(node_id = %node_id, conn_id = %conn_id, "cluster node registered");
    }

    /// Deregister a connected node session. Only removes if the current session
    /// for this `node_id` actually belongs to this `conn_id` (reconnect-safe:
    /// old connection cleanup won't accidentally evict the new session). Returns
    /// whether a session was removed.
    pub fn deregister(&self, conn_id: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let Some(node_id) = inner.nodes_by_conn.remove(conn_id) else {
            return false;
        };
        if let std::collections::hash_map::Entry::Occupied(entry) =
            inner.nodes_by_id.entry(node_id.clone())
        {
            if entry.get().session.conn_id == conn_id {
                let removed_name = entry.get().session.device_name.clone();
                entry.remove();
                tracing::debug!(node_id = %node_id, conn_id = %conn_id, name = %removed_name, "cluster node session deregistered");
                return true;
            }
        }
        false
    }

    /// Read-only projection snapshot of online nodes. Results are stably sorted
    /// by `(name, id)` — `nodes_by_id` is a `HashMap` with non-deterministic
    /// iteration order; sorting prevents the Panel fleet list and the
    /// model-visible `node_list` from jittering on every refresh (tests can also
    /// assert a deterministic order).
    pub fn list_environments(&self) -> Vec<Environment> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut envs: Vec<Environment> = inner
            .nodes_by_id
            .values()
            .map(|n| Environment {
                id: n.session.node_id.clone(),
                name: n.session.device_name.clone(),
                status: "online",
                commands: n.session.declared_commands.clone(),
                tags: n.session.tags.clone(),
                connected_at: n.session.connected_at,
                last_seen_at: None,
                version: n.session.version.clone(),
            })
            .collect();
        envs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        envs
    }

    /// Resolve `(node_id, device_name)` for a connection that is a registered
    /// node. Returns `None` for non-node / unregistered connections. The center
    /// uses this to stamp node identity from the AUTHENTICATED connection rather
    /// than trusting request params (anti-spoof).
    pub fn node_identity_by_conn(&self, conn_id: &str) -> Option<(String, String)> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let node_id = inner.nodes_by_conn.get(conn_id)?;
        let n = inner.nodes_by_id.get(node_id)?;
        Some((n.session.node_id.clone(), n.session.device_name.clone()))
    }

    /// Resolve a name/id to a unique `node_id` (multi-level match; the registry
    /// only holds online sessions so there is no "prefer-connected" tie-break —
    /// all candidates are online). Match levels (strong → weak):
    /// ① exact `node_id` (as-is, ids are UUIDs) ② normalized `device_name` equality
    /// ③ fuzzy (id prefix ≥4 OR normalized name substring). Name matching via
    /// [`normalize_node_key`] is case- and punctuation/space-insensitive
    /// (maps openclaw `node-match.ts::normalizeNodeKey`), so "GPU Box" can be
    /// addressed as "gpu-box". If any level yields multiple hits, reports
    /// `Ambiguous` — never silently picks the first.
    fn match_id(inner: &RegistryInner, q: &str) -> std::result::Result<String, ResolveError> {
        // ① Exact id (UUID, case-sensitive, no normalization — avoids collapsing
        //    hyphen semantics within the id).
        if inner.nodes_by_id.contains_key(q) {
            return Ok(q.to_string());
        }
        let nq = normalize_node_key(q);
        // ② Normalized exact name (device_name is not guaranteed unique → may be
        //    ambiguous). Skip name matching for empty keys (all-punctuation
        //    queries), otherwise they would falsely match dirty names that also
        //    normalize to empty.
        if !nq.is_empty() {
            let exact: Vec<&RegisteredNode> = inner
                .nodes_by_id
                .values()
                .filter(|n| normalize_node_key(&n.session.device_name) == nq)
                .collect();
            match exact.as_slice() {
                [n] => return Ok(n.session.node_id.clone()),
                [] => {}
                many => return Err(ResolveError::Ambiguous(candidate_labels(many))),
            }
        }
        // ③ Fuzzy: id prefix (≥4 chars, as-is lowercased, avoids 1-char
        //    explosion) or normalized name substring.
        let ql = q.to_ascii_lowercase();
        let fuzzy: Vec<&RegisteredNode> = inner
            .nodes_by_id
            .values()
            .filter(|n| {
                (q.len() >= 4 && n.session.node_id.to_ascii_lowercase().starts_with(&ql))
                    || (!nq.is_empty()
                        && normalize_node_key(&n.session.device_name).contains(&nq))
            })
            .collect();
        match fuzzy.as_slice() {
            [n] => Ok(n.session.node_id.clone()),
            [] => Err(ResolveError::NotFound),
            many => Err(ResolveError::Ambiguous(candidate_labels(many))),
        }
    }

    /// Resolve an online node by name or id, returning its reverse RPC channel
    /// and declared command catalog.
    ///
    /// `node_invoke` / `node_file` use this to address and fail-fast validate.
    /// Ambiguity or miss is returned as a structured [`ResolveError`],
    /// letting callers give precise hints to the LLM.
    pub fn resolve(
        &self,
        name_or_id: &str,
    ) -> std::result::Result<(ReverseRpcChannel, Vec<CommandDescriptor>), ResolveError> {
        let (channel, declared, _) = self.resolve_with_catalog(name_or_id)?;
        Ok((channel, declared))
    }

    /// Same multi-level match as [`resolve`](Self::resolve), plus the parse
    /// state of the declared catalog: the third element is `true` when the
    /// node's `commands` field was **present but malformed**.
    ///
    /// A malformed catalog is NOT an empty catalog: an empty one means "the
    /// node declares no commands" (the node stays authoritative and the center
    /// skips its declaration check), while a malformed one means the center
    /// could not read what the node permits — so the caller has to fail closed
    /// instead of skipping the check.
    pub fn resolve_with_catalog(
        &self,
        name_or_id: &str,
    ) -> std::result::Result<(ReverseRpcChannel, Vec<CommandDescriptor>, bool), ResolveError> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let id = Self::match_id(&inner, name_or_id)?;
        let n = inner
            .nodes_by_id
            .get(&id)
            .ok_or_else(|| ResolveError::NodeNotFound {
                name_or_id: name_or_id.to_string(),
            })?;
        Ok((
            n.session.channel.clone(),
            n.session.declared_commands.clone(),
            n.catalog_malformed,
        ))
    }

    /// Same multi-level match as [`resolve`], but returns only the `node_id` —
    /// `cluster.deregister` uses this to map an operator-supplied name/id to a
    /// unique node identity before evicting + revoking the token. `pub(crate)`
    /// because every current caller lives inside this crate
    /// (`enrollment::deregister_node` and its tests); a single test outside
    /// the crate (`src/gateway/handlers/cluster.rs:269`) references this
    /// function by doc comment only and does not call it.
    pub(crate) fn resolve_id(&self, name_or_id: &str) -> std::result::Result<String, ResolveError> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        Self::match_id(&inner, name_or_id)
    }

    /// All online nodes carrying EVERY tag in `tags` (AND match). An empty
    /// `tags` slice matches every online node (the "broadcast" case). Used by
    /// `node_invoke_many` for tag-selected concurrent fan-out. Returns a clone
    /// snapshot so the caller dispatches without holding the registry lock.
    ///
    /// Results are sorted by `(node_id, name)` so the JoinSet spawn order is
    /// deterministic across calls — `node_invoke_many` already sorts its
    /// result envelope, but a future caller that observes spawn order, or a
    /// test asserting on fan-out sequencing, would otherwise inherit
    /// HashMap-iteration jitter.
    pub fn resolve_all_by_tags(&self, tags: &[String]) -> Vec<NodeMatch> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<NodeMatch> = inner
            .nodes_by_id
            .values()
            .filter(|n| tags.iter().all(|t| n.session.tags.contains(t)))
            .map(|n| NodeMatch {
                node_id: n.session.node_id.clone(),
                name: n.session.device_name.clone(),
                channel: n.session.channel.clone(),
                declared_commands: n.session.declared_commands.clone(),
                tags: n.session.tags.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.node_id.cmp(&b.node_id).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// Actively evict a session by `node_id` (used by operator deregister).
    /// Removes from both tables **and asks the node's connection to close**.
    /// Returns the evicted session, or `None` if no live session held that
    /// `node_id` (the caller uses the returned session's `device_name` /
    /// `conn_id` to publish `node.disconnected` without a second lookup).
    /// Orthogonal to
    /// [`deregister`](Self::deregister) (disconnect reconciliation by `conn_id`).
    ///
    /// **Why the close signal**: eviction alone only stops *new* dispatches
    /// (`node_invoke` / `node_file` can no longer address the node). The socket
    /// itself survives until the next ping / ≤90s inbound idle-watchdog, and
    /// until then the revoked node still runs whatever the center dispatched
    /// before, and its `node.approval.request` path can still raise approval
    /// cards at the operator who just deregistered it. Firing the connection's
    /// close signal (bound per-connection via
    /// [`ReverseRpcChannel::with_close`]) runs the handler's existing full
    /// cleanup, after which the node's reconnect meets
    /// [`NodeAdmission::Deregistered`](crate::cluster::NodeAdmission) and exits.
    /// No-op for channels built with [`ReverseRpcChannel::new`] (node side,
    /// tests).
    pub fn forget(&self, node_id: &str) -> Option<NodeSession> {
        // Both tables are mutated under ONE guard, and the close fires after it
        // drops. Not for deadlock reasons — `close_connection` is a
        // non-blocking `Notify::notify_one` that takes no lock and re-enters
        // nothing; the connection task's `deregister` runs later, on its own
        // task. The point is that the eviction is atomic (see `register`) and
        // the critical section carries no wakeup side effects.
        let removed = {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            let removed = inner.nodes_by_id.remove(node_id);
            if let Some(n) = &removed {
                inner.nodes_by_conn.remove(&n.session.conn_id);
            }
            removed
        };
        if let Some(n) = &removed {
            tracing::info!(node_id = %node_id, name = %n.session.device_name, "cluster node session evicted (operator forgot/forget)");
            n.session.channel.close_connection();
        }
        removed.map(|n| n.session)
    }
}

/// Render candidate sessions as human-readable labels `name (short-id)` for
/// ambiguity errors. The short-id is the first 8 chars — enough to
/// disambiguate without noise. Results are sorted for stable error messages
/// (easier test/log comparison).
fn candidate_labels(sessions: &[&RegisteredNode]) -> Vec<String> {
    let mut labels: Vec<String> = sessions
        .iter()
        .map(|n| {
            let short = truncate_on_char_boundary(&n.session.node_id, 8);
            format!("{} ({})", n.session.device_name, short)
        })
        .collect();
    labels.sort();
    labels
}

/// Normalize a human-readable node name into a stable lookup key: lowercase +
/// collapse each run of non-alphanumeric chars to a single `-` + strip leading
/// and trailing `-`. Alphanumeric detection uses Unicode-aware
/// [`char::is_alphanumeric`] (NOT ASCII-only), so CJK / accented Latin
/// characters are **preserved** rather than discarded — `"工作站"` normalizes to
/// a non-empty key and remains addressable by name ("GPU Box" / "gpu_box" still
/// collapse to `gpu-box`). The old ASCII-only impl would fold a purely
/// non-ASCII name to an empty key ⇒ Chinese/Japanese node names were completely
/// unaddressable by name and every reconnect in
/// [`crate::cluster::admit_node`] would mint a fresh id (ghost row
/// proliferation).
///
/// Maps the **common branch** of openclaw `node-match.ts::normalizeNodeKey`'s
/// Unicode-aware version (NFC + `[^\p{L}\p{M}\p{N}]+ → -`). **Intentional
/// deviation (R3 core minimalism — not pulling in the `unicode-normalization`
/// crate for a single helper)**: combining marks (`\p{M}`, e.g. Devanagari
/// vowel signs / decomposed accents) are treated as separators, and NFC is not
/// applied. This only affects the key's **appearance**, not addressability —
/// normalization is applied **symmetrically** to both query and stored name, so
/// the two sides fold the same way and can match.
///
/// Both online [`NodeRegistry::match_id`] and offline `cluster.deregister`
/// fallback addressing share this single source of truth, preventing semantic
/// drift between the two paths. Empty keys (all-punctuation / all-mark queries)
/// are guarded by an `is_empty` check at each call site.
pub(crate) fn normalize_node_key(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_dash = false;
    for ch in value.chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            // `char::to_lowercase` may produce multiple chars (e.g. İ → i̇);
            // use extend rather than push.
            out.extend(ch.to_lowercase());
        } else {
            // Defer the separator so leading/trailing/repeated runs collapse and
            // never produce a boundary dash.
            pending_dash = true;
        }
    }
    out
}

/// connect→register seam: registers this connection into `NodeRegistry` only
/// when `role == Some("node")`. `params` is the connect frame's params
/// (extracts `device_name` + commands). Returns whether registration occurred.
/// Extracted as a pure function for unit testing and to keep `server/connection/mod.rs` thin.
///
/// The `role` gate is currently dead in production: the single live caller —
/// the `connect` frame's `NodeAdmission::Admitted` arm in
/// `gateway::server::connection::handle_connection` — passes a hardcoded
/// `Some("node")` after upstream shape detection. The gate is kept so
/// future call sites cannot register a non-node connection by accident; the
/// contract is "call this only for `role == Some("node")`", and the unit
/// test `maybe_register_node_registers_only_for_node_role` enforces it.
pub fn maybe_register_node(
    registry: &NodeRegistry,
    role: Option<&str>,
    device_id: &str,
    conn_id: &str,
    params: Option<&Value>,
    channel: &ReverseRpcChannel,
) -> bool {
    if role != Some("node") {
        return false;
    }
    // (B1-08) A connect frame from a node without `device_name` is
    // suspicious: every shipped runtime sends the field. Surface the absence
    // so an operator chasing an "anonymous" fleet entry has a breadcrumb.
    let device_name = bound_device_name(
        params_str(params, "device_name").unwrap_or_else(|| {
            tracing::warn!(
                device_id = %device_id,
                conn_id = %conn_id,
                "cluster node connect frame omitted device_name; falling back to \"unknown\""
            );
            "unknown".to_string()
        }),
        device_id,
    );
    // (B1-02) Parse failures used to silently downgrade to an empty list. A
    // node with no declared commands is registered as "online but every
    // command denied" — confusing for the operator, and lets a peer hold
    // fleet slots with malformed frames. Log and downgrade; only `commands`
    // is gating (every node ships `bash`), so an empty commands list keeps
    // the registration but is loud about it.
    let (declared_commands, catalog_malformed) = match params_typed::<Vec<CommandDescriptor>>(
        params,
        "commands",
        device_id,
        &device_name,
        "commands; registering with an empty catalog flagged as malformed",
    ) {
        FrameField::Present(commands) => (bound_commands(commands, device_id), false),
        FrameField::Absent => (Vec::new(), false),
        // A malformed catalog is NOT an empty catalog: recording it as empty
        // would tell the center "this node declares no commands", which
        // switches the center-side fail-fast check OFF for this node. The
        // flag travels with the session so the center can fail closed.
        FrameField::Malformed => (Vec::new(), true),
    };
    let tags: Vec<String> = match params_typed::<Vec<String>>(
        params,
        "tags",
        device_id,
        &device_name,
        "tags; registering with empty tag list",
    ) {
        FrameField::Present(tags) => bound_tags(tags, device_id),
        FrameField::Absent | FrameField::Malformed => Vec::new(),
    };
    let version: Option<String> = match params.and_then(|p| p.get("version")) {
        None => None,
        Some(v) => match v.as_str() {
            Some(s) if !s.is_empty() => Some(s.to_string()),
            Some(_) => None,
            None => {
                tracing::warn!(
                    device_id = %device_id,
                    node = %device_name,
                    "cluster node connect frame version field is not a string; treating as absent"
                );
                None
            }
        },
    };
    // Version skew is **surfaced, not enforced**. A center and its fleet live on
    // separate upgrade schedules, so refusing a skewed node (openclaw's
    // `server.node-version-mismatch` guard, which only governs its same-machine
    // bundled local node) would freeze the whole fleet on the center's release.
    // One log line per connect + the field on `node_list` / `environments.list`
    // is enough for an operator to correlate "this node behaves oddly" with
    // "this node is three releases behind".
    match version.as_deref() {
        Some(v) if v != env!("ALEPH_VERSION") => tracing::warn!(
            node = %device_name,
            node_version = v,
            center_version = env!("ALEPH_VERSION"),
            "cluster node runs a different aleph-server build than the center"
        ),
        None => tracing::debug!(
            node = %device_name,
            "cluster node declared no version (predates the version handshake)"
        ),
        Some(_) => {}
    }
    registry.register_session(
        NodeSession {
            node_id: device_id.to_string(),
            conn_id: conn_id.to_string(),
            device_name,
            channel: channel.clone(),
            declared_commands,
            tags,
            version,
            connected_at: now_unix(),
        },
        catalog_malformed,
    );
    true
}

/// Read a string field from the connect-frame params, returning `Some(s)`
/// when present and a UTF-8 string, `None` otherwise. Centralises the
/// `params.and_then(|p| p.get(K)).and_then(|v| v.as_str()).map(str::to_string)`
/// shape that every connect-frame field-share read follows.
fn params_str(params: Option<&Value>, key: &str) -> Option<String> {
    params
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Outcome of reading a typed field from a node's connect frame.
///
/// "Absent" and "malformed" are deliberately distinct: an absent `commands`
/// field means the node declared no catalog (the node stays authoritative),
/// while a malformed one means the center could not read what the node
/// permits — the caller has to fail closed rather than treat the node as
/// having declared nothing.
enum FrameField<T> {
    Absent,
    Present(T),
    Malformed,
}

/// Read a typed field from the connect-frame params, reporting absence and
/// deserialisation failure separately (see [`FrameField`]). The
/// `fallback_msg` suffix is the second half of the `cluster node connect
/// frame carried malformed <fallback_msg>` warn line — the caller has
/// already decided what the empty fallback means in its own domain
/// (commands → empty catalog, tags → empty list).
fn params_typed<T>(
    params: Option<&Value>,
    key: &str,
    device_id: &str,
    device_name: &str,
    fallback_msg: &str,
) -> FrameField<T>
where
    T: serde::de::DeserializeOwned,
{
    let Some(value) = params.and_then(|p| p.get(key)) else {
        return FrameField::Absent;
    };
    match serde_json::from_value::<T>(value.clone()) {
        Ok(parsed) => FrameField::Present(parsed),
        Err(e) => {
            tracing::warn!(
                device_id = %device_id,
                node = %device_name,
                error = %e,
                "cluster node connect frame carried malformed {fallback_msg}"
            );
            FrameField::Malformed
        }
    }
}

/// Bounds on the peer-controlled metadata a node may declare in its connect
/// frame.
///
/// The registry stores this verbatim and [`NodeRegistry::list_environments`]
/// deep-clones it on **every** call (it feeds the model-callable `node_list`),
/// so an unbounded declaration is a memory-growth and model-budget-displacement
/// lever for anyone who can reach the gateway. The limits are generous for a
/// real node (one name, a handful of tags, a handful of commands) and are
/// counted in **bytes** — the unit [`truncate_on_char_boundary`] works in.
const MAX_DEVICE_NAME_BYTES: usize = 128;
const MAX_TAG_COUNT: usize = 32;
const MAX_TAG_BYTES: usize = 64;
const MAX_COMMAND_COUNT: usize = 64;
const MAX_COMMAND_NAME_BYTES: usize = 64;
const MAX_COMMAND_SCHEMA_BYTES: usize = 8 * 1024;

/// Bound the declared device name. It is a display affordance, so truncating
/// is always safe (and never splits a UTF-8 scalar).
fn bound_device_name(raw: String, device_id: &str) -> String {
    if raw.len() <= MAX_DEVICE_NAME_BYTES {
        return raw;
    }
    tracing::warn!(
        device_id = %device_id,
        bytes = raw.len(),
        limit = MAX_DEVICE_NAME_BYTES,
        "cluster node connect frame declared an oversized device_name; truncating"
    );
    truncate_on_char_boundary(&raw, MAX_DEVICE_NAME_BYTES).to_string()
}

/// Bound the declared tags: how many there may be, and how long each may be.
fn bound_tags(raw: Vec<String>, device_id: &str) -> Vec<String> {
    if raw.len() > MAX_TAG_COUNT {
        tracing::warn!(
            device_id = %device_id,
            count = raw.len(),
            limit = MAX_TAG_COUNT,
            "cluster node connect frame declared too many tags; dropping the extras"
        );
    }
    raw.into_iter()
        .take(MAX_TAG_COUNT)
        .map(|tag| {
            if tag.len() <= MAX_TAG_BYTES {
                return tag;
            }
            tracing::warn!(
                device_id = %device_id,
                bytes = tag.len(),
                limit = MAX_TAG_BYTES,
                "cluster node connect frame declared an oversized tag; truncating"
            );
            truncate_on_char_boundary(&tag, MAX_TAG_BYTES).to_string()
        })
        .collect()
}

/// Bound the declared command catalog: how many commands, how long each
/// command name may be, and how large each declared schema may be.
///
/// The schema is informational today — nothing validates the caller's `args`
/// against it (the node-side allowlist is the authoritative gate) — so an
/// oversized one is replaced with an empty object instead of being stored.
fn bound_commands(raw: Vec<CommandDescriptor>, device_id: &str) -> Vec<CommandDescriptor> {
    if raw.len() > MAX_COMMAND_COUNT {
        tracing::warn!(
            device_id = %device_id,
            count = raw.len(),
            limit = MAX_COMMAND_COUNT,
            "cluster node connect frame declared too many commands; dropping the extras"
        );
    }
    raw.into_iter()
        .take(MAX_COMMAND_COUNT)
        .map(|mut command| {
            if command.name.len() > MAX_COMMAND_NAME_BYTES {
                tracing::warn!(
                    device_id = %device_id,
                    bytes = command.name.len(),
                    limit = MAX_COMMAND_NAME_BYTES,
                    "cluster node connect frame declared an oversized command name; truncating"
                );
                let bounded = truncate_on_char_boundary(&command.name, MAX_COMMAND_NAME_BYTES)
                    .to_string();
                command.name = bounded;
            }
            let schema_bytes = serde_json::to_string(&command.schema).map_or(usize::MAX, |s| s.len());
            if schema_bytes > MAX_COMMAND_SCHEMA_BYTES {
                tracing::warn!(
                    device_id = %device_id,
                    command = %command.name,
                    bytes = schema_bytes,
                    limit = MAX_COMMAND_SCHEMA_BYTES,
                    "cluster node connect frame declared an oversized command schema; dropping it"
                );
                command.schema = serde_json::json!({});
            }
            command
        })
        .collect()
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| {
            // Host clock is before 1970-01-01 (RTC dead, NTP unsynced, VM
            // clock skew on resume). Returning 0 would silently stamp every
            // freshly-registered NodeSession as "connected at the epoch"
            // and mislead any operator inspecting the fleet. Surface the
            // anomaly once via tracing and pick `i64::MIN` so downstream
            // readers can distinguish a bogus stamp from a real 1970 one.
            tracing::warn!(
                "now_unix: host clock is before UNIX_EPOCH; connected_at \
                     values will be inaccurate until the system clock is fixed"
            );
            i64::MIN
        },
        |d| d.as_secs() as i64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc;

    fn test_channel() -> ReverseRpcChannel {
        let (tx, _rx) = mpsc::channel::<String>(8);
        ReverseRpcChannel::new(tx)
    }

    fn session(node_id: &str, conn_id: &str) -> NodeSession {
        NodeSession {
            node_id: node_id.to_string(),
            conn_id: conn_id.to_string(),
            device_name: format!("dev-{node_id}"),
            channel: test_channel(),
            declared_commands: vec![CommandDescriptor {
                name: "bash".to_string(),
                schema: json!({"type": "object"}),
            }],
            tags: vec![],
            version: None,
            connected_at: 1,
        }
    }

    /// Two connections racing to register the SAME `node_id` must not both end
    /// up in `nodes_by_conn`.
    ///
    /// Reachable in production: every WS connection is its own tokio task and
    /// `maybe_register_node` is called from each, so a network flap that makes a
    /// node redial while its old socket is still live runs two `register` calls
    /// for one `node_id` concurrently. While `register` dropped its write guard
    /// mid-function, both calls could read the SAME previous session, both
    /// remove its conn mapping, and then both insert their own — leaving a
    /// connection that the registry never closed and never overwrote, yet which
    /// still resolves through [`NodeRegistry::node_identity_by_conn`]. That
    /// function is the anti-spoof stamp for every inbound node frame, so the
    /// orphan speaks as the node under the winning session's identity.
    #[test]
    fn concurrent_register_same_node_leaves_no_orphan_conn_mapping() {
        use std::sync::{Arc, Barrier};

        const THREADS: usize = 4;
        const ROUNDS: usize = 500;

        for round in 0..ROUNDS {
            let reg = Arc::new(NodeRegistry::new());
            let barrier = Arc::new(Barrier::new(THREADS));
            let conns: Vec<String> = (0..THREADS).map(|i| format!("conn-{i}")).collect();
            let handles: Vec<_> = conns
                .iter()
                .cloned()
                .map(|conn| {
                    let reg = Arc::clone(&reg);
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        std::thread::yield_now();
                        reg.register(session("node-a", &conn));
                    })
                })
                .collect();
            for h in handles {
                h.join().expect("a register thread panicked");
            }

            // Public-API assertion, on the security-relevant consumer: exactly
            // one conn_id may carry the node's identity — the one whose session
            // `nodes_by_id` actually holds.
            let resolving: Vec<(&String, (String, String))> = conns
                .iter()
                .filter_map(|c| reg.node_identity_by_conn(c).map(|id| (c, id)))
                .collect();
            assert_eq!(
                resolving.len(),
                1,
                "round {round}: {} connections resolve as node-a ({resolving:?}); every \
                 loser must be evicted from nodes_by_conn, otherwise it keeps speaking as \
                 the node under another session's identity",
                resolving.len()
            );
        }
    }

    #[test]
    fn register_then_list_projects_environment() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "node-a");
        assert_eq!(envs[0].name, "dev-node-a");
        assert_eq!(envs[0].status, "online");
        assert_eq!(envs[0].commands.len(), 1);
        assert_eq!(envs[0].commands[0].name, "bash");
        assert!(envs[0].tags.is_empty());
    }

    #[test]
    fn deregister_removes_from_both_maps() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        assert!(reg.deregister("conn-1"));
        assert!(reg.list_environments().is_empty());
        assert!(reg.resolve("node-a").is_err());
        assert!(!reg.deregister("conn-x"));
    }

    #[test]
    fn reconnect_same_node_overwrites_and_old_cleanup_does_not_evict_new() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        reg.register(session("node-a", "conn-2"));
        assert_eq!(reg.list_environments().len(), 1);
        assert!(!reg.deregister("conn-1"));
        assert_eq!(reg.list_environments().len(), 1);
        assert!(reg.deregister("conn-2"));
        assert!(reg.list_environments().is_empty());
    }

    #[test]
    fn list_environments_is_sorted_by_name_then_id() {
        let reg = NodeRegistry::new();
        // Register out of name order; the projection must come back sorted so the
        // HashMap iteration order can't leak into the Panel / node_list view.
        reg.register(session("z-id", "c-z")); // device_name = "dev-z-id"
        reg.register(session("a-id", "c-a")); // device_name = "dev-a-id"
        reg.register(session("m-id", "c-m")); // device_name = "dev-m-id"
        let names: Vec<String> = reg
            .list_environments()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["dev-a-id", "dev-m-id", "dev-z-id"]);
    }

    #[test]
    fn resolve_by_id_then_by_name() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1")); // device_name = "dev-node-a"
        assert!(reg.resolve("node-a").is_ok(), "by id");
        let (_, cmds) = reg.resolve("dev-node-a").expect("by name");
        assert_eq!(cmds[0].name, "bash");
        assert!(matches!(reg.resolve("nope"), Err(ResolveError::NotFound)));
    }

    #[test]
    fn resolve_by_unique_id_prefix_and_name_substring() {
        let reg = NodeRegistry::new();
        reg.register(session("abcd1234", "conn-1")); // device_name = "dev-abcd1234"
                                                     // id prefix (≥4) uniquely matches.
        assert_eq!(reg.resolve_id("abcd").unwrap(), "abcd1234");
        // name substring (case-insensitive) uniquely matches.
        assert_eq!(reg.resolve_id("ABCD1234").unwrap(), "abcd1234");
        assert_eq!(reg.resolve_id("dev-abcd").unwrap(), "abcd1234");
        // too-short prefix that isn't a name substring → not found.
        assert_eq!(reg.resolve_id("xyz").unwrap_err(), ResolveError::NotFound);
    }

    #[test]
    fn normalize_node_key_folds_case_and_punctuation() {
        assert_eq!(normalize_node_key("GPU Box"), "gpu-box");
        assert_eq!(normalize_node_key("gpu_box"), "gpu-box");
        assert_eq!(normalize_node_key("gpu-box"), "gpu-box");
        assert_eq!(normalize_node_key("  GPU   Box!! "), "gpu-box");
        assert_eq!(normalize_node_key("--Worker__1--"), "worker-1");
        assert_eq!(normalize_node_key("Worker1"), "worker1");
        // All-punctuation / empty → empty key (callers skip name matching on it).
        assert_eq!(normalize_node_key("  -_- "), "");
        assert_eq!(normalize_node_key(""), "");
    }

    #[test]
    fn normalize_node_key_is_unicode_aware() {
        // Non-ASCII letters must SURVIVE rather than collapse to an empty key —
        // a pure-CJK node name was previously unaddressable and re-minted a fresh
        // id on every reconnect. Maps openclaw's Unicode-aware normalizeNodeKey.
        assert_eq!(normalize_node_key("工作站"), "工作站");
        assert_eq!(normalize_node_key("工作站 01"), "工作站-01");
        assert_eq!(normalize_node_key("GPU 工作站"), "gpu-工作站");
        // Precomposed accented Latin lowercases and is preserved (café, not caf).
        assert_eq!(normalize_node_key("Café"), "café");
        // Combining-mark scripts (Devanagari vowel signs are \p{M}, dropped in the
        // zero-dep impl) still fold to a stable NON-EMPTY key, so the node stays
        // addressable by name — the key is an internal match key, need not be
        // visually identical, and the same fold applies to query and stored name.
        assert!(!normalize_node_key("किताब").is_empty());
        // All-punctuation, including non-ASCII punctuation, still folds to empty.
        assert_eq!(normalize_node_key("。、！"), "");
    }

    #[test]
    fn resolve_cjk_name_is_addressable() {
        let reg = NodeRegistry::new();
        reg.register(NodeSession {
            node_id: "id-cn".into(),
            conn_id: "c-cn".into(),
            device_name: "工作站".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        // Exact CJK name resolves (was NotFound before: "工作站" → "" → the
        // empty-key guard skipped name matching entirely).
        assert_eq!(reg.resolve_id("工作站").unwrap(), "id-cn");
        // Fuzzy substring on the normalized CJK form also resolves.
        assert_eq!(reg.resolve_id("工作").unwrap(), "id-cn");
    }

    #[test]
    fn resolve_name_is_case_and_punctuation_insensitive() {
        let reg = NodeRegistry::new();
        reg.register(NodeSession {
            node_id: "id-1".into(),
            conn_id: "c1".into(),
            device_name: "GPU Box".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        // The operator/LLM can spell the spaced name with a dash, underscore, or
        // any case — all fold to the same key (maps openclaw normalizeNodeKey).
        assert_eq!(reg.resolve_id("gpu-box").unwrap(), "id-1");
        assert_eq!(reg.resolve_id("GPU_BOX").unwrap(), "id-1");
        assert_eq!(reg.resolve_id("gpu box").unwrap(), "id-1");
        // Substring fuzzy still works on the normalized form.
        assert_eq!(reg.resolve_id("box").unwrap(), "id-1");
        // An all-punctuation query matches nothing (empty normalized key).
        assert_eq!(reg.resolve_id("---").unwrap_err(), ResolveError::NotFound);
    }

    #[test]
    fn normalized_names_that_collide_are_ambiguous() {
        let reg = NodeRegistry::new();
        // "Worker 1" and "worker-1" both normalize to "worker-1" — addressing by
        // name must report ambiguity rather than silently pick one.
        reg.register(NodeSession {
            node_id: "id-a".into(),
            conn_id: "ca".into(),
            device_name: "Worker 1".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        reg.register(NodeSession {
            node_id: "id-b".into(),
            conn_id: "cb".into(),
            device_name: "worker-1".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        assert!(matches!(
            reg.resolve_id("WORKER_1"),
            Err(ResolveError::Ambiguous(_))
        ));
    }

    #[test]
    fn resolve_reports_ambiguity_with_sorted_candidates() {
        let reg = NodeRegistry::new();
        // Two nodes whose names share the substring "work".
        reg.register(NodeSession {
            node_id: "id-two".to_string(),
            conn_id: "c2".to_string(),
            device_name: "worker-2".to_string(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        reg.register(NodeSession {
            node_id: "id-one".to_string(),
            conn_id: "c1".to_string(),
            device_name: "worker-1".to_string(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        match reg.resolve_id("worker").unwrap_err() {
            ResolveError::Ambiguous(c) => {
                // sorted + labelled "name (short-id)".
                assert_eq!(c, vec!["worker-1 (id-one)", "worker-2 (id-two)"]);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn truncate_on_char_boundary_never_splits_a_scalar() {
        assert_eq!(truncate_on_char_boundary("abcdefghij", 8), "abcdefgh");
        assert_eq!(truncate_on_char_boundary("abc", 8), "abc");
        assert_eq!(truncate_on_char_boundary("", 8), "");
        // "工作站" is 9 bytes with boundaries at 0/3/6/9 — byte 8 is INSIDE the
        // last scalar, which is exactly where `&s[..8]` panics.
        assert_eq!(truncate_on_char_boundary("工作站", 8), "工作");
        // Backing off past a whole scalar can legitimately reach the empty
        // string rather than panicking.
        assert_eq!(truncate_on_char_boundary("工", 2), "");
    }

    #[test]
    fn ambiguity_labels_survive_non_ascii_node_ids() {
        // `admit_node`'s "unknown id" branch adopts whatever `device_id` the
        // peer presented, so a node id is NOT guaranteed to be an ASCII UUID.
        // Rendering ambiguity candidates used to byte-slice it to 8 chars and
        // panic the caller's task on a multi-byte boundary.
        let reg = NodeRegistry::new();
        for (id, name) in [("节点标识符甲", "worker-1"), ("节点标识符乙", "worker-2")] {
            reg.register(NodeSession {
                node_id: id.into(),
                conn_id: format!("c-{name}"),
                device_name: name.into(),
                channel: test_channel(),
                declared_commands: vec![],
                tags: vec![],
                version: None,
                connected_at: 1,
            });
        }
        match reg.resolve_id("worker").unwrap_err() {
            ResolveError::Ambiguous(c) => assert_eq!(c.len(), 2, "{c:?}"),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn forget_asks_the_connection_to_close() {
        // Evicting a session must also tear the socket down. Eviction alone only
        // stops NEW dispatches; the revoked node would keep its connection (and
        // its approval path back to the operator) until the ≤90s inbound
        // idle-watchdog — which never fires while the node keeps sending.
        let (tx, _rx) = mpsc::channel::<String>(8);
        let close = std::sync::Arc::new(tokio::sync::Notify::new());
        let reg = NodeRegistry::new();
        reg.register(NodeSession {
            node_id: "n-1".into(),
            conn_id: "c-1".into(),
            device_name: "worker-1".into(),
            channel: ReverseRpcChannel::with_close(tx, close.clone()),
            declared_commands: vec![],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        assert!(reg.forget("n-1").is_some());
        tokio::time::timeout(std::time::Duration::from_secs(1), close.notified())
            .await
            .expect("forget must fire the connection's close signal");
    }

    #[test]
    fn maybe_register_node_records_the_declared_version() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let params = json!({"device_name": "worker", "commands": [], "version": "26.7.25"});
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d1",
            "c1",
            Some(&params),
            &ch
        ));
        assert_eq!(
            reg.list_environments()[0].version.as_deref(),
            Some("26.7.25")
        );
        // A node predating the handshake registers fine with no version.
        let ch2 = test_channel();
        let legacy = json!({"device_name": "old", "commands": []});
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d2",
            "c2",
            Some(&legacy),
            &ch2
        ));
        let old = reg
            .list_environments()
            .into_iter()
            .find(|e| e.id == "d2")
            .expect("registered");
        assert!(old.version.is_none(), "skew is observed, never enforced");
    }

    #[test]
    fn forget_evicts_by_node_id_from_both_maps() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        assert!(reg.forget("node-a").is_some());
        assert!(reg.list_environments().is_empty());
        assert!(reg.resolve("node-a").is_err());
        // A stale conn cleanup after forget is a harmless no-op.
        assert!(!reg.deregister("conn-1"));
        // Forgetting an unknown id reports nothing removed.
        assert!(reg.forget("ghost").is_none());
    }

    #[test]
    fn node_identity_by_conn_returns_id_and_name() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1")); // device_name = "dev-node-a"
        assert_eq!(
            reg.node_identity_by_conn("conn-1"),
            Some(("node-a".to_string(), "dev-node-a".to_string()))
        );
        assert_eq!(reg.node_identity_by_conn("conn-x"), None);
    }

    #[test]
    fn maybe_register_node_registers_only_for_node_role() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let params = json!({"device_name": "worker", "commands": [{"name": "bash", "schema": {}}]});
        assert!(!maybe_register_node(
            &reg,
            Some("operator"),
            "d1",
            "c1",
            Some(&params),
            &ch
        ));
        assert!(reg.list_environments().is_empty());
        assert!(!maybe_register_node(
            &reg,
            None,
            "d0",
            "c0",
            Some(&params),
            &ch
        ));
        assert!(reg.list_environments().is_empty());
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d2",
            "c2",
            Some(&params),
            &ch
        ));
        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "d2");
        assert_eq!(envs[0].commands[0].name, "bash");
    }

    #[test]
    fn resolve_all_by_tags_and_semantics() {
        let reg = NodeRegistry::new();
        reg.register(NodeSession {
            node_id: "a".into(),
            conn_id: "ca".into(),
            device_name: "node-a".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec!["gpu".into(), "us".into()],
            version: None,
            connected_at: 1,
        });
        reg.register(NodeSession {
            node_id: "b".into(),
            conn_id: "cb".into(),
            device_name: "node-b".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec!["gpu".into()],
            version: None,
            connected_at: 1,
        });
        // AND: both tags required → only "a".
        let both = reg.resolve_all_by_tags(&["gpu".into(), "us".into()]);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].node_id, "a");
        assert_eq!(both[0].name, "node-a");
        // Single tag both carry → both.
        assert_eq!(reg.resolve_all_by_tags(&["gpu".into()]).len(), 2);
        // Empty tags → every online node.
        assert_eq!(reg.resolve_all_by_tags(&[]).len(), 2);
        // Unmatched tag → none.
        assert!(reg.resolve_all_by_tags(&["fpga".into()]).is_empty());
        // NodeMatch carries the node's tags (used for the zero-match hint).
        let gpu = reg.resolve_all_by_tags(&["gpu".into()]);
        assert!(gpu.iter().any(|m| m.tags.contains(&"us".to_string())));
        // (B1-05) HashMap iteration is per-process-random; fan-out callers
        // rely on this returning the same order across calls so JoinSet
        // spawn order is deterministic.
        let gpu_again = reg.resolve_all_by_tags(&["gpu".into()]);
        assert_eq!(
            gpu.iter().map(|m| &m.node_id).collect::<Vec<_>>(),
            gpu_again.iter().map(|m| &m.node_id).collect::<Vec<_>>(),
            "resolve_all_by_tags must be deterministic across calls"
        );
        assert_eq!(
            gpu.iter().map(|m| &m.node_id).collect::<Vec<_>>(),
            vec![&"a".to_string(), &"b".to_string()],
            "ordered by node_id"
        );
    }

    #[test]
    fn resolve_error_display_covers_each_variant() {
        // (B1-07) Each variant's Display string is part of the operator-facing
        // contract for cluster.deregister and node_invoke — test the surface
        // directly so a refactor that drops the human-readable prefix is caught.
        assert_eq!(ResolveError::NotFound.to_string(), "no online node matches");
        assert_eq!(
            ResolveError::Ambiguous(vec!["a (aaa)".into(), "b (bbb)".into()]).to_string(),
            "ambiguous — matches: a (aaa), b (bbb)"
        );
        assert_eq!(
            ResolveError::NodeNotFound {
                name_or_id: "x".into()
            }
            .to_string(),
            "internal node lookup failed for 'x'"
        );
    }

    #[test]
    fn maybe_register_node_parses_tags_from_params() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let params = json!({
            "device_name": "worker",
            "commands": [{"name": "bash", "schema": {}}],
            "tags": ["gpu", "region=us"]
        });
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d1",
            "c1",
            Some(&params),
            &ch
        ));
        let m = reg.resolve_all_by_tags(&["region=us".into()]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].node_id, "d1");
        // Missing "tags" key → empty, not an error.
        let ch2 = test_channel();
        let no_tags = json!({"device_name": "w2", "commands": []});
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d2",
            "c2",
            Some(&no_tags),
            &ch2
        ));
        assert_eq!(reg.resolve_all_by_tags(&[]).len(), 2);
    }
    /// Shared close-signal probe for the contrasting pair below, so ONE
    /// instrument has to report both polarities (a "was it closed?" probe that
    /// can only ever answer one way is worthless). `notify_one` stores a permit
    /// when nobody is waiting, so a close fired synchronously inside `register`
    /// is still observable afterwards: a single poll of `notified()` picks the
    /// permit up. The zero-length timeout makes that single poll the whole
    /// probe — ready ⇒ a permit was stored (fired), elapsed ⇒ none was.
    async fn close_fired(close: &std::sync::Arc<tokio::sync::Notify>) -> bool {
        tokio::time::timeout(std::time::Duration::from_millis(0), close.notified())
            .await
            .is_ok()
    }

    /// Like [`session`], but the channel carries a close signal, the way the
    /// center builds it per connection.
    fn session_with_close(
        node_id: &str,
        conn_id: &str,
        close: &std::sync::Arc<tokio::sync::Notify>,
    ) -> NodeSession {
        let (tx, _rx) = mpsc::channel::<String>(8);
        NodeSession {
            channel: ReverseRpcChannel::with_close(tx, close.clone()),
            ..session(node_id, conn_id)
        }
    }

    /// (B1-03) The same `conn_id` re-announcing under a DIFFERENT `node_id`
    /// orphans the previous session's map entries — but must NOT close its
    /// channel. `gateway/server/connection/mod.rs` builds exactly one
    /// `with_close(tx, rpc_close.clone())` per connection and stores clones of
    /// it, so every channel reachable through that `conn_id` shares one
    /// `Arc<Notify>`: the "old" session's channel is *this very connection's*.
    /// Closing it tears down the session `register` goes on to insert two lines
    /// later — which is what the code did before 705609899: it closed, then
    /// registered anyway, and the log claimed a survivor that was already being
    /// torn down.
    #[tokio::test]
    async fn reannounce_under_new_node_id_does_not_close_this_connection() {
        // One Notify for the whole connection, exactly as production binds it.
        let close_c = std::sync::Arc::new(tokio::sync::Notify::new());
        let reg = NodeRegistry::new();
        reg.register(session_with_close("node-a", "conn-c", &close_c));
        reg.register(session_with_close("node-b", "conn-c", &close_c));

        let envs = reg.list_environments();
        assert_eq!(
            envs.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["node-b"],
            "the orphaned node-a session must be gone from both maps"
        );
        assert_eq!(
            reg.node_identity_by_conn("conn-c").map(|(id, _)| id),
            Some("node-b".to_string()),
            "the connection must now be stamped as the node it re-announced as"
        );
        assert!(
            !close_fired(&close_c).await,
            "B1-03 must not fire this connection's close signal — it is the same \
             Arc<Notify> the node-b session just registered on top of it depends on"
        );
    }

    /// (B1-01) The contrast to the test above, and the reason the asymmetry is
    /// deliberate rather than an oversight: a reconnect under the same
    /// `node_id` from a DIFFERENT `conn_id` displaces a genuinely different,
    /// still-running connection, and that one MUST be closed. Eviction alone
    /// only stops NEW dispatches — the displaced socket (and with it the live
    /// `node.approval.request` path back to the operator) would survive until
    /// the <=90s inbound watchdog, which never fires while that node keeps
    /// talking, and any surviving `channel.clone()` could still `call()` a
    /// session the registry no longer knows about.
    ///
    /// Same probe as the B1-03 test, opposite answer — and opposite answers
    /// again within this test, on the two connections' own signals.
    #[tokio::test]
    async fn reconnect_from_a_new_conn_closes_the_displaced_connection() {
        let close_1 = std::sync::Arc::new(tokio::sync::Notify::new());
        let close_2 = std::sync::Arc::new(tokio::sync::Notify::new());
        let reg = NodeRegistry::new();
        reg.register(session_with_close("node-a", "conn-1", &close_1));
        reg.register(session_with_close("node-a", "conn-2", &close_2));

        assert_eq!(
            reg.node_identity_by_conn("conn-1"),
            None,
            "the displaced conn must no longer resolve as the node (anti-spoof stamp)"
        );
        assert_eq!(
            reg.node_identity_by_conn("conn-2").map(|(id, _)| id),
            Some("node-a".to_string()),
            "the fresh connection owns the identity"
        );
        assert!(
            close_fired(&close_1).await,
            "B1-01 must fire the displaced connection's close signal, otherwise the \
             evicted session keeps its socket until the inbound idle-watchdog"
        );
        assert!(
            !close_fired(&close_2).await,
            "and only the displaced one: the surviving connection's own signal stays \
             unfired (same probe, both polarities, inside one test)"
        );
    }

    /// A peer may declare arbitrarily large metadata in its connect frame. The
    /// registry stores it verbatim and `list_environments` deep-clones it on
    /// every call (it feeds the model-callable `node_list`), so an unbounded
    /// declaration is a memory-growth and model-budget-displacement lever for
    /// anyone who can reach the gateway. Every bound is applied at the
    /// boundary that owns the data — `maybe_register_node`.
    #[test]
    fn maybe_register_node_bounds_peer_declared_metadata() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let mut tags: Vec<String> = (0..MAX_TAG_COUNT + 5).map(|i| format!("tag-{i}")).collect();
        tags[0] = "t".repeat(MAX_TAG_BYTES + 10);
        let mut commands: Vec<serde_json::Value> = (0..MAX_COMMAND_COUNT + 3)
            .map(|i| json!({"name": format!("cmd-{i}"), "schema": {}}))
            .collect();
        commands[0] = json!({
            "name": "cmd-0",
            "schema": {"blob": "x".repeat(MAX_COMMAND_SCHEMA_BYTES + 100)},
        });
        commands[1] = json!({
            "name": "c".repeat(MAX_COMMAND_NAME_BYTES + 10),
            "schema": {},
        });
        let params = json!({
            "device_name": "n".repeat(MAX_DEVICE_NAME_BYTES + 50),
            "commands": commands,
            "tags": tags,
        });

        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d1",
            "c1",
            Some(&params),
            &ch
        ));

        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        let env = &envs[0];
        assert_eq!(
            env.name.len(),
            MAX_DEVICE_NAME_BYTES,
            "the declared device name must be truncated to the bound"
        );
        assert_eq!(env.tags.len(), MAX_TAG_COUNT, "extra tags must be dropped");
        assert_eq!(
            env.tags[0].len(),
            MAX_TAG_BYTES,
            "an oversized tag must be truncated"
        );
        assert!(
            env.tags.iter().all(|t| t.len() <= MAX_TAG_BYTES),
            "every stored tag must respect the per-tag bound"
        );
        assert_eq!(
            env.commands.len(),
            MAX_COMMAND_COUNT,
            "extra commands must be dropped"
        );
        assert!(
            env.commands.iter().all(|c| c.name.len() <= MAX_COMMAND_NAME_BYTES),
            "every stored command name must respect the bound"
        );
        assert_eq!(
            env.commands[1].name.len(),
            MAX_COMMAND_NAME_BYTES,
            "an oversized command name must be truncated"
        );
        assert_eq!(
            env.commands[0].schema,
            json!({}),
            "an oversized declared schema must be dropped, not stored"
        );
        assert!(
            env.commands[1..]
                .iter()
                .all(|c| serde_json::to_string(&c.schema).is_ok_and(|s| s.len() <= MAX_COMMAND_SCHEMA_BYTES)),
            "no stored schema may exceed the bound"
        );
    }

    /// A malformed `commands` field and an absent one are different facts: the
    /// center must not read "I could not parse what this node declares" as
    /// "this node declares nothing" (the latter keeps the node authoritative).
    #[test]
    fn maybe_register_node_distinguishes_a_malformed_catalog_from_an_empty_one() {
        let reg = NodeRegistry::new();
        // A command entry without its `schema` field makes the WHOLE array
        // fail to deserialize — the peer declared something, the center could
        // not read it.
        let malformed = json!({
            "device_name": "worker-1",
            "commands": [{"name": "bash"}],
            "tags": [],
        });
        let ch = test_channel();
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d1",
            "c1",
            Some(&malformed),
            &ch
        ));
        let (_channel, declared, catalog_malformed) =
            reg.resolve_with_catalog("d1").expect("the node is registered");
        assert!(
            catalog_malformed,
            "a malformed catalog must not read as an empty one"
        );
        assert!(declared.is_empty());

        // The contrast: a node that declared NO commands keeps its documented
        // "the node is authoritative" meaning.
        let ch2 = test_channel();
        let empty = json!({"device_name": "worker-2", "commands": []});
        assert!(maybe_register_node(
            &reg,
            Some("node"),
            "d2",
            "c2",
            Some(&empty),
            &ch2
        ));
        let (_channel, declared, catalog_malformed) =
            reg.resolve_with_catalog("d2").expect("the node is registered");
        assert!(declared.is_empty());
        assert!(
            !catalog_malformed,
            "an empty catalog is not a malformed one"
        );
    }
}
