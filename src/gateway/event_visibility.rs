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
//! Session-visibility lookups (`session_key` → may this caller see it) go
//! through a second bounded cache. What that cache holds is the session row's
//! `(owner_user_id, scope_id)` pair — the two FACTS — and never a per-caller
//! verdict. The verdict itself is recomputed per caller per frame by
//! `visibility::owner_and_scope_visible_to`, the same body
//! `visibility::session_visible_to` runs for RPCs (spec §5.4's
//! single-authority requirement extends here). Caching the pair rather than
//! the answer is what makes P2's "removing a member takes effect at the next
//! predicate evaluation" promise hold on this path too — a cached boolean
//! would have nothing to invalidate — and it costs nothing: the roster read
//! behind a room's answer is a synchronous in-memory `RwLock`
//! ([`crate::projects::roster`]).
//!
//! ## Why the `team.<id>.*` plane needs its own resolution
//!
//! `publish_team_event` builds a raw `{topic, data}` string envelope — no
//! `GatewayEventFrame` variant exists behind it — so the whole team plane sits
//! outside `every_frame_variant_is_classified`'s exhaustive match, the very
//! mechanism installed to stop a session-scoped frame from defaulting to
//! `Global`. It defaulted to `Global` for exactly as long as nobody looked:
//! every connected user received every other user's team chat BODIES
//! (`team.<id>.message` carries the member agent's deliverable text verbatim).
//! `run.subagent_tree` was the same blind spot, found once and fixed for that
//! one producer without asking which OTHER producers bypass `publish_frame`.
//! Hence [`SessionIdentity::ByTeamId`], resolved through the `TeamStore`, and
//! `no_published_team_topic_suffix_classifies_as_global` — a SOURCE-level pin,
//! because a compile-time one structurally cannot see a raw-string producer.
//!
//! The team id is extracted **structurally**
//! ([`aleph_protocol::team_topic::team_topic_id`], the one parser the Panel
//! also consumes): any `team.<id>.<anything>` addresses `<id>`. Recognizing a
//! whitelist of suffixes here would put the next new suffix back on the
//! broadcast path — an enumeration only covers the world as of the day it was
//! written.
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
//! `Vec<String>` spanning every user's in-flight sessions with no single owner
//! to check against, so pass/fail is the wrong question for it entirely; it
//! stays `Global` and its ARRAY is narrowed per connection instead — see the
//! next section. See [`session_identity_of`]'s doc and the
//! `every_frame_variant_is_classified` pin test for the full, reviewed list.
//!
//! ## Payload projection: the one frame narrowed rather than admitted
//!
//! `stream.running_set_changed` cannot be answered with a boolean. Its payload
//! is `{seq, running: Vec<String>}` — every in-flight session key in the
//! process, spanning every user — and BOTH available booleans are wrong.
//! Forwarding it whole hands every member every other member's live session
//! keys (agent persona, channel peer id, activity timing). Gating the topic
//! operator-only (the obvious fix, correctly rejected once already) silently
//! extinguishes each member's OWN sidebar red dot, which this frame is the
//! authoritative server-side feed for.
//!
//! So this module has a second entry point beside [`EventVisibilityIndex::
//! event_admits`]: [`EventVisibilityIndex::project_for`] rewrites the payload
//! per connection, keeping only the elements that connection's caller could
//! already see through `session_admits`. Its two invariants — always send the
//! frame, even when the array comes back empty; drop any element whose owner
//! cannot be resolved — are stated at that method, because they are the two
//! ways a well-meaning change breaks it.
//!
//! Dropping an unresolvable element is coupled to WHEN the frame is produced,
//! and the coupling had to be paid for: `SessionRunRegistry::try_claim`
//! broadcasts from the first statement of `ExecutionEngine::admit_run`, before
//! the session row exists, so a brand-new conversation's key was dropped from
//! the only frame that would have lit its dot. The seq is then spent, the next
//! frame is the release at run end (which excludes the key too), and nothing
//! re-fetches — the dot stayed dark for the whole first turn. The fix is a
//! PRODUCER, not a looser rule: `execute.rs` calls
//! `SessionRunRegistry::republish_running_set` right after `ensure_session`,
//! re-publishing the same set at a fresh seq once the key resolves. Any new
//! projected frame owes the same question: is this id resolvable at the
//! instant the frame is produced?
//!
//! Deliberately NOT a `PayloadProjector` trait, a projector registry, or a
//! three-variant `Delivery` enum (R10/P6): one topic, one arm, one consumer.
//! `Option<Value>` is the whole shape, and `None` — one string compare — is
//! every other frame in the system.
//!
//! The same array has a SECOND producer, and fixing either alone leaves the
//! other broken: `gateway.metrics.run_concurrency` returns the identical set
//! over RPC (the Panel's cold-load seed for the red dot, and its usage gauge).
//! It is filtered by the same rule in `handlers::gateway_metrics`, which is
//! what makes that method's `ListFiltered` registration true.
//!
//! ## Fail-closed
//!
//! A `caller_user: None` connection (walled — the login wall already refused
//! it) is denied for any resolvable identity, as defense in depth. An
//! unresolvable `run_id` (cache miss — the event raced ahead of
//! `RunAccepted`, or predates this filter) is denied: a dropped early frame
//! self-heals via `run_complete`'s summary reconciliation on the client side,
//! but a leaked frame cannot be un-leaked. A `team_id` that no longer resolves
//! to a team — or that arrives with no `TeamStore` wired at all — is denied on
//! the same reasoning.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;
use tokio::sync::RwLock;

use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::SessionMetadata;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility::{owner_and_scope_visible_to, owner_or_legacy};
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::utils::fifo_cache::remember;
use aleph_protocol::team_topic::team_topic_id;

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
    /// A `team.<id>.*` frame: attributable to a TEAM rather than a session (a
    /// team's events span its members' many runs and sessions). Resolved to the
    /// team's owner through the `TeamStore` — see the module doc.
    ByTeamId(String),
    /// Unattributable to any one session — org-level infrastructure, or
    /// already covered by a different gate (see module doc).
    Global,
}

