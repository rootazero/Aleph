//! Owner-scoped WS event delivery (P1 data isolation, spec §5.4).
//!
//! Sibling of `visibility.rs` for the event-bus fan-out path rather than RPC
//! responses: `EventScopeGuard` (filter #1 in `server::handler`'s
//! `should_forward` chain) is role-based — it gates a handful of admin-only
//! topic prefixes and is default-**allow** for everything else, including
//! every ordinary session/chat/agent-run event. So today every connected
//! member receives every OTHER user's live run stream — the event-bus
//! analogue of the pre-P1 `sessions.list`/`sessions.history` gap `visibility.rs`
//! closed for RPCs. This module is the 4th filter term that closes it for
//! events.
//!
//! ## Why a run→session index
//!
//! Most agent-run frames (`AgentTrace`, `ToolStart`, `RunComplete`, …) carry
//! only `run_id` — the session key that owns them isn't in the payload.
//! `RunAccepted` is the one frame that carries both `{run_id, session_key}`,
//! emitted once at the start of a run, before any other frame for that
//! `run_id`. [`EventVisibilityIndex`] caches that pairing (seeded by
//! [`EventVisibilityIndex::note_frame`], called unconditionally in the
//! delivery loop before filtering) so every later same-run frame resolves to
//! a session key with no extra store round-trip, and evicts the pairing when
//! the run ends (`RunComplete`/`RunError`) — mirroring the capacity-capped,
//! insertion-order-evicting hygiene `streaming/relay.rs`'s `StreamRegistry`
//! already established for a similar per-run cache.
//!
//! Session-owner lookups (`session_key` → the owning user) go through a
//! second bounded cache, filled on miss from the [`SessionStore`] via
//! `visibility::effective_owner` — the SAME owner-resolution rule RPCs use,
//! not a second one (spec §5.4's single-authority requirement extends here).
//!
//! ## Deliberately `Global`, not owner-scoped
//!
//! `approval.*`, `surface.approval`, `pairing.*`, `config.changed` all carry
//! (or could carry) a `session_key`, but this module does NOT additionally
//! owner-scope them: they are already role-gated by `EventScopeGuard` (filter
//! #1), and an exec approval for a MEMBER's session is resolved by an
//! OPERATOR — a naive owner-equality check would deny the operator delivery
//! of a member's approval card, breaking the one workflow that exists to let
//! an admin act on a non-owned session's behalf. `RunningSetChanged` carries a
//! `Vec<String>` spanning every user's in-flight sessions with no single
//! owner to check against; it stays `Global` (session KEYS only, no message
//! content — matches its pre-P1 unfiltered behavior as the sidebar red-dot
//! signal). See [`session_identity_of`]'s doc and the
//! `every_frame_variant_is_classified` pin test for the full, reviewed list.
//!
//! ## Known gap: `RunningSetChanged` leaks cross-user session keys (fix round 1)
//!
//! Review flagged that any member currently sees every OTHER user's active
//! `session_key`s via `stream.running_set_changed` — not org-public, not
//! guarded by `EventScopeGuard`. The obvious fix (add its topic to
//! `EventScopeGuard::default_rules()`'s guarded set, operator-only) was
//! checked against actual Panel consumption first, per this task's own
//! "verify before guessing" discipline, and REJECTED: `interfaces/webchat/
//! src/state/sessions.rs::SessionMap::server_running` is documented as "the
//! SOLE input source for the red dot — purely server-authoritative, client
//! refcounts are not consulted," fed exclusively by this event
//! (`components/chat_sidebar.rs:480`). Gating it operator-only would silently
//! break every MEMBER's OWN sidebar running-indicator for their OWN sessions
//! — the opposite of a P1 isolation fix. Left `Global`, unfixed, and
//! recorded here (not silently dropped) pending a real fix: per-connection
//! payload projection (filter `running` down to the receiving connection's
//! visible session keys before send) needs a payload-REWRITE step this
//! module's boolean `event_admits` doesn't have — the delivery loop's
//! `should_forward` is pass/fail only, never rewrites the wire bytes it
//! forwards. That is new infrastructure, not a term in this filter chain,
//! and is out of scope for this task.
//!
//! ## Fail-closed
//!
//! A `caller_user: None` connection (walled — the login wall already refused
//! it) is denied for any resolvable identity, as defense in depth. An
//! unresolvable `run_id` (cache miss — the event raced ahead of
//! `RunAccepted`, or predates this filter) is denied: a dropped early frame
//! self-heals via `run_complete`'s summary reconciliation on the client side,
//! but a leaked frame cannot be un-leaked.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;
use tokio::sync::RwLock;

