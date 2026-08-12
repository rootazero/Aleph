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

/// A command declared by a node (name + self-describing schema). 0b does not
/// parse the schema — passed through as-is.
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// No online node matches this name/id.
    NotFound,
    /// Multiple online nodes match — includes readable candidate labels
    /// (`name (short-id)`) for LLM disambiguation.
    Ambiguous(Vec<String>),
    /// Internal state inconsistency: the id returned by match_id is missing from nodes_by_id.
    NodeNotFound { name_or_id: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no online node matches"),
            Self::Ambiguous(c) => write!(f, "ambiguous — matches: {}", c.join(", ")),
            Self::NodeNotFound { name_or_id } => {
                write!(f, "internal node lookup failed for '{name_or_id}'")
            }
        }
    }
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

#[derive(Default)]
struct RegistryInner {
    /// `node_id` → session (authoritative).
    nodes_by_id: HashMap<String, NodeSession>,
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

    /// Register a node session. Reconnect with the same `node_id` → overwrites
    /// the old session, clears the old conn mapping, **and asks the old session's
    /// connection to close** (B1-01). Reusing the same `conn_id` under a
    /// different `node_id` also evicts the colliding session for the same reason
    /// (B1-03).
    pub fn register(&self, session: NodeSession) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let node_id = session.node_id.clone();
        let conn_id = session.conn_id.clone();
        // (B1-03) If this `conn_id` is already mapped to a *different* node_id,
        // that older session is orphaned by the new mapping; drop it the same
        // way `forget` does — close its connection and remove both tables'
        // entries. Same-eviction costs a no-op (we'd be removing the slot we
        // are about to overwrite).
        if let Some(prev_node_id) = inner.nodes_by_conn.get(&conn_id).cloned() {
            if prev_node_id != node_id {
                if let Some(prev) = inner.nodes_by_id.remove(&prev_node_id) {
                    let prev_conn = prev.conn_id.clone();
                    let prev_channel = prev.channel.clone();
                    drop(prev);
                    inner.nodes_by_conn.remove(&prev_conn);
                    // Drop the write lock before signalling: the notified
                    // connection task re-enters the registry via `deregister`.
                    drop(inner);
                    prev_channel.close_connection();
                    tracing::info!(
                        old_node_id = %prev_node_id,
                        new_node_id = %node_id,
                        conn_id = %conn_id,
                        "cluster node connection reused under a different node_id; evicting old session"
                    );
                    inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
                }
            }
        }
        // (B1-01) Reconnect with the same node_id: drop the old session's
        // conn→node mapping AND signal its connection to close. Without the
        // close signal, the dropped session's connection task keeps running,
        // and any `channel.clone()` still alive in another part of the program
        // can still `call()` a session the registry no longer knows about.
        if let Some(prev) = inner.nodes_by_id.get(&node_id) {
            if prev.conn_id != conn_id {
                let prev_conn = prev.conn_id.clone();
                let prev_channel = prev.channel.clone();
                inner.nodes_by_conn.remove(&prev_conn);
                drop(inner);
                tracing::info!(
                    node_id = %node_id,
                    old_conn_id = %prev_conn,
                    new_conn_id = %conn_id,
                    "cluster node reconnected under a fresh connection; closing old"
                );
                prev_channel.close_connection();
                inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            }
        }
        inner.nodes_by_conn.insert(conn_id.clone(), node_id.clone());
        inner.nodes_by_id.insert(node_id.clone(), session);
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
            if entry.get().conn_id == conn_id {
                let removed_name = entry.get().device_name.clone();
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
            .map(|s| Environment {
                id: s.node_id.clone(),
                name: s.device_name.clone(),
                status: "online",
                commands: s.declared_commands.clone(),
                tags: s.tags.clone(),
                connected_at: s.connected_at,
                last_seen_at: None,
                version: s.version.clone(),
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
        let s = inner.nodes_by_id.get(node_id)?;
        Some((s.node_id.clone(), s.device_name.clone()))
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
            let exact: Vec<&NodeSession> = inner
                .nodes_by_id
                .values()
                .filter(|s| normalize_node_key(&s.device_name) == nq)
                .collect();
            match exact.as_slice() {
                [s] => return Ok(s.node_id.clone()),
                [] => {}
                many => return Err(ResolveError::Ambiguous(candidate_labels(many))),
            }
        }
        // ③ Fuzzy: id prefix (≥4 chars, as-is lowercased, avoids 1-char
        //    explosion) or normalized name substring.
        let ql = q.to_ascii_lowercase();
        let fuzzy: Vec<&NodeSession> = inner
            .nodes_by_id
            .values()
            .filter(|s| {
                (q.len() >= 4 && s.node_id.to_ascii_lowercase().starts_with(&ql))
                    || (!nq.is_empty() && normalize_node_key(&s.device_name).contains(&nq))
            })
            .collect();
        match fuzzy.as_slice() {
            [s] => Ok(s.node_id.clone()),
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
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let id = Self::match_id(&inner, name_or_id)?;
        let s = inner
            .nodes_by_id
            .get(&id)
            .ok_or_else(|| ResolveError::NodeNotFound {
                name_or_id: name_or_id.to_string(),
            })?;
        Ok((s.channel.clone(), s.declared_commands.clone()))
    }

    /// Same multi-level match as [`resolve`], but returns only the `node_id` —
    /// `cluster.deregister` uses this to map an operator-supplied name/id to a
    /// unique node identity before evicting + revoking the token.
    pub fn resolve_id(&self, name_or_id: &str) -> std::result::Result<String, ResolveError> {
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
            .filter(|s| tags.iter().all(|t| s.tags.contains(t)))
            .map(|s| NodeMatch {
                node_id: s.node_id.clone(),
                name: s.device_name.clone(),
                channel: s.channel.clone(),
                declared_commands: s.declared_commands.clone(),
                tags: s.tags.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.node_id.cmp(&b.node_id).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// Actively evict a session by `node_id` (used by operator deregister).
    /// Removes from both tables **and asks the node's connection to close**.
    /// Returns whether a session was actually removed. Orthogonal to
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
    pub fn forget(&self, node_id: &str) -> bool {
        // Drop the write lock before signalling: the notified connection task
        // runs cleanup that re-enters the registry (`deregister`).
        let removed = {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            let session = inner.nodes_by_id.remove(node_id);
            if let Some(s) = &session {
                inner.nodes_by_conn.remove(&s.conn_id);
            }
            session
        };
        match removed {
            Some(s) => {
                tracing::info!(node_id = %node_id, name = %s.device_name, "cluster node session evicted (operator forgot/forget)");
                s.channel.close_connection();
                true
            }
            None => false,
        }
    }
}

/// Render candidate sessions as human-readable labels `name (short-id)` for
/// ambiguity errors. The short-id is the first 8 chars — enough to
/// disambiguate without noise. Results are sorted for stable error messages
/// (easier test/log comparison).
fn candidate_labels(sessions: &[&NodeSession]) -> Vec<String> {
    let mut labels: Vec<String> = sessions
        .iter()
        .map(|s| {
            let short = truncate_on_char_boundary(&s.node_id, 8);
            format!("{} ({})", s.device_name, short)
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
/// Extracted as a pure function for unit testing and to keep `handler.rs` thin.
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
    let device_name = match params
        .and_then(|p| p.get("device_name"))
        .and_then(|v| v.as_str())
    {
        Some(s) => s.to_string(),
        None => {
            tracing::warn!(
                device_id = %device_id,
                conn_id = %conn_id,
                "cluster node connect frame omitted device_name; falling back to \"unknown\""
            );
            "unknown".to_string()
        }
    };
    // (B1-02) Parse failures used to silently downgrade to an empty list. A
    // node with no declared commands is registered as "online but every
    // command denied" — confusing for the operator, and lets a peer hold
    // fleet slots with malformed frames. Log and downgrade; only `commands`
    // is gating (every node ships `bash`), so an empty commands list keeps
    // the registration but is loud about it.
    let declared_commands: Vec<CommandDescriptor> = match params
        .and_then(|p| p.get("commands"))
        .map(|v| serde_json::from_value::<Vec<CommandDescriptor>>(v.clone()))
    {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            tracing::warn!(
                device_id = %device_id,
                node = %device_name,
                error = %e,
                "cluster node connect frame carried malformed commands; registering with empty catalog"
            );
            Vec::new()
        }
        None => Vec::new(),
    };
    let tags: Vec<String> = match params
        .and_then(|p| p.get("tags"))
        .map(|v| serde_json::from_value::<Vec<String>>(v.clone()))
    {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            tracing::warn!(
                device_id = %device_id,
                node = %device_name,
                error = %e,
                "cluster node connect frame carried malformed tags; registering with empty tag list"
            );
            Vec::new()
        }
        None => Vec::new(),
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
    registry.register(NodeSession {
        node_id: device_id.to_string(),
        conn_id: conn_id.to_string(),
        device_name,
        channel: channel.clone(),
        declared_commands,
        tags,
        version,
        connected_at: now_unix(),
    });
    true
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
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
        assert!(reg.forget("n-1"));
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
        assert!(reg.forget("node-a"));
        assert!(reg.list_environments().is_empty());
        assert!(reg.resolve("node-a").is_err());
        // A stale conn cleanup after forget is a harmless no-op.
        assert!(!reg.deregister("conn-1"));
        // Forgetting an unknown id reports nothing removed.
        assert!(!reg.forget("ghost"));
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
}