/// The one topic this module PROJECTS rather than admits/denies, and the one
/// payload field it rewrites. Named once because two bodies match on them —
/// [`session_identity_of`]'s `Global` arm and
/// [`EventVisibilityIndex::project_for`]'s single arm — and a literal that
/// disagrees between those two is silent: the classification keeps saying
/// `Global` while the projection stops firing, i.e. the leak comes back with
/// every test still green.
const RUNNING_SET_TOPIC: &str = "stream.running_set_changed";
const RUNNING_SET_FIELD: &str = "running";

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
/// denial. The `team.` prefix is the one family that opts OUT of that default:
/// it is checked structurally before the match, so an unreviewed team suffix
/// is owner-scoped rather than broadcast (see the module doc).
#[must_use]
pub fn session_identity_of(topic: &str, data: Option<&Value>) -> SessionIdentity {
    // `team.<id>.*` first, and structurally: these topics are raw strings from
    // `publish_team_event` / `CoordTaskStore`, so no exhaustive match downstream
    // can catch a suffix added later. Everything under a non-empty team id is
    // that team's, whether or not this file has heard of the suffix. The global
    // `team.changed` has no id and falls through to the match below.
    if let Some(team_id) = team_topic_id(topic) {
        return SessionIdentity::ByTeamId(team_id.to_string());
    }
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

        // Broadcast red-dot spanning every owner's running sessions. `Global`
        // is the right CLASSIFICATION — no one owns this frame — and is not the
        // whole answer: its `running` array is narrowed per connection by
        // [`EventVisibilityIndex::project_for`]. Gating the topic instead would
        // extinguish every member's OWN red dot; see the module doc.
        RUNNING_SET_TOPIC => SessionIdentity::Global,

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
        // The 2-segment team-LIST invalidation (`GatewayEventFrame::TeamChanged`,
        // carrying only a team id + change kind): it names no team the grammar
        // above can extract, and every user's sidebar needs the nudge to refetch
        // its OWN — already owner-filtered — `teams.list`.
        | "team.changed"
        | "surface.notify" => SessionIdentity::Global,

        // Unrecognized topic: fail open at classification (see doc above).
        _ => SessionIdentity::Global,
    }
}

/// Mirrors `streaming/relay.rs`'s `StreamRegistry` hygiene: a hard capacity
/// cap plus insertion-order (FIFO) eviction, so a long-uptime process with
/// many runs/sessions/teams never grows any of these caches unbounded. The
/// eviction rule itself is `utils::fifo_cache::remember`; only the caps live
/// with their owners.
const MAX_TRACKED_RUNS: usize = 4096;
const MAX_CACHED_SESSION_OWNERS: usize = 4096;
const MAX_CACHED_TEAM_OWNERS: usize = 4096;

#[derive(Default)]
struct RunIndex {
    order: VecDeque<String>,
    map: HashMap<String, String>,
}

/// The two immutable facts a session row contributes to a visibility decision
/// — everything `visibility::owner_and_scope_visible_to` reads, and nothing
/// else. Both are stamped once at creation and never rewritten
/// (`SessionMetadata::stamp_attribution`), which is what makes them safe to
/// cache for the process lifetime; the ROSTER they are evaluated against is
/// not cached here and is re-read on every frame.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionOwnership {
    owner_user_id: Option<String>,
    scope_id: Option<String>,
}

impl SessionOwnership {
    fn of(meta: &SessionMetadata) -> Self {
        Self {
            owner_user_id: meta.owner_user_id.clone(),
            scope_id: meta.scope_id.clone(),
        }
    }

    fn visible_to(&self, caller: &str) -> bool {
        owner_and_scope_visible_to(
            self.owner_user_id.as_deref(),
            self.scope_id.as_deref(),
            caller,
        )
    }
}

/// `None` as a cached value means "this key can never resolve to a session"
/// (a malformed key string), which denies every caller — deliberately NOT the
/// same thing as an absent `owner_user_id`, which reads as the legacy owner.
#[derive(Default)]
struct OwnershipCache {
    order: VecDeque<String>,
    map: HashMap<String, Option<SessionOwnership>>,
}

/// Cached team→owner stamps for [`SessionIdentity::ByTeamId`].
///
/// The value is the team row's `owner_user_id` VERBATIM, so `None` here means
/// a legacy/unstamped team — which reads as the operator's through
/// `visibility::owner_or_legacy` and ADMITS them. An id that could not be
/// resolved at all (absent team, store error) is deliberately not stored:
/// there is no invalidation hook on this cache, so a cached "unresolvable"
/// would outlive its cause. It is denied per frame and re-resolved on the next
/// one, exactly like an absent session row.
///
/// A team's owner is stamped once in `SqliteTeamStore::create_team` and no
/// `TeamStore` method rewrites it, which is what makes the fact cacheable for
/// the process lifetime.
#[derive(Default)]
struct TeamOwnerCache {
    order: VecDeque<String>,
    map: HashMap<String, Option<String>>,
}