use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility::effective_owner;
use crate::sync_primitives::Arc;

/// Which session (if any) a delivered event frame is attributable to, keyed
/// off the SAME wire strings `server::handler`'s filter chain already
/// extracts (`topic` for `TopicEvent`-form frames, `method` for `stream.*`
/// JSON-RPC notification frames — see `event_bus.rs::publish_frame`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionIdentity {
    /// The frame's own payload names its session directly.
    BySessionKey(String),
    /// The frame names only a `run_id`; the session must be resolved through
    /// [`EventVisibilityIndex`]'s run→session cache.
    ByRunId(String),
    /// Unattributable to any one session — org-level infrastructure, or
    /// already covered by a different gate (see module doc).
    Global,
}

fn str_field(data: Option<&Value>, field: &str) -> Option<String> {
    data.and_then(|d| d.get(field))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `run.subagent_tree` (`subagent_tree_relay.rs`, republishing
/// `aleph_protocol::subagent_tree::SubagentTreeEvent`) is NOT a
/// `GatewayEventFrame` variant — a different producer entirely, published
/// via a hand-built `TopicEvent::to_notification()` rather than
/// `publish_frame` — so it sits outside `every_frame_variant_is_classified`'s
/// exhaustive match; covered by its own tests instead (fix round 1).
///
/// Its session key (`root_session`, a field of `SubagentNode`) sits at a
/// variant-dependent nesting depth: `Progress`/`Settled` carry it at the
/// top level of the event payload, but `Spawned { node: SubagentNode }`
/// nests it one level deeper, under `node`. Check both positions.
fn subagent_tree_root_session(data: Option<&Value>) -> Option<String> {
    let d = data?;
    str_field(Some(d), "root_session").or_else(|| str_field(d.get("node"), "root_session"))
}

/// Classify a delivered event frame's session identity from its wire
/// `topic`/`method` string and payload.
///
/// **This match must stay reviewed, not just exhaustive.** The runtime
/// signature is string-based (it reads the wire form, not
/// `GatewayEventFrame` directly) so it cannot itself force a compile error
/// when a new frame variant is added — that guarantee lives in this module's
/// `every_frame_variant_is_classified` test, which matches on the real enum
/// with no wildcard arm. Whoever adds a variant there is the one who must
/// decide its classification; a session-scoped variant that lands here as
/// `Global` by omission is a data leak, not a missing feature.
///
/// The catch-all default for an unrecognized topic string is `Global`
/// (fail-open at classification) — matching `EventScopeGuard::can_receive`'s
/// own "no rule matched → unguarded" default, so a topic not yet reviewed
/// here keeps exactly its pre-Task-8 delivery behavior instead of a novel
/// denial.
#[must_use]
pub fn session_identity_of(topic: &str, data: Option<&Value>) -> SessionIdentity {
    match topic {
        // --- stream.* frames that carry their session key directly ---
        "stream.run_accepted"
        | "stream.ask_user"
        | "stream.clarification_ended"
        | "stream.session_updated" => match str_field(data, "session_key") {
            Some(k) => SessionIdentity::BySessionKey(k),
            None => SessionIdentity::Global,
        },

        // --- stream.* frames correlated only by run_id ---
        "stream.reasoning"
        | "stream.tool_start"
        | "stream.tool_update"
        | "stream.tool_end"
        | "stream.agent_trace"
        | "stream.response_chunk"
        | "stream.context_gauge"
        | "stream.run_complete"
        | "stream.run_error"
        | "stream.reasoning_block"
        | "stream.uncertainty_signal"
        | "stream.model_resolved"
        | "stream.run_retrying" => match str_field(data, "run_id") {
            Some(r) => SessionIdentity::ByRunId(r),
            None => SessionIdentity::Global,
        },

        // Broadcast red-dot spanning every owner's running sessions — see
        // module doc "Deliberately Global".
        "stream.running_set_changed" => SessionIdentity::Global,

        // Not a `GatewayEventFrame` variant — republished by
        // `subagent_tree_relay.rs` via a hand-built
        // `TopicEvent::to_notification()`. Genuinely session-scoped (a live
        // per-run subagent tree is exactly as cross-user-sensitive as
        // `stream.agent_trace`) and was previously unreachable here at all:
        // the double-nested `{"method":"event","params":{"topic":...}}`
        // envelope this producer uses read as topic `"event"` before the
        // `extract_topic_and_data` fix (fix round 1) — see that function's
        // doc in `server::handler`.
        "run.subagent_tree" => match subagent_tree_root_session(data) {
            Some(k) => SessionIdentity::BySessionKey(k),
            None => SessionIdentity::Global,
        },

        // --- TopicEvent-form frames genuinely session-scoped and NOT
        // covered by any other filter today ---
        "session.lifecycle.changed" | "sessions.changed" => {
            match str_field(data, "session_key") {
                Some(k) => SessionIdentity::BySessionKey(k),
                None => SessionIdentity::Global,
            }
        }

        // --- TopicEvent-form frames already role-gated by EventScopeGuard —
        // see module doc "Deliberately Global" for why these are not ALSO
        // owner-scoped despite carrying a session_key. ---
        "approval.requested" | "approval.resolved" | "approval.expired" | "surface.approval"
        | "pairing.requested" | "pairing.completed" | "config.changed" => SessionIdentity::Global,

        // --- TopicEvent-form frames with no session concept at all ---
        "channel.message"
        | "channel.typing"
        | "channel.status"
        | "channel.error"
        | "acp.sessions.changed"
        | "gateway.token.rotated"
        | "gateway.device.revoked"
        | "cron.job.changed"
        | "heartbeat.task.changed"
        | "team.changed"
        | "surface.notify" => SessionIdentity::Global,

        // Unrecognized topic: fail open at classification (see doc above).
        _ => SessionIdentity::Global,
    }
}

/// Mirrors `streaming/relay.rs`'s `StreamRegistry` hygiene: a hard capacity
/// cap plus insertion-order (FIFO) eviction, so a long-uptime process with
/// many runs/sessions never grows either cache unbounded.
const MAX_TRACKED_RUNS: usize = 4096;
const MAX_CACHED_SESSION_OWNERS: usize = 4096;

#[derive(Default)]
struct RunIndex {
    order: VecDeque<String>,
    map: HashMap<String, String>,
}

#[derive(Default)]
struct OwnerCache {
    order: VecDeque<String>,
    map: HashMap<String, Option<String>>,
}

/// Process-shared (via `GatewaySharedState`/`ConnectionContext`, one
/// instance for the whole gateway) run→session seed plus session→owner
/// cache backing [`session_identity_of`]'s `ByRunId`/`BySessionKey`
/// resolution. See the module doc for the full design rationale.
#[derive(Default)]
pub struct EventVisibilityIndex {
    runs: RwLock<RunIndex>,
    owners: RwLock<OwnerCache>,
}

impl EventVisibilityIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed or evict the run→session cache from a delivered frame. Called
    /// UNCONDITIONALLY (before filtering) on every connection's delivery
    /// loop, so the shared index stays warm regardless of which connection
    /// happens to process a given `RunAccepted`/`RunComplete`/`RunError`
    /// first — first writer wins, and re-seeding an already-known run_id is
    /// harmless (same session_key every time for a given run).
    pub async fn note_frame(&self, topic: &str, data: Option<&Value>) {
        match topic {
            "stream.run_accepted" => {
                let (Some(run_id), Some(session_key)) =
                    (str_field(data, "run_id"), str_field(data, "session_key"))
                else {
                    return;
                };
                self.insert_run(run_id, session_key).await;
            }
            "stream.run_complete" | "stream.run_error" => {
                if let Some(run_id) = str_field(data, "run_id") {
                    self.evict_run(&run_id).await;
                }
            }
            _ => {}
        }
    }

    /// Whether `caller_user` may receive an event classified by `topic`/`data`.
    /// See the module doc for the full fail-closed/`Global` rationale.
    pub async fn event_admits(
        &self,
        topic: &str,
        data: Option<&Value>,
        caller_user: Option<&str>,
        store: &Arc<dyn SessionStore>,
    ) -> bool {
        match session_identity_of(topic, data) {
            SessionIdentity::Global => true,
            SessionIdentity::BySessionKey(session_key) => {
                let Some(caller) = caller_user else {
                    return false;
                };
                self.owner_matches(&session_key, caller, store).await
            }
            SessionIdentity::ByRunId(run_id) => {
                let Some(caller) = caller_user else {
                    return false;
                };
                let Some(session_key) = self.session_key_for_run(&run_id).await else {
                    return false; // unresolvable — fail closed (see module doc)
                };
                self.owner_matches(&session_key, caller, store).await
            }
        }
    }

    async fn insert_run(&self, run_id: String, session_key: String) {
        let mut inner = self.runs.write().await;
        if !inner.map.contains_key(&run_id) {
            inner.order.push_back(run_id.clone());
        }
        inner.map.insert(run_id, session_key);
        while inner.map.len() > MAX_TRACKED_RUNS {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            inner.map.remove(&oldest);
        }
    }

    async fn evict_run(&self, run_id: &str) {
        let mut inner = self.runs.write().await;
        inner.map.remove(run_id);
        inner.order.retain(|r| r != run_id);
    }

    async fn session_key_for_run(&self, run_id: &str) -> Option<String> {
        self.runs.read().await.map.get(run_id).cloned()
    }

    /// Owner-equality check for a resolved session key, through the bounded
    /// cache — fill-on-miss from `store`, via the SAME `effective_owner`
    /// rule `visibility.rs`'s RPC-side predicates use (single authority,
    /// spec §5.4).
    async fn owner_matches(
        &self,
        session_key: &str,
        caller: &str,
        store: &Arc<dyn SessionStore>,
    ) -> bool {
        if let Some(cached) = {
            let inner = self.owners.read().await;
            inner.map.get(session_key).cloned()
        } {
            return cached.as_deref() == Some(caller);
        }

        let owner = match SessionKey::from_key_string(session_key) {
            Some(key) => match store.get_metadata(&key).await {
                Ok(Some(meta)) => Some(effective_owner(&meta).to_string()),
                // Row absent: TRANSIENT, exactly like the store error below,
                // and for a reason that fires on the happy path of a brand-new
                // conversation. `execute.rs` emits `RunAccepted{session_key}`
                // BEFORE `ensure_session` creates the row, so the very first
                // frame of a fresh session can arrive while the row does not
                // exist yet. Caching that absence as `owner: None` would deny
                // EVERY later frame for that session key — the cache has no
                // invalidation and evicts only by FIFO at
                // `MAX_CACHED_SESSION_OWNERS` — so streaming for that
                // conversation would stay dead for the process lifetime. It
                // fails closed, so nothing leaks, but it dies silently, and
                // loopback resolves to `Some(OWNER_USER_ID)` so a single-user
                // box runs this path too.
                //
                // Deny THIS frame and re-resolve on the next one (a dropped
                // early frame self-heals via `run_complete`'s summary
                // reconciliation on the client — see the module doc).
                Ok(None) => return false,
                // Store error: fail closed, and don't cache a transient
                // failure as a permanent "no owner" — matching
                // `visibility::existing_session_is_visible`'s own rule.
                Err(_) => return false,
            },
            // Malformed session_key string: cache as "no owner" so a
            // repeated malformed key doesn't re-hit the parse on every event.
            // This one IS permanent — a string that does not parse today will
            // not parse later, so nothing can invalidate it.
            None => None,
        };
        self.cache_owner(session_key.to_string(), owner.clone())
            .await;
        owner.as_deref() == Some(caller)
    }

    async fn cache_owner(&self, session_key: String, owner: Option<String>) {
        let mut inner = self.owners.write().await;
        if !inner.map.contains_key(&session_key) {
            inner.order.push_back(session_key.clone());
        }
        inner.map.insert(session_key, owner);
        while inner.map.len() > MAX_CACHED_SESSION_OWNERS {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            inner.map.remove(&oldest);
        }
    }

    #[cfg(test)]
    async fn tracked_run_count(&self) -> usize {
        self.runs.read().await.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::ConversationId;
    use crate::gateway::event_emitter::{
        ConfidenceLevel, ReasoningStepType, RunSummary, ToolResult, UncertaintyAction,
    };
    use crate::gateway::events::frame::{
        ChangeKind, ClarificationOutcome, GatewayEventFrame, InboundMessagePayload, MessageSender,
    };
    use crate::gateway::security::store::OWNER_USER_ID;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::{ChannelId, ChannelStatus};
    use crate::providers::health::ModelInfo;
    use tempfile::TempDir;

    fn test_store() -> (FileSessionStore, TempDir) {
        let temp = TempDir::new().unwrap();
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        (store, temp)
    }

    async fn stamp_owner(store: &FileSessionStore, key: &SessionKey, owner: &str) {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal(owner)),
            store.get_or_create(key),
        )
        .await
        .unwrap();
    }

    /// The brief's own worked example: `RunAccepted{run_id: r1, session_key:
    /// K(owner alice)}` seeds the index; a later `AgentTrace{run_id: r1}`
    /// resolves through it — alice admits, bob doesn't, and neither does the
    /// operator (owner-by-absence only covers LEGACY rows with no stamped
    /// owner; this row has one, and it isn't the operator's).
    #[tokio::test]
    async fn run_events_are_owner_scoped_via_the_run_accepted_seed() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-1");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let index = EventVisibilityIndex::new();
        let accepted = serde_json::json!({
            "run_id": "r1",
            "session_key": key.to_key_string(),
            "accepted_at": "t",
        });
        index
            .note_frame("stream.run_accepted", Some(&accepted))
            .await;

        let trace = serde_json::json!({
            "run_id": "r1",
            "seq": 1,
            "event": {"kind": "turn_started", "iteration": 1},
        });
        assert!(
            index
                .event_admits("stream.agent_trace", Some(&trace), Some("alice"), &store)
                .await
        );
        assert!(
            !index
                .event_admits("stream.agent_trace", Some(&trace), Some("bob"), &store)
                .await
        );
        assert!(
            !index
                .event_admits(
                    "stream.agent_trace",
                    Some(&trace),
                    Some(OWNER_USER_ID),
                    &store
                )
                .await,
            "the operator is not exempt from session ownership — see visibility.rs's \
             same rule for RPCs"
        );
    }

    /// The final review's I4. `RunAccepted{session_key}` is emitted before
    /// `ensure_session` creates the row, so the first frame of a brand-new
    /// conversation can lose that race. The frame that loses it must be
    /// denied — but the DENIAL must not be cached, or every later frame for
    /// that session dies too, for the process lifetime, silently.
    #[tokio::test]
    async fn an_absent_session_row_is_transient_not_a_cached_denial() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-racing");
        let key_str = key.to_key_string();
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let index = EventVisibilityIndex::new();
        index
            .note_frame(
                "stream.run_accepted",
                Some(&serde_json::json!({
                    "run_id": "r-race",
                    "session_key": key_str,
                    "accepted_at": "t",
                })),
            )
            .await;
        let trace = serde_json::json!({
            "run_id": "r-race",
            "seq": 1,
            "event": {"kind": "turn_started", "iteration": 1},
        });

        // The row does not exist yet: this frame is denied (fail closed).
        assert!(
            !index
                .event_admits("stream.agent_trace", Some(&trace), Some("alice"), &store)
                .await
        );

        // `ensure_session` lands, stamping alice as the owner.
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("alice")),
            store.get_or_create(&key),
        )
        .await
        .unwrap();

        // A LATER frame for the same session must now be admitted. Before the
        // fix this stayed false forever — the `Ok(None)` had been cached as
        // `owner: None` with nothing to invalidate it.
        assert!(
            index
                .event_admits("stream.agent_trace", Some(&trace), Some("alice"), &store)
                .await,
            "an absent row must be re-resolved on the next frame, not cached as a denial"
        );
        // ...and the re-resolution is a real one, not a blanket allow.
        assert!(
            !index
                .event_admits("stream.agent_trace", Some(&trace), Some("bob"), &store)
                .await
        );
    }

    #[tokio::test]
    async fn unseeded_run_id_denies() {
        let (store, _temp) = test_store();
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        let trace = serde_json::json!({
            "run_id": "never-seeded",
            "seq": 1,
            "event": {"kind": "turn_started", "iteration": 1},
        });
        assert!(
            !index
                .event_admits("stream.agent_trace", Some(&trace), Some("alice"), &store)
                .await,
            "a run_id with no RunAccepted seed must fail closed"
        );
    }

    #[tokio::test]
    async fn session_key_bearing_topic_events_are_owner_scoped() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-2");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        let data = serde_json::json!({
            "session_key": key.to_key_string(),
            "old_state": null,
            "new_state": "active",
            "reason": null,
        });
        assert!(
            index
                .event_admits(
                    "session.lifecycle.changed",
                    Some(&data),
                    Some("alice"),
                    &store
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    "session.lifecycle.changed",
                    Some(&data),
                    Some("bob"),
                    &store
                )
                .await
        );
    }

    /// `run.subagent_tree`'s `Progress`/`Settled` shapes carry `root_session`
    /// at the top level of the payload — the common case.
    #[tokio::test]
    async fn subagent_tree_progress_and_settled_are_owner_scoped_by_flat_root_session() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-3");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        let progress = serde_json::json!({
            "kind": "progress",
            "node_id": "n1",
            "root_session": key.to_key_string(),
            "step": 1,
            "activity": "tool_called",
            "tool_name": "bash",
            "tool_count": 1,
        });
        assert!(
            index
                .event_admits("run.subagent_tree", Some(&progress), Some("alice"), &store)
                .await
        );
        assert!(
            !index
                .event_admits("run.subagent_tree", Some(&progress), Some("bob"), &store)
                .await
        );

        let settled = serde_json::json!({
            "kind": "settled",
            "node_id": "n1",
            "root_session": key.to_key_string(),
            "lifecycle": "completed",
            "duration_ms": 100,
            "iterations": 1,
            "tool_calls_made": 1,
            "total_tokens": 10,
        });
        assert!(
            index
                .event_admits("run.subagent_tree", Some(&settled), Some("alice"), &store)
                .await
        );
        assert!(
            !index
                .event_admits("run.subagent_tree", Some(&settled), Some("bob"), &store)
                .await
        );
    }

    /// `run.subagent_tree`'s `Spawned { node: SubagentNode }` shape nests
    /// `root_session` one level deeper, under `node` — the tricky case that
    /// a flat `str_field(data, "root_session")` lookup alone would miss.
    #[tokio::test]
    async fn subagent_tree_spawned_is_owner_scoped_by_nested_root_session() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-4");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        let spawned = serde_json::json!({
            "kind": "spawned",
            "node": {
                "node_id": "n1",
                "parent_id": null,
                "depth": 1,
                "root_session": key.to_key_string(),
                "task": "t",
                "model": null,
                "lifecycle": "running",
                "started_at_ms": 0,
                "elapsed_ms": 0,
                "tool_count": 0,
                "last_tool": null,
                "last_activity": null,
            },
        });
        assert!(
            index
                .event_admits("run.subagent_tree", Some(&spawned), Some("alice"), &store)
                .await
        );
        assert!(
            !index
                .event_admits("run.subagent_tree", Some(&spawned), Some("bob"), &store)
                .await
        );
    }

    #[tokio::test]
    async fn sessions_changed_topic_is_owner_scoped() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-sessions-changed");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        let data = serde_json::json!({
            "session_key": key.to_key_string(),
            "label": "alice-secret",
            "channel": "telegram",
        });
        assert!(
            index
                .event_admits("sessions.changed", Some(&data), Some("alice"), &store)
                .await
        );
        assert!(
            !index
                .event_admits("sessions.changed", Some(&data), Some("bob"), &store)
                .await
        );
    }

    #[tokio::test]
    async fn global_topics_pass_for_everyone() {
        let (store, _temp) = test_store();
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        for caller in [Some("alice"), Some("bob"), None] {
            assert!(
                index
                    .event_admits("tools.changed", None, caller, &store)
                    .await,
                "an unattributable topic must pass for {caller:?}"
            );
        }
    }

    /// The exhaustive, compile-anchored review pin: one instance of every
    /// `GatewayEventFrame` variant, matched with no wildcard arm. Adding a
    /// new variant to the enum breaks this match — the reviewer adding it
    /// must decide (and justify, per this module's doc) its
    /// `SessionIdentity`, rather than a new variant silently defaulting to
    /// `Global` through `session_identity_of`'s string catch-all.
    #[test]
    fn every_frame_variant_is_classified() {
        fn expected(frame: &GatewayEventFrame) -> SessionIdentity {
            match frame {
                GatewayEventFrame::RunAccepted { session_key, .. } => {
                    SessionIdentity::BySessionKey(session_key.clone())
                }
                GatewayEventFrame::Reasoning { run_id, .. }
                | GatewayEventFrame::ToolStart { run_id, .. }
                | GatewayEventFrame::ToolUpdate { run_id, .. }
                | GatewayEventFrame::ToolEnd { run_id, .. }
                | GatewayEventFrame::AgentTrace { run_id, .. }
                | GatewayEventFrame::ResponseChunk { run_id, .. }
                | GatewayEventFrame::ContextGauge { run_id, .. }
                | GatewayEventFrame::RunComplete { run_id, .. }
                | GatewayEventFrame::RunError { run_id, .. }
                | GatewayEventFrame::ReasoningBlock { run_id, .. }
                | GatewayEventFrame::UncertaintySignal { run_id, .. }
                | GatewayEventFrame::ModelResolved { run_id, .. }
                | GatewayEventFrame::RunRetrying { run_id, .. } => {
                    SessionIdentity::ByRunId(run_id.clone())
                }
                // Carries session_key directly — routed by session, not run
                // (see the frame's own doc comment).
                GatewayEventFrame::AskUser { session_key, .. }
                | GatewayEventFrame::ClarificationEnded { session_key, .. }
                | GatewayEventFrame::SessionUpdated { session_key, .. } => {
                    SessionIdentity::BySessionKey(session_key.clone())
                }
                // Broadcast red-dot spanning every owner's sessions — no
                // single resolvable owner. See module doc.
                GatewayEventFrame::RunningSetChanged { .. } => SessionIdentity::Global,
                GatewayEventFrame::ChannelMessage { .. }
                | GatewayEventFrame::ChannelTyping { .. }
                | GatewayEventFrame::ChannelStatusChanged { .. }
                | GatewayEventFrame::ChannelError { .. }
                | GatewayEventFrame::ConfigChanged { .. }
                | GatewayEventFrame::PairingRequested { .. }
                | GatewayEventFrame::PairingCompleted { .. } => SessionIdentity::Global,
                // Already role-gated by EventScopeGuard; deliberately not
                // ALSO owner-scoped (module doc "Deliberately Global" —
                // operator-resolves-a-member's-approval workflow).
                GatewayEventFrame::ApprovalRequested { .. }
                | GatewayEventFrame::ApprovalResolved { .. }
                | GatewayEventFrame::ApprovalExpired { .. } => SessionIdentity::Global,
                GatewayEventFrame::SessionLifecycleChanged { session_key, .. } => {
                    SessionIdentity::BySessionKey(session_key.clone())
                }
                GatewayEventFrame::AcpSessionsChanged
                | GatewayEventFrame::TokenRotated
                | GatewayEventFrame::DeviceRevoked { .. }
                | GatewayEventFrame::CronJobChanged { .. }
                | GatewayEventFrame::HeartbeatTaskChanged { .. }
                | GatewayEventFrame::TeamChanged { .. }
                | GatewayEventFrame::SurfaceNotify { .. }
                | GatewayEventFrame::SurfaceApproval { .. } => SessionIdentity::Global,
            }
        }

        let samples: Vec<GatewayEventFrame> = vec![
            GatewayEventFrame::RunAccepted {
                run_id: "r1".into(),
                session_key: "agent:main:main".into(),
                accepted_at: "t".into(),
            },
            GatewayEventFrame::Reasoning {
                run_id: "r1".into(),
                seq: 1,
                content: "c".into(),
                is_complete: false,
            },
            GatewayEventFrame::ToolStart {
                run_id: "r1".into(),
                seq: 1,
                tool_name: "bash".into(),
                tool_id: "t1".into(),
                params: serde_json::json!({}),
            },
            GatewayEventFrame::ToolUpdate {
                run_id: "r1".into(),
                seq: 1,
                tool_id: "t1".into(),
                progress: "p".into(),
            },
            GatewayEventFrame::ToolEnd {
                run_id: "r1".into(),
                seq: 1,
                tool_id: "t1".into(),
                result: ToolResult::error("x"),
                duration_ms: 1,
            },
            GatewayEventFrame::AgentTrace {
                run_id: "r1".into(),
                seq: 1,
                event: aleph_protocol::AgentTraceEvent::TurnStarted { iteration: 1 },
            },
            GatewayEventFrame::ResponseChunk {
                run_id: "r1".into(),
                seq: 1,
                delta: "d".into(),
                full_text: "d".into(),
                content: "d".into(),
                chunk_index: 0,
                is_final: false,
                is_intermediate: false,
            },
            GatewayEventFrame::ContextGauge {
                run_id: "r1".into(),
                seq: 1,
                context_tokens: 1,
                context_window: 2,
                total_tokens: 3,
            },
            GatewayEventFrame::RunComplete {
                run_id: "r1".into(),
                seq: 1,
                summary: RunSummary::default(),
                total_duration_ms: 1,
            },
            GatewayEventFrame::RunError {
                run_id: "r1".into(),
                seq: 1,
                error: "e".into(),
                error_code: None,
            },
            GatewayEventFrame::AskUser {
                run_id: "r1".into(),
                seq: 1,
                session_key: "agent:main:main".into(),
                question: "q".into(),
                options: vec![],
            },
            GatewayEventFrame::ClarificationEnded {
                session_key: "agent:main:main".into(),
                outcome: ClarificationOutcome::Resolved,
            },
            GatewayEventFrame::ReasoningBlock {
                run_id: "r1".into(),
                seq: 1,
                step_type: ReasoningStepType::Observation,
                label: "l".into(),
                content: "c".into(),
                confidence: Some(ConfidenceLevel::High),
                is_final: false,
            },
            GatewayEventFrame::UncertaintySignal {
                run_id: "r1".into(),
                seq: 1,
                uncertainty: "u".into(),
                suggested_action: UncertaintyAction::ProceedWithCaution,
            },
            GatewayEventFrame::ModelResolved {
                run_id: "r1".into(),
                model_info: ModelInfo {
                    model: "m".into(),
                    provider: "p".into(),
                    is_fallback: false,
                    original_model: None,
                },
            },
            GatewayEventFrame::RunRetrying {
                run_id: "r1".into(),
                seq: 1,
                provider: "p".into(),
                attempt: 1,
                max_attempts: 3,
                reason: "r".into(),
            },
            GatewayEventFrame::SessionUpdated {
                session_key: "agent:main:main".into(),
                origin_channel: None,
            },
            GatewayEventFrame::RunningSetChanged {
                seq: 1,
                running: vec!["agent:main:main".into()],
            },
            GatewayEventFrame::ChannelMessage {
                channel_id: ChannelId::new("c1"),
                conversation_id: ConversationId::new("conv-1"),
                message: InboundMessagePayload {
                    text: "hi".into(),
                    sender: MessageSender {
                        id: "u1".into(),
                        name: "n".into(),
                        avatar_url: None,
                    },
                },
            },
            GatewayEventFrame::ChannelTyping {
                channel_id: ChannelId::new("c1"),
                conversation_id: ConversationId::new("conv-1"),
            },
            GatewayEventFrame::ChannelStatusChanged {
                channel_id: ChannelId::new("c1"),
                status: ChannelStatus::Connected,
            },
            GatewayEventFrame::ChannelError {
                channel_id: ChannelId::new("c1"),
                error: "e".into(),
            },
            GatewayEventFrame::ConfigChanged {
                section: None,
                value: serde_json::json!({}),
            },
            GatewayEventFrame::PairingRequested {
                device_name: "d".into(),
            },
            GatewayEventFrame::PairingCompleted {
                device_id: "d1".into(),
            },
            GatewayEventFrame::ApprovalRequested {
                approval_id: "a1".into(),
                session_key: "agent:main:main".into(),
                channel_id: String::new(),
                conversation_id: String::new(),
                tool_call_id: None,
            },
            GatewayEventFrame::ApprovalResolved {
                approval_id: "a1".into(),
                session_key: "agent:main:main".into(),
                decision: crate::exec::socket::ApprovalDecisionType::AllowOnce,
                resolved_by: None,
            },
            GatewayEventFrame::ApprovalExpired {
                approval_id: "a1".into(),
                session_key: "agent:main:main".into(),
            },
            GatewayEventFrame::SessionLifecycleChanged {
                session_key: "agent:main:main".into(),
                old_state: None,
                new_state: "s".into(),
                reason: None,
            },
            GatewayEventFrame::AcpSessionsChanged,
            GatewayEventFrame::TokenRotated,
            GatewayEventFrame::DeviceRevoked {
                device_id: "d1".into(),
            },
            GatewayEventFrame::CronJobChanged {
                job_id: "j1".into(),
                change: ChangeKind::Updated,
            },
            GatewayEventFrame::HeartbeatTaskChanged {
                task_id: "t1".into(),
                change: ChangeKind::Updated,
            },
            GatewayEventFrame::TeamChanged {
                team_id: "t1".into(),
                change: ChangeKind::Updated,
            },
            GatewayEventFrame::SurfaceNotify {
                audience: vec!["desktop".into()],
                title: "t".into(),
                body: "b".into(),
                source_topic: "x".into(),
            },
            GatewayEventFrame::SurfaceApproval {
                audience: vec!["desktop".into()],
                approval_id: "a1".into(),
                title: "t".into(),
                body: "b".into(),
            },
        ];

        for frame in &samples {
            let topic = frame
                .stream_method()
                .map_or_else(|| frame.topic_name(), str::to_string);
            let data = serde_json::to_value(frame).unwrap();
            let actual = session_identity_of(&topic, Some(&data));
            assert_eq!(actual, expected(frame), "topic={topic}");
        }
    }

    #[tokio::test]
    async fn index_is_bounded_and_evicts_on_run_completion() {
        let index = EventVisibilityIndex::new();

        let accepted = serde_json::json!({
            "run_id": "r1",
            "session_key": "agent:main:main",
            "accepted_at": "t",
        });
        index
            .note_frame("stream.run_accepted", Some(&accepted))
            .await;
        assert_eq!(
            index.session_key_for_run("r1").await,
            Some("agent:main:main".to_string())
        );

        let complete = serde_json::json!({
            "run_id": "r1",
            "seq": 1,
            "summary": {},
            "total_duration_ms": 1,
        });
        index
            .note_frame("stream.run_complete", Some(&complete))
            .await;
        assert_eq!(
            index.session_key_for_run("r1").await,
            None,
            "RunComplete must evict the run→session seed"
        );

        for i in 0..(MAX_TRACKED_RUNS + 10) {
            let f = serde_json::json!({
                "run_id": format!("run-{i}"),
                "session_key": "agent:main:overflow",
                "accepted_at": "t",
            });
            index.note_frame("stream.run_accepted", Some(&f)).await;
        }
        assert!(
            index.tracked_run_count().await <= MAX_TRACKED_RUNS,
            "the run index must stay capacity-bounded"
        );
        assert_eq!(
            index.session_key_for_run("run-0").await,
            None,
            "the oldest entry must be evicted under capacity pressure"
        );
    }
}