/// Process-shared (via `GatewaySharedState`/`ConnectionContext`, one
/// instance for the whole gateway) run→session seed plus session→owner and
/// team→owner caches backing [`session_identity_of`]'s
/// `ByRunId`/`BySessionKey`/`ByTeamId` resolution. See the module doc for the
/// full design rationale.
#[derive(Default)]
pub struct EventVisibilityIndex {
    runs: RwLock<RunIndex>,
    owners: RwLock<OwnershipCache>,
    team_owners: RwLock<TeamOwnerCache>,
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
    ///
    /// `teams` is the gateway's `TeamStore` handle, needed only by the
    /// `team.<id>.*` plane; `None` (no team database in this deployment) DENIES
    /// those frames rather than waving them through — a team topic carries a
    /// member agent's chat body, and with no resolver there is no honest answer
    /// to "whose". Nothing can produce one either: the broadcaster and the
    /// coordination-task store that publish them are both built from the very
    /// store whose absence this arm describes.
    pub async fn event_admits(
        &self,
        topic: &str,
        data: Option<&Value>,
        caller_user: Option<&str>,
        store: &Arc<dyn SessionStore>,
        teams: Option<&Arc<dyn TeamStore>>,
    ) -> bool {
        match session_identity_of(topic, data) {
            SessionIdentity::Global => true,
            SessionIdentity::BySessionKey(session_key) => {
                let Some(caller) = caller_user else {
                    return false;
                };
                self.session_admits(&session_key, caller, store).await
            }
            SessionIdentity::ByRunId(run_id) => {
                let Some(caller) = caller_user else {
                    return false;
                };
                let Some(session_key) = self.session_key_for_run(&run_id).await else {
                    return false; // unresolvable — fail closed (see module doc)
                };
                self.session_admits(&session_key, caller, store).await
            }
            SessionIdentity::ByTeamId(team_id) => {
                let Some(caller) = caller_user else {
                    return false;
                };
                self.team_admits(&team_id, caller, teams).await
            }
        }
    }

    /// The per-connection payload PROJECTION — [`Self::event_admits`]'s
    /// sibling for the one frame whose honest answer is not "yes or no" but
    /// "yes, this much of it".
    ///
    /// Returns `Some(payload)` — a replacement for the frame's own payload
    /// object — when this caller must receive a narrowed copy, and `None` for
    /// every other topic, which is one string compare and lets the delivery
    /// loop forward the bytes it already holds untouched. Only
    /// [`RUNNING_SET_TOPIC`] has an arm; see the module doc for why that frame
    /// is `Global` and still not deliverable verbatim.
    ///
    /// Two properties this must keep, both of which look safe to break:
    ///
    /// 1. **The frame is still sent when the array comes back empty.** The
    ///    Panel's `SessionMap::set_server_running` discards any frame whose
    ///    `seq` is `<= server_seq`, so SUPPRESSING one does not merely go
    ///    unrendered — it consumes that seq, and no later frame can then clear
    ///    a dot that is already lit. An empty `running` is the meaningful
    ///    answer "nothing of yours is running"; silence latches the stale dot
    ///    for the rest of the connection. Hence a payload rewrite here and no
    ///    new `false` in `event_admits`.
    /// 2. **An element whose owner cannot be resolved is DROPPED, never passed
    ///    through.** [`Self::session_admits`] already fails closed on a
    ///    malformed key, an absent row and a store error; applying it per
    ///    element inherits that rule rather than re-deriving it. A walled
    ///    connection (`caller_user: None`) resolves nothing and therefore
    ///    receives an empty array — same fail-closed direction as
    ///    `event_admits`, expressed in the shape this frame needs.
    pub async fn project_for(
        &self,
        topic: &str,
        data: Option<&Value>,
        caller_user: Option<&str>,
        store: &Arc<dyn SessionStore>,
    ) -> Option<Value> {
        match topic {
            RUNNING_SET_TOPIC => {}
            _ => return None,
        }
        // No array to narrow ⇒ nothing this projection could leak, and
        // rewriting a shape we do not recognize is worse than forwarding it.
        // `the_published_frame_is_projected_through_its_real_wire_shape` pins
        // both the topic and the field name against the real producer, so a
        // rename cannot land here as a silent no-op.
        let payload = data?.as_object()?;
        let running = payload.get(RUNNING_SET_FIELD).and_then(Value::as_array)?;

        let mut visible: Vec<Value> = Vec::with_capacity(running.len());
        if let Some(caller) = caller_user {
            for entry in running {
                let Some(key) = entry.as_str() else {
                    continue; // not a session key ⇒ not resolvable ⇒ dropped
                };
                if self.session_admits(key, caller, store).await {
                    visible.push(Value::String(key.to_string()));
                }
            }
        }

        let mut projected = payload.clone();
        projected.insert(RUNNING_SET_FIELD.to_string(), Value::Array(visible));
        Some(Value::Object(projected))
    }

    async fn insert_run(&self, run_id: String, session_key: String) {
        let mut inner = self.runs.write().await;
        let RunIndex { order, map } = &mut *inner;
        remember(order, map, run_id, session_key, MAX_TRACKED_RUNS);
    }

    async fn evict_run(&self, run_id: &str) {
        let mut inner = self.runs.write().await;
        inner.map.remove(run_id);
        inner.order.retain(|r| r != run_id);
    }

    async fn session_key_for_run(&self, run_id: &str) -> Option<String> {
        self.runs.read().await.map.get(run_id).cloned()
    }

    /// Whether `caller` may see a resolved session key — the SAME body
    /// `visibility.rs`'s RPC-side predicates run
    /// ([`owner_and_scope_visible_to`], reached from `session_visible_to`
    /// there), so a project room's frames follow the ROSTER and a personal
    /// session's follow its owner.
    ///
    /// Only the session's two immutable facts are cached (fill-on-miss from
    /// `store`); the caller is applied to them on every call. An owner-equality
    /// check here — or a per-caller boolean in the cache — denies every member
    /// of a room but its creator, and denies them silently.
    async fn session_admits(
        &self,
        session_key: &str,
        caller: &str,
        store: &Arc<dyn SessionStore>,
    ) -> bool {
        if let Some(cached) = {
            let inner = self.owners.read().await;
            inner.map.get(session_key).cloned()
        } {
            return cached.is_some_and(|o| o.visible_to(caller));
        }

        let ownership = match SessionKey::from_key_string(session_key) {
            Some(key) => match store.get_metadata(&key).await {
                Ok(Some(meta)) => Some(SessionOwnership::of(&meta)),
                // Row absent: TRANSIENT, exactly like the store error below,
                // and for a reason that fires on the happy path of a brand-new
                // conversation. `execute.rs` emits `RunAccepted{session_key}`
                // BEFORE `ensure_session` creates the row, so the very first
                // frame of a fresh session can arrive while the row does not
                // exist yet. Caching that absence as an unresolvable key would
                // deny EVERY later frame for that session key — the cache has no
                // invalidation and evicts only by FIFO at
                // `MAX_CACHED_SESSION_OWNERS` — so streaming for that
                // conversation would stay dead for the process lifetime. It
                // fails closed, so nothing leaks, but it dies silently, and
                // loopback resolves to `Some(OWNER_USER_ID)` so a single-user
                // box runs this path too.
                //
                // Deny THIS frame and re-resolve on the next one. For run
                // frames the dropped early frame self-heals via
                // `run_complete`'s summary reconciliation on the client (see
                // the module doc). For the `running_set_changed` PROJECTION
                // there is no such client-side reconciliation, so "the next
                // one" is a real producer, not a hope:
                // `SessionRunRegistry::republish_running_set`, called by
                // `execute.rs` immediately after `ensure_session` — see the
                // projection section of the module doc.
                Ok(None) => return false,
                // Store error: fail closed, and don't cache a transient
                // failure as a permanent "no owner" — matching
                // `visibility::existing_session_is_visible`'s own rule.
                Err(_) => return false,
            },
            // Malformed session_key string: cache as unresolvable so a
            // repeated malformed key doesn't re-hit the parse on every event.
            // This one IS permanent — a string that does not parse today will
            // not parse later, so nothing can invalidate it. It must stay
            // distinct from a row whose `owner_user_id` is absent, which reads
            // as the LEGACY owner and admits them.
            None => None,
        };
        self.cache_ownership(session_key.to_string(), ownership.clone())
            .await;
        ownership.is_some_and(|o| o.visible_to(caller))
    }

    async fn cache_ownership(&self, session_key: String, ownership: Option<SessionOwnership>) {
        let mut inner = self.owners.write().await;
        let OwnershipCache { order, map } = &mut *inner;
        remember(
            order,
            map,
            session_key,
            ownership,
            MAX_CACHED_SESSION_OWNERS,
        );
    }

    /// Whether `caller` may receive a `team.<id>.*` frame.
    ///
    /// A team is owned outright — there is no `scope_id` column on `teams` and
    /// no roster of USERS (a team's members are agents), so the room-vs-personal
    /// branch `session_admits` needs has nothing to decide here. The one rule
    /// that must not be re-derived is adoption-by-absence, and that goes through
    /// [`owner_or_legacy`] — the same single authority `session_visible_to`
    /// reaches on the RPC path.
    ///
    /// Only the team's owner STAMP is cached; the caller is applied to it on
    /// every call, so this can never hold a stale per-caller verdict.
    ///
    /// The store handle is the ownership-scoped `ScopedTeamStore` (the only one
    /// boot publishes). Its filter is ambient — `scope::ambient_owner()` — and
    /// this runs on the socket's own delivery task, which is outside every
    /// dispatch scope, so the decorator reads as unrestricted here and the
    /// per-caller decision is the explicit one below. If a future change ever
    /// wraps this loop in a scope, the decorator would start hiding teams from
    /// the delivery path and this resolution degrades to a denial — the safe
    /// direction, but a silent one.
    async fn team_admits(
        &self,
        team_id: &str,
        caller: &str,
        teams: Option<&Arc<dyn TeamStore>>,
    ) -> bool {
        if let Some(cached) = {
            let inner = self.team_owners.read().await;
            inner.map.get(team_id).cloned()
        } {
            return owner_or_legacy(cached.as_deref()) == caller;
        }

        // No team database wired ⇒ nothing can answer "whose team is this".
        let Some(store) = teams else {
            return false;
        };
        let owner = match store.get_team(team_id).await {
            Ok(Some(team)) => team.owner_user_id,
            // Absent (deleted mid-fan-out, or an id from a producer this
            // deployment does not have) or a store error: deny this frame and
            // re-resolve on the next, never cache the denial (see
            // `TeamOwnerCache`).
            Ok(None) | Err(_) => return false,
        };
        self.cache_team_owner(team_id.to_string(), owner.clone())
            .await;
        owner_or_legacy(owner.as_deref()) == caller
    }

    async fn cache_team_owner(&self, team_id: String, owner: Option<String>) {
        let mut inner = self.team_owners.write().await;
        let TeamOwnerCache { order, map } = &mut *inner;
        remember(order, map, team_id, owner, MAX_CACHED_TEAM_OWNERS);
    }

    #[cfg(test)]
    async fn tracked_run_count(&self) -> usize {
        self.runs.read().await.map.len()
    }

    #[cfg(test)]
    async fn cached_team_count(&self) -> usize {
        self.team_owners.read().await.map.len()
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
                .event_admits(
                    "stream.agent_trace",
                    Some(&trace),
                    Some("alice"),
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    "stream.agent_trace",
                    Some(&trace),
                    Some("bob"),
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    "stream.agent_trace",
                    Some(&trace),
                    Some(OWNER_USER_ID),
                    &store,
                    None
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
                .event_admits(
                    "stream.agent_trace",
                    Some(&trace),
                    Some("alice"),
                    &store,
                    None
                )
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
                .event_admits(
                    "stream.agent_trace",
                    Some(&trace),
                    Some("alice"),
                    &store,
                    None
                )
                .await,
            "an absent row must be re-resolved on the next frame, not cached as a denial"
        );
        // ...and the re-resolution is a real one, not a blanket allow.
        assert!(
            !index
                .event_admits(
                    "stream.agent_trace",
                    Some(&trace),
                    Some("bob"),
                    &store,
                    None
                )
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
                .event_admits(
                    "stream.agent_trace",
                    Some(&trace),
                    Some("alice"),
                    &store,
                    None
                )
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
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    "session.lifecycle.changed",
                    Some(&data),
                    Some("bob"),
                    &store,
                    None
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
                .event_admits(
                    "run.subagent_tree",
                    Some(&progress),
                    Some("alice"),
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    "run.subagent_tree",
                    Some(&progress),
                    Some("bob"),
                    &store,
                    None
                )
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
                .event_admits(
                    "run.subagent_tree",
                    Some(&settled),
                    Some("alice"),
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    "run.subagent_tree",
                    Some(&settled),
                    Some("bob"),
                    &store,
                    None
                )
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
                .event_admits(
                    "run.subagent_tree",
                    Some(&spawned),
                    Some("alice"),
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    "run.subagent_tree",
                    Some(&spawned),
                    Some("bob"),
                    &store,
                    None
                )
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
                    .event_admits("tools.changed", None, caller, &store, None)
                    .await,
                "an unattributable topic must pass for {caller:?}"
            );
        }
    }

    // ── the `team.<id>.*` plane ─────────────────────────────────────────

    async fn team_store() -> Arc<dyn TeamStore> {
        let s = crate::teams::SqliteTeamStore::new(rusqlite::Connection::open_in_memory().unwrap());
        s.migrate().await.unwrap();
        crate::teams::ScopedTeamStore::wrap(Arc::new(s))
    }

    /// Create a team the way boot's single construction site does: stamped
    /// from the ambient scope. `owner: None` reproduces a legacy/unscoped
    /// creation (cron, pre-P1 row), which carries no stamp at all.
    async fn create_team(store: &Arc<dyn TeamStore>, owner: Option<&str>, name: &str) -> String {
        let input = crate::teams::NewTeam {
            name: name.to_string(),
            description: String::new(),
            leader_id: "agent-main".to_string(),
        };
        crate::scope::with_scope(
            owner.map(crate::scope::ScopeAttribution::personal),
            store.create_team(input),
        )
        .await
        .unwrap()
        .id
    }

    /// The leak this arm closes: `team.<id>.message` carries a member agent's
    /// deliverable text verbatim, and every one of these topics used to fall
    /// through `session_identity_of`'s catch-all to `Global` — delivered to
    /// every connected user.
    ///
    /// The unknown suffix in the loop is the load-bearing case: classification
    /// must be structural, so the day someone publishes a sixth suffix it is
    /// scoped without anyone remembering to add it here.
    #[tokio::test]
    async fn team_topics_are_scoped_to_the_teams_owner() {
        let (sessions, _temp) = test_store();
        let sessions: Arc<dyn SessionStore> = Arc::new(sessions);
        let teams = team_store().await;
        let team_id = create_team(&teams, Some("u-alice"), "Alice Squad").await;

        let index = EventVisibilityIndex::new();
        let body = serde_json::json!({
            "agent_id": "agent-main",
            "text": "alice's team said something private",
            "final": true,
        });

        for suffix in [
            "message",
            "activity",
            "system",
            "fanout",
            "task.created",
            "a_new_verb",
        ] {
            let topic = format!("team.{team_id}.{suffix}");
            assert!(
                index
                    .event_admits(
                        &topic,
                        Some(&body),
                        Some("u-alice"),
                        &sessions,
                        Some(&teams)
                    )
                    .await,
                "{topic}: the team's owner must still receive her own team's frames"
            );
            assert!(
                !index
                    .event_admits(&topic, Some(&body), Some("u-bob"), &sessions, Some(&teams))
                    .await,
                "{topic}: a second logged-in user must not receive another user's team chat"
            );
            assert!(
                !index
                    .event_admits(
                        &topic,
                        Some(&body),
                        Some(OWNER_USER_ID),
                        &sessions,
                        Some(&teams)
                    )
                    .await,
                "{topic}: the operator is not exempt from team ownership either"
            );
            assert!(
                !index
                    .event_admits(&topic, Some(&body), None, &sessions, Some(&teams))
                    .await,
                "{topic}: a walled connection carries no identity to admit"
            );
        }
    }

    /// Adoption by absence, the zero-change half: a team created outside any
    /// dispatch scope (single-user box before P1, cron, internal) carries no
    /// stamp and reads as the operator's — resolved through
    /// `visibility::owner_or_legacy`, never a second `unwrap_or` here.
    #[tokio::test]
    async fn a_legacy_unstamped_team_reads_as_the_operators() {
        let (sessions, _temp) = test_store();
        let sessions: Arc<dyn SessionStore> = Arc::new(sessions);
        let teams = team_store().await;
        let team_id = create_team(&teams, None, "Legacy").await;
        let topic = format!("team.{team_id}.message");

        let index = EventVisibilityIndex::new();
        assert!(
            index
                .event_admits(&topic, None, Some(OWNER_USER_ID), &sessions, Some(&teams))
                .await,
            "an unstamped team belongs to the legacy operator — loopback must still \
             see its own team chat"
        );
        assert!(
            !index
                .event_admits(&topic, None, Some("u-bob"), &sessions, Some(&teams))
                .await
        );
    }

    /// Fail-closed, and NOT latched: an id that resolves to nothing is denied
    /// per frame and never written to the cache, which has no invalidation
    /// hook anyone could call.
    #[tokio::test]
    async fn an_unresolvable_team_denies_everyone_and_is_not_cached() {
        let (sessions, _temp) = test_store();
        let sessions: Arc<dyn SessionStore> = Arc::new(sessions);
        let teams = team_store().await;
        let real = create_team(&teams, Some("u-alice"), "Alice Squad").await;

        let index = EventVisibilityIndex::new();
        for caller in [Some("u-alice"), Some(OWNER_USER_ID)] {
            assert!(
                !index
                    .event_admits(
                        "team.team-never-existed.message",
                        None,
                        caller,
                        &sessions,
                        Some(&teams)
                    )
                    .await,
                "an unresolvable team must fail closed for {caller:?}"
            );
        }
        assert_eq!(
            index.cached_team_count().await,
            0,
            "an unresolvable id must not be cached — the cache has nothing to \
             invalidate it with"
        );

        // ...while a team that DOES resolve is cached exactly once.
        let topic = format!("team.{real}.message");
        assert!(
            index
                .event_admits(&topic, None, Some("u-alice"), &sessions, Some(&teams))
                .await
        );
        assert!(
            index
                .event_admits(&topic, None, Some("u-alice"), &sessions, Some(&teams))
                .await
        );
        assert_eq!(index.cached_team_count().await, 1);
    }

    /// No team database wired ⇒ no resolver ⇒ no delivery. Denies even the
    /// operator: "I cannot tell whose this is" is not a reason to broadcast.
    #[tokio::test]
    async fn a_team_topic_with_no_team_store_denies_everyone() {
        let (sessions, _temp) = test_store();
        let sessions: Arc<dyn SessionStore> = Arc::new(sessions);
        let index = EventVisibilityIndex::new();

        for caller in [Some("u-alice"), Some(OWNER_USER_ID), None] {
            assert!(
                !index
                    .event_admits("team.t1.message", None, caller, &sessions, None)
                    .await,
                "with no TeamStore there is no honest answer for {caller:?}"
            );
        }
    }

    /// The `if let` gate a call site sits directly under, scraped from source:
    /// the last `if let` before `marker`, up to the `{` opening its block.
    fn enclosing_if_let_gate(src: &str, marker: &str) -> String {
        let at = src
            .find(marker)
            .unwrap_or_else(|| panic!("no `{marker}` in source — this pin has gone vacuous"));
        let gate_start = src[..at].rfind("if let ").unwrap_or_else(|| {
            panic!("`{marker}` sits under no `if let` at all — re-read this pin's doc")
        });
        let tail = &src[gate_start..];
        let brace = tail
            .find('{')
            .unwrap_or_else(|| panic!("unterminated `if let` before `{marker}`"));
        tail[..brace].to_string()
    }

    /// Every `*_store` binding a gate expression names, sorted and deduped.
    fn stores_named(gate: &str) -> Vec<String> {
        let mut v: Vec<String> = gate
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|t| t.ends_with("_store"))
            .map(str::to_string)
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// SOURCE-level pin: the `TeamStore` handle this module needs in order to
    /// resolve `team.<id>.*` must be installed under a condition no NARROWER
    /// than the one registering the PRODUCER of those frames.
    ///
    /// `team_admits` denies outright when `teams` is `None` (pinned directly by
    /// `a_team_topic_with_no_team_store_denies_everyone`), and that denial is
    /// silent by design — refusal is the common case, so nothing logs it. So if
    /// boot installs the resolver under a condition the producer does not
    /// share, team group chat keeps publishing while EVERY connection, operator
    /// included, drops the frames: no error on any surface, no log line.
    ///
    /// The narrower condition this pin was written against was `team_store &&
    /// coord_task_store` — two different databases (`teams.db` / `coord.db`)
    /// opened by two different files, where `coord.db` failing is an explicitly
    /// supported warn-and-continue degraded boot. See 地雷 H in
    /// `src/gateway/CLAUDE.md`.
    ///
    /// Source text is what is left to assert on: both sites are boot wiring in
    /// the `aleph-server` binary target, reachable only by running `start`.
    #[test]
    fn the_team_resolver_gate_is_no_narrower_than_its_frame_producers_gate() {
        const RESOLVER_SRC: &str = include_str!("../bin/aleph-server/commands/start/mod.rs");
        const PRODUCER_SRC: &str =
            include_str!("../bin/aleph-server/commands/start/builder/agent_init/mod.rs");
        const RESOLVER_CALL: &str = "server.set_team_store(";
        const PRODUCER_CALL: &str = ".register(\"teams.chat.send\"";

        assert_eq!(
            RESOLVER_SRC.matches(RESOLVER_CALL).count(),
            1,
            "expected exactly one `{RESOLVER_CALL}` in boot: zero means this pin has \
             gone vacuous, more than one means a second install site needs its own \
             gate checked here"
        );
        assert_eq!(
            PRODUCER_SRC.matches(PRODUCER_CALL).count(),
            1,
            "expected exactly one `{PRODUCER_CALL}` registration; the scraper has \
             stopped matching the call shape"
        );

        let resolver_gate = enclosing_if_let_gate(RESOLVER_SRC, RESOLVER_CALL);
        let producer_gate = enclosing_if_let_gate(PRODUCER_SRC, PRODUCER_CALL);
        let producer_needs = stores_named(&producer_gate);

        assert!(
            producer_needs.iter().any(|s| s == "team_store"),
            "producer gate `{producer_gate}` names no team store — the scraper is \
             reading the wrong `if let`"
        );
        for store in stores_named(&resolver_gate) {
            assert!(
                producer_needs.contains(&store),
                "boot installs the `team.<id>.*` resolver under `{resolver_gate}`, \
                 which requires `{store}`, but the frames' PRODUCER requires only \
                 {producer_needs:?}. Whenever `{store}` is absent every connection — \
                 operator included — silently denies team chat."
            );
        }
    }

    /// Every string literal a producer passes as the topic SUFFIX, scraped
    /// from its source. Arg 1 of these calls is always an identifier
    /// (`team_id` / `&team_id` / `&self.team_id`), so the first string literal
    /// inside the call is the suffix.
    fn published_suffixes(src: &str, marker: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (idx, _) in src.match_indices(marker) {
            // Bounded by chars, not bytes: these files contain non-ASCII.
            let window: String = src[idx..].chars().take(300).collect();
            let Some(open) = window.find('"') else {
                continue;
            };
            let rest = &window[open + 1..];
            let Some(close) = rest.find('"') else {
                continue;
            };
            out.push(rest[..close].to_string());
        }
        out
    }

    /// SOURCE-level pin: no suffix any producer actually publishes may
    /// classify as `Global`.
    ///
    /// `every_frame_variant_is_classified` cannot do this job.
    /// `publish_team_event` emits a raw `{topic, data}` string with no
    /// `GatewayEventFrame` variant behind it, so adding a team topic breaks no
    /// match anywhere in this crate — which is exactly how the whole plane
    /// stayed `Global`. A compile-anchored pin is structurally blind to a
    /// raw-string producer; the only thing that is not blind to it is its
    /// source text.
    ///
    /// `CoordTaskStore::emit_task_topic` (`team.<id>.task.<verb>`) composes its
    /// whole topic with `format!` and has no suffix argument to scrape, so it
    /// is asserted directly below — and covered anyway by the classifier being
    /// structural rather than a suffix list.
    #[test]
    fn no_published_team_topic_suffix_classifies_as_global() {
        const PRODUCERS: [(&str, &str, &str); 3] = [
            (
                "src/teams/broadcast/mod.rs",
                include_str!("../teams/broadcast/mod.rs"),
                "publish_team_event(",
            ),
            (
                "src/teams/dispatcher/schedule/settle.rs",
                include_str!("../teams/dispatcher/schedule/settle.rs"),
                "publish_team_event(",
            ),
            (
                "src/gateway/event_emitter/team_fanout.rs",
                include_str!("event_emitter/team_fanout.rs"),
                "self.publish(",
            ),
        ];

        let mut scanned = 0usize;
        for (path, src, marker) in PRODUCERS {
            let suffixes = published_suffixes(src, marker);
            assert!(
                !suffixes.is_empty(),
                "{path}: found no `{marker}` call — the scanner stopped matching the \
                 call shape, so this pin has quietly become vacuous"
            );
            for suffix in suffixes {
                scanned += 1;
                let topic = format!("team.t-pin.{suffix}");
                assert_eq!(
                    session_identity_of(&topic, None),
                    SessionIdentity::ByTeamId("t-pin".to_string()),
                    "{path} publishes `{suffix}`, which classifies as anything but \
                     its own team — every connection would receive it"
                );
            }
        }
        assert!(
            scanned >= 8,
            "only {scanned} suffixes scraped; the producers have ~10 calls between \
             them, so the scanner is missing some"
        );

        // The `format!`-composed producer, asserted by hand.
        assert_eq!(
            session_identity_of("team.t-pin.task.created", None),
            SessionIdentity::ByTeamId("t-pin".to_string()),
            "CoordTaskStore::emit_task_topic's family must be team-scoped too"
        );
        // ...and the one `team.` topic that genuinely belongs to everyone.
        assert_eq!(
            session_identity_of("team.changed", None),
            SessionIdentity::Global,
            "the global team-LIST invalidation must stay Global — every user's \
             sidebar needs it to refetch its own (already filtered) teams.list"
        );
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
                origin_run_id: None,
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

    // ── Payload projection (`stream.running_set_changed`) ────────────────

    /// Read the projected array back out of whatever `project_for` returned,
    /// so every assertion below is about the REPLACEMENT PAYLOAD that would go
    /// on the wire — not about the call having happened.
    fn projected_keys(projected: &Value) -> Vec<String> {
        projected[RUNNING_SET_FIELD]
            .as_array()
            .expect("a projected running-set frame still carries a `running` array")
            .iter()
            .map(|v| v.as_str().expect("session keys are strings").to_string())
            .collect()
    }

    /// The projection is pinned to the REAL producer, not to a hand-written
    /// payload: publish an actual `RunningSetChanged` through the real event
    /// bus, take the bytes off the wire, and project those. This is what
    /// catches a rename of either literal (`stream.running_set_changed` /
    /// `running`) — a mismatch there is otherwise silent, because
    /// `session_identity_of` keeps saying `Global` while the projection quietly
    /// stops firing and the whole array goes back on the wire.
    #[tokio::test]
    async fn the_published_frame_is_projected_through_its_real_wire_shape() {
        use crate::gateway::event_bus::GatewayEventBus;

        let (store, _temp) = test_store();
        let alice_key = SessionKey::main("proj-wire-alice");
        let bob_key = SessionKey::main("proj-wire-bob");
        stamp_owner(&store, &alice_key, "u-alice").await;
        stamp_owner(&store, &bob_key, "u-bob").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let bus = GatewayEventBus::new();
        let mut rx = bus.subscribe();
        bus.publish_frame(&GatewayEventFrame::RunningSetChanged {
            seq: 9,
            running: vec![alice_key.to_key_string(), bob_key.to_key_string()],
        })
        .unwrap();
        let wire: Value =
            serde_json::from_str(&rx.try_recv().expect("publish_frame delivers synchronously"))
                .unwrap();

        // Exactly the two strings `server::handler`'s delivery loop derives.
        let topic = wire["method"].as_str().expect("stream-form frame");
        let payload = wire.get("params");
        assert_eq!(
            topic, RUNNING_SET_TOPIC,
            "the producer's wire topic and this module's constant must agree"
        );

        let index = EventVisibilityIndex::new();
        let projected = index
            .project_for(topic, payload, Some("u-alice"), &store)
            .await
            .expect("the real published frame must be projected, not waved through");
        assert_eq!(
            projected_keys(&projected),
            vec![alice_key.to_key_string()],
            "alice must be told about her own session and nobody else's"
        );
        assert_eq!(
            projected["seq"], 9,
            "the projection replaces `running` only — `seq` is the client's \
             ordering guard and must survive verbatim"
        );
    }

    /// Invariant 1, the one that is dangerous to get wrong: a caller with
    /// nothing running must still receive the FRAME, carrying an empty array.
    /// `SessionMap::set_server_running` drops any frame with `seq <=
    /// server_seq`, so suppressing this one burns the seq and latches whatever
    /// dot was last lit for the rest of the connection.
    #[tokio::test]
    async fn a_caller_with_nothing_running_still_gets_a_frame_carrying_an_empty_set() {
        let (store, _temp) = test_store();
        let alice_key = SessionKey::main("proj-empty-alice");
        stamp_owner(&store, &alice_key, "u-alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let payload = serde_json::json!({
            "type": "running_set_changed",
            "seq": 4,
            "running": [alice_key.to_key_string()],
        });
        let index = EventVisibilityIndex::new();
        let projected = index
            .project_for(RUNNING_SET_TOPIC, Some(&payload), Some("u-bob"), &store)
            .await
            .expect("an empty result is a FRAME, never a suppression");
        assert!(
            projected_keys(&projected).is_empty(),
            "bob sees none of alice's sessions"
        );
        assert_eq!(projected["seq"], 4);
    }

    /// Invariant 2: an element that cannot be resolved to an owner is dropped,
    /// never forwarded. Three ways to be unresolvable — a string that is not a
    /// session key at all, a well-formed key with no row behind it, and a
    /// non-string element — and all three must vanish rather than ride along
    /// because "we couldn't tell whose it was".
    #[tokio::test]
    async fn an_unresolvable_element_is_dropped_not_passed_through() {
        let (store, _temp) = test_store();
        let mine = SessionKey::main("proj-unres-mine");
        stamp_owner(&store, &mine, "u-alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let ghost = SessionKey::main("proj-unres-never-created").to_key_string();
        let payload = serde_json::json!({
            "seq": 1,
            "running": [
                mine.to_key_string(),
                ghost,
                "this is not a session key",
                17,
            ],
        });
        let index = EventVisibilityIndex::new();
        let projected = index
            .project_for(RUNNING_SET_TOPIC, Some(&payload), Some("u-alice"), &store)
            .await
            .expect("projected");
        assert_eq!(
            projected_keys(&projected),
            vec![mine.to_key_string()],
            "only the element whose owner actually resolved survives"
        );
    }

    /// P2: the projection asks the same question the rest of the event plane
    /// asks, so a ROOM's running session reaches every member of the roster —
    /// not just whoever created it. Owner-equality would keep bob's dot dark
    /// for a room he is legitimately in, and would do it silently.
    #[tokio::test]
    async fn a_rooms_running_session_reaches_every_member_of_its_roster() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let projects =
            crate::projects::ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
        projects.create_schema().unwrap();
        let room = projects.create("proj room", Some("u-alice"), None).unwrap();
        projects.add_member(&room.id, "u-bob").unwrap();

        let (store, _temp) = test_store();
        let room_key = SessionKey::main("proj-room-session");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution {
                owner_user_id: "u-alice".to_string(),
                scope: crate::scope::ScopeId::Project(room.id.clone()),
            }),
            store.get_or_create(&room_key),
        )
        .await
        .unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let payload = serde_json::json!({
            "seq": 2,
            "running": [room_key.to_key_string()],
        });
        let index = EventVisibilityIndex::new();
        for member in ["u-alice", "u-bob"] {
            let projected = index
                .project_for(RUNNING_SET_TOPIC, Some(&payload), Some(member), &store)
                .await
                .expect("projected");
            assert_eq!(
                projected_keys(&projected),
                vec![room_key.to_key_string()],
                "{member} is on the roster and must see the room's dot"
            );
        }
        let outsider = index
            .project_for(RUNNING_SET_TOPIC, Some(&payload), Some("u-mallory"), &store)
            .await
            .expect("projected");
        assert!(
            projected_keys(&outsider).is_empty(),
            "a non-member sees nothing of the room"
        );
    }

    /// A walled connection resolves no identity, so it must be told about
    /// nothing — the same fail-closed direction `event_admits` takes for
    /// `caller_user: None`, expressed as an empty array rather than a drop.
    #[tokio::test]
    async fn a_walled_connection_receives_an_empty_running_set() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("proj-walled");
        stamp_owner(&store, &key, "u-alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let payload = serde_json::json!({ "seq": 1, "running": [key.to_key_string()] });
        let projected = EventVisibilityIndex::new()
            .project_for(RUNNING_SET_TOPIC, Some(&payload), None, &store)
            .await
            .expect("still a frame");
        assert!(projected_keys(&projected).is_empty());
    }

    /// The 99% path. Every other topic returns `None` so the delivery loop
    /// forwards the bytes it already holds — including topics that DO carry a
    /// session identity, because those are answered by `event_admits`, not
    /// here. Two arms would mean two places to decide the same thing.
    #[tokio::test]
    async fn no_other_topic_is_projected() {
        let (store, _temp) = test_store();
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();
        let payload = serde_json::json!({
            "seq": 1,
            "running": ["agent:main:main"],
            "session_key": "agent:main:main",
            "run_id": "r1",
        });
        for topic in [
            "stream.session_updated",
            "stream.agent_trace",
            "stream.run_accepted",
            "team.t1.message",
            "session.lifecycle.changed",
            "channel.message",
            "some.topic.nobody.classified",
        ] {
            assert!(
                index
                    .project_for(topic, Some(&payload), Some("u-alice"), &store)
                    .await
                    .is_none(),
                "{topic} must not be rewritten — even carrying a `running` field"
            );
        }
    }

    /// The projected frame must keep publishing in the `{method, params}`
    /// STREAM wire form, because that is what makes the projection land on the
    /// bytes that go out.
    ///
    /// `server::handler::event_wire_form` inserts the projected payload at
    /// `.params` unconditionally — correct for a stream-form frame, whose
    /// payload already lives there. Lose the `stream_method()` and
    /// `event_bus::publish_frame` emits the bare `{topic, data}` form instead:
    /// the projection would be inserted into a NEW `params` key, the wrap
    /// branch would fire, and the ORIGINAL un-narrowed `running` array — every
    /// user's in-flight session keys — would ride out under `params.data`.
    /// Every test in this module would stay green, because they all project
    /// the payload directly rather than re-deriving the envelope.
    ///
    /// A cross-module coincidence held this together; this is the pin. It
    /// belongs beside the projection, not beside the delivery loop, because
    /// the projection is what silently becomes wrong.
    #[test]
    fn the_projected_frame_must_keep_its_stream_wire_form() {
        let frame = GatewayEventFrame::RunningSetChanged {
            seq: 1,
            running: vec!["agent:main:main".to_string()],
        };
        assert_eq!(
            frame.stream_method(),
            Some(RUNNING_SET_TOPIC),
            "a bare {{topic, data}} RunningSetChanged puts the un-narrowed \
             array back on the wire under `params.data`"
        );
    }
}
