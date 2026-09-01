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
//! a session key with no extra store round-trip. The pairing is retired by
//! capacity alone — the capacity-capped, insertion-order-evicting hygiene
//! `streaming/relay.rs`'s `StreamRegistry` already established for a similar
//! per-run cache — and explicitly NOT when the run ends, because
//! `RunComplete`/`RunError` are themselves resolved through it; see
//! [`EventVisibilityIndex::note_frame`] for what an end-of-run eviction cost.
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
//! `pairing.*` and `config.changed` carry (or could carry) a `session_key`, but
//! this module does NOT additionally owner-scope them: they are already
//! role-gated by `EventScopeGuard` (filter #1).
//!
//! ⚠️ The raw `approval.*` frames USED to be on that list, on the reasoning
//! that an exec approval for a MEMBER's session is resolved by an OPERATOR, so
//! a naive owner-equality check would deny the operator the very card they are
//! meant to act on. That reasoning was right and is preserved — but it was
//! leaning on a role gate that had to go: `Auto`, the DEFAULT tier, parks every
//! non-idempotent tool call, and a member had no principal allowed to release
//! their own, so every such call died at the approval timeout. Since 2026-08-08
//! those three topics are [`SessionIdentity::BySessionKeyOrAdmin`] (a real
//! `session_key`) or [`SessionIdentity::OperatorOnly`] (an empty one — a
//! cluster node raised it, it has no owner) and the `approval.` rule is gone
//! from `EventScopeGuard` — the protection moved down one filter rather than
//! being removed, and the operator's delivery is byte-for-byte what it was.
//! `surface.approval` — the R5 banner derived from `approval.requested` —
//! joined them once `r5_router::approval_for` stopped dropping the session key
//! on the way through. It was the last frame in the family still `Global`, and
//! the symptom was the same shape one rung out: a member received the decision
//! card and never the interrupt whose entire job is to fetch them to it.
//! `RunningSetChanged` carries a
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
use crate::gateway::visibility::owner_and_scope_visible_to;
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::utils::fifo_cache::{forget, remember};
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
    /// Attributable to no session and reserved to operators — a fleet-level
    /// fact with no owner to compare against.
    ///
    /// The delivery plane could not express this before 2026-08-08. Role was a
    /// property of the SECOND filter term only ([`crate::gateway::event_scope`]),
    /// which keys on the topic PREFIX — so a family carrying both per-session
    /// frames and fleet-level ones had to be all-or-nothing, and `approval.`
    /// was gated whole for that reason alone. The consequence was the inversion
    /// this variant exists to end: the approval bell and the resolve verb were
    /// both operator-only, so a member's own run blocked on a gate they could
    /// not see, died at the 120-second timeout, and the only way to get work
    /// done was `exec_tier: "full"` — the least safe tier being the only one
    /// that worked.
    ///
    /// Use it when a frame has no owner to resolve, not when resolving is
    /// merely inconvenient: `Global` remains the answer for facts everybody may
    /// have.
    OperatorOnly,
    /// Attributable to a USER directly, with no session in between — a live
    /// speech-to-text stream belongs to whoever is speaking, and is not a
    /// conversation yet.
    ByUserId(String),
    /// A frame that SHOULD carry an attribution and does not. Denied to every
    /// scoped caller, admitted to an unscoped one (internal / single-user).
    ///
    /// Distinct from [`Self::Global`] on purpose: `Global` means "everyone may
    /// have this", and folding an unstamped frame into it is how a missing
    /// producer becomes a broadcast. Distinct from a bare `false` because an
    /// unscoped process must keep working.
    Unattributed,
    /// The frame names its session, AND an admin receives it regardless of who
    /// owns that session.
    ///
    /// The one asymmetric answer in this enum, and it exists for exactly one
    /// workflow: an exec approval is a request for a HUMAN decision about
    /// someone's parked tool call, and an operator answering on a member's
    /// behalf is the point of the operator role — a plain
    /// [`Self::BySessionKey`] would deny the operator the very card they are
    /// meant to act on. The owner half is what makes the topic deliverable to
    /// members at all (they resolve their own; see
    /// `handlers::exec_approvals`), so this variant is a WIDENING for members
    /// and byte-for-byte unchanged for operators, who received these frames
    /// unconditionally before.
    ///
    /// Do not reach for this to make some other frame convenient for admins.
    /// Every other session-scoped frame answers "may this person read this
    /// person's work", and the answer to that is not "admins may read
    /// everything" — P1 deliberately does not grant that.
    BySessionKeyOrAdmin(String),
    /// A whiteboard frame (`canvas.updated`): attributable to the canvas's
    /// stamped owner plus — when project-linked — that room's roster.
    ///
    /// The frame SELF-REPORTS both halves (§4.8 mine H: a resolution handle
    /// must not be installed under narrower conditions than the frame is
    /// produced under — a canvas apply has no run or session to seed any
    /// index from, so the frame carries its own attribution). The arm in
    /// [`EventVisibilityIndex::event_admits_for`] only DELEGATES to
    /// [`crate::gateway::visibility::canvas_visible_to`] — the same predicate
    /// the RPC face (`canvas_visible`) and the tool face
    /// (`ambient_canvas_visible`) resolve, so the third face of the verb
    /// cannot drift (§0 "一个动词有 N 个面时，谁能看要在每个面用同一个推导").
    /// An absent `owner` reads as the legacy operator inside that predicate
    /// (`owner_or_legacy`), never as "everyone".
    ByCanvasScope {
        owner: Option<String>,
        project: Option<String>,
    },
    /// A project-room frame (`projects.changed`): attributable to a room's
    /// ROSTER, not to any one session or owner — P2's whole predicate is
    /// that membership decides visibility (`SECURITY.md`'s project-rooms
    /// section: "Visibility is the roster, full stop"; `owner_user_id`
    /// answers only owner-only VERBS, never "who can see this").
    ///
    /// The admit arm delegates to
    /// [`crate::gateway::visibility::project_or_removal_visible_to`] — the
    /// same [`crate::projects::roster::is_member`] call the RPC face
    /// (`projects.list`'s `visibility::project_visible` filter) and the
    /// partition/session twins reach, so this event face cannot drift from
    /// them (§0 "一个动词有 N 个面时，谁能看要在每个面用同一个推导"). There
    /// is deliberately NO admin/operator carve-out, unlike
    /// [`Self::BySessionKeyOrAdmin`]: an operator who is not on a room's
    /// roster cannot see that room's SESSIONS either
    /// (`owner_and_scope_visible_to` has no admin arm for a `Project` scope),
    /// and `canvas_visible_to`'s own doc states the general rule this
    /// mirrors — "the answer to 'may this person read this person's work' is
    /// not 'admins may read everything'". An operator who created the room
    /// (the common case) is already on its roster from creation
    /// (`ProjectStore::create` seats the owner), so they are admitted
    /// through membership, not through role.
    ByProjectScope {
        project_id: String,
        /// Mirrors `GatewayEventFrame::ProjectsChanged::affected_user`: set
        /// only for a member-removal frame, naming the user the roster
        /// predicate above can no longer admit.
        affected_user: Option<String>,
    },
    /// A `pty.screen` / `pty.exit` frame: attributable to the session's
    /// `created_by` stamp, resolved through
    /// [`crate::gateway::pty::PtyManager::owner_of`] rather than from the
    /// payload — `PtyScreenFrame` publishes at 60 Hz and its own wire-key
    /// test pins the exact set, so the owner is deliberately NOT one of
    /// them. This is the delivery-side half of the ownership rule
    /// `handlers::pty::require_owned` and `handle_list`'s filter enforce on
    /// the RPC side; all three consume the one predicate
    /// ([`crate::gateway::pty::owner_admits`]) so a hole in one face cannot
    /// exist without holing the others (§0's "一个动词有 N 个面时，谁能看要
    /// 在每个面用同一个推导").
    ///
    /// `owner_of` deliberately answers for a session that is already gone
    /// (see its own doc and `OWNER_RETENTION`): `pty.exit` is published one
    /// line before `PtyManager::remove`, and delivery is asynchronous, so a
    /// lookup keyed on live sessions alone would deny a client the one frame
    /// telling it its own shell died.
    ///
    /// This narrows WITHIN the operators the `pty.` prefix rule in
    /// [`crate::gateway::event_scope::EventScopeGuard`] already admits —
    /// exactly the second-filter-term shape `approval.*` uses
    /// ([`Self::BySessionKeyOrAdmin`]'s sibling family), except there is no
    /// admin carve-out here: an operator who did not create a shell has no
    /// standing claim on another operator's raw terminal, unlike an
    /// approval card written for them to act on.
    ByPtySession(String),
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
        // The two clarification frames are plain `BySessionKey`: they must
        // admit exactly whom their two RPC faces admit, and those faces are
        // OWNER-keyed — `clarification.pending` filters every item through
        // `visibility::session_visible` and `clarification.resolve` refuses a
        // session that same predicate rejects.
        //
        // ⚠️ This arm was `…OrAdmin` from 2026-08-08 to 2026-08-29, on the
        // stated premise that `session_visible` "is every session, for an
        // operator". It is not. `handlers::connect::resolve_connection_identity`
        // gives a loopback or admin connection `CALLER_USER =
        // Some(OWNER_USER_ID)`, never `None`, so `visible_owner_filter()` is
        // `Some(..)` for an operator and `session_visible` compares OWNERSHIP
        // for them exactly as for anyone else. The `is_none()` arm those two
        // handlers widen on is reachable only from an UNSCOPED internal caller
        // (cron / A2A / in-process test). The premise was written down three
        // times — here, in `every_frame_variant_is_classified`'s `expected()`,
        // and in the pin test's own assertion message — and was false in all
        // three, so the widening pushed a member's question CARD to an operator
        // who then got `session not found` from both `clarification.pending`
        // and `clarification.resolve`.
        //
        // The twin `approval.*` family stays `…OrAdmin`, and that is not an
        // inconsistency: ITS RPC faces really are role-keyed (`exec_approvals`
        // and `exec_grants` both ask `caller_identity::caller_is_member`). An
        // approval is an AUTHORIZATION question an operator can answer on
        // someone else's behalf; a clarification is a CONTENT question only the
        // asker can answer. Different question, different predicate — and the
        // pin below now DERIVES the expected verdict from the RPC predicate
        // instead of restating it, so this arm cannot drift from those faces
        // again without a red test.
        "stream.ask_user" | "stream.clarification_ended" => {
            match str_field(data, "session_key").filter(|k| !k.is_empty()) {
                Some(k) => SessionIdentity::BySessionKey(k),
                // Fail closed: a frame with no session names nobody, and the
                // widest possible delivery is the wrong answer to "who owns
                // this".
                None => SessionIdentity::OperatorOnly,
            }
        }

        // --- stream.* frames that carry their session key directly ---
        "stream.run_accepted"
        // A queued run's only frame before admission — see the field doc on
        // `StreamEvent::RunQueued` for why it carries `session_key` at all
        // (the run→session seed moves from admission to arrival). Grouped
        // with `stream.run_accepted` rather than the `ByRunId` family below
        // for the same reason: both are keyed by the session they were
        // ADDRESSED to, not correlated after the fact through a run id.
        | "stream.run_queued"
        | "stream.session_updated"
        // The peer-echo of a human's message. Session-scoped for the same
        // reason its transcript is: the audience that may read the row is
        // exactly the audience that may read the session it landed in.
        | "stream.session_user_message" => match str_field(data, "session_key") {
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

        // Live speech-to-text. `delta` is the TEXT OF WHAT THE SPEAKER SAID,
        // published incrementally, and this topic had no arm at all — so it
        // fell to `_ => Global` and every connection received it. The frame
        // names no session (a streaming transcription is not a conversation
        // yet), so it is attributed by the owner stamped at `StreamRegistry`'s
        // single mint point and carried in the payload.
        //
        // Deliberately fails CLOSED on an absent stamp for a scoped caller:
        // `ByUserId` with `None` denies, which is the right direction here
        // (this answers "may I be told what someone said", not "may I address
        // this key" — the `existing_session_is_visible` asymmetry runs the
        // other way and does not apply).
        //
        // ⚠️ Raw-string producer: `every_frame_variant_is_classified` is
        // structurally blind to it, so this arm owes the SOURCE-level pin
        // `the_voice_delta_topic_is_classified_at_its_producer`.
        "voice.transcribe.delta" => match str_field(data, "owner_user_id") {
            Some(owner) if !owner.is_empty() => SessionIdentity::ByUserId(owner),
            _ => SessionIdentity::Unattributed,
        },

        // A raw shell, admin-gated at the RPC face (`pty.` in
        // `method_admin::ADMIN_PREFIXES`) and at the role face
        // (`EventScopeGuard`'s `pty.` rule) — this arm is the third, and the
        // one that narrows WITHIN the operators those two already admit, the
        // same way `handlers::pty::require_owned`/`handle_list` narrow the
        // RPC face. See `SessionIdentity::ByPtySession`'s doc for why the
        // owner is resolved through `PtyManager::owner_of` rather than read
        // off the payload (`session_id` is the only field either frame
        // carries that names anything).
        //
        // ⚠️ Raw-string producer, same as `voice.transcribe.delta` above:
        // `every_frame_variant_is_classified` cannot see it, so this arm owes
        // the SOURCE-level pin
        // `every_pty_topic_the_center_publishes_is_owner_scoped`.
        "pty.screen" | "pty.exit" => match str_field(data, "session_id") {
            Some(id) => SessionIdentity::ByPtySession(id),
            // Malformed — neither frame is ever built without this field in
            // production. `OperatorOnly` rather than `Global`: there is no
            // owner to compare against, and the safe read of "we could not
            // tell whose this is" is the operator's, not everyone's.
            None => SessionIdentity::OperatorOnly,
        },

        // Host telemetry. The two producers this arm was written for —
        // `host.presence.update` (the machine's `hostname`, its OS `username`
        // and whether a human is sitting at it) and `host.mic_level.update`
        // (whether the room is making noise) — were deleted on 2026-08-09 for
        // having no subscriber. **The arm deliberately outlives them.**
        //
        // It is not plumbing waiting for a consumer; it is the default this
        // classifier applies to a whole namespace. Without it `host.*` falls to
        // `_ => Global`, and `EventScopeGuard` has no `host.` rule either, so
        // the next small host reporter someone writes reaches every
        // authenticated member and every filterless client on its first tick —
        // which is exactly what happened the first time, and what
        // `PresenceConfig`'s `default_enabled() == false` failed to prevent for
        // any operator who followed the doc and turned it on. Deleting a
        // fail-closed default because its first users are gone re-arms the
        // trap; a match arm costs one line and misleads no one, so R10's
        // retract-the-unused clause does not reach it.
        //
        // `OperatorOnly`, not a session key: the host belongs to whoever runs
        // the daemon, not to any conversation — there is nothing to attribute
        // it to. Structural prefix match rather than a topic whitelist, because
        // a whitelist only covers the world as it was on the day it was
        // written. Pinned by `the_host_namespace_stays_operator_only`.
        t if t.starts_with("host.") => SessionIdentity::OperatorOnly,

        // --- TopicEvent-form frames genuinely session-scoped and NOT
        // covered by any other filter today ---
        "session.lifecycle.changed" | "sessions.changed" => {
            match str_field(data, "session_key") {
                Some(k) => SessionIdentity::BySessionKey(k),
                None => SessionIdentity::Global,
            }
        }

        // --- The raw exec-approval frames: owner-scoped, admin-inclusive ---
        //
        // These were `Global` while `EventScopeGuard` refused them to anyone
        // but an operator, which made the role gate the whole of their
        // protection. That gate is gone (2026-08-08): a member has to receive
        // the card for their OWN parked tool call, because `Auto` — the
        // DEFAULT tier — parks every non-idempotent call and the member is now
        // the principal allowed to release it (`exec.` carve-out in
        // `method_admin`). With the role gate open, `Global` here would have
        // handed every member every other member's parked commands, so the
        // classification moved rather than the protection.
        //
        // The family carries two kinds of frame under one prefix: a tool-gate
        // approval names the blocked session; a cluster-node approval arrives
        // over reverse RPC, belongs to no local run, and is published with
        // `session_key: String::new()` (`approval/node_requester.rs`). The
        // discriminator is therefore STRUCTURAL — is there a session key —
        // rather than a guess about the requester.
        //
        // `surface.approval` is the fourth member of the family, not a
        // different question: it is the R5 BANNER leg derived from
        // `approval.requested` and now carries the same `session_key`. It was
        // the last one still `Global` + role-gated, which delivered it to
        // operators only — so a member whose own call was parked got the card
        // and no interrupt. Same discriminator, same two answers.
        // `approval.reminder` is the fifth member. It is a re-announcement of
        // `approval.requested`, carrying the same `approval_id` and the same
        // `session_key`, so it answers here exactly as the request does. Left
        // off this list it would have fallen to the `_ => Global` arm at the
        // bottom and broadcast one user's parked approval to every connection —
        // a leak the typed derivation above cannot catch, because the two are
        // separate matches over the same fact.
        "approval.requested"
        | "approval.reminder"
        | "approval.resolved"
        | "approval.expired"
        | "surface.approval" => {
            match str_field(data, "session_key").filter(|k| !k.is_empty()) {
                // A real session: the owner resolves their own, and the admin
                // arm keeps an operator receiving a member's card as before.
                Some(k) => SessionIdentity::BySessionKeyOrAdmin(k),
                // Fleet or malformed: no owner to compare against, so it is
                // the operator's. Fail closed — `Global` here would make a
                // malformed payload the widest possible delivery.
                None => SessionIdentity::OperatorOnly,
            }
        }
        "pairing.requested" | "pairing.completed" | "config.changed" => SessionIdentity::Global,

        // Whiteboard applies (`GatewayEventFrame::CanvasUpdated`). The frame
        // self-reports its owner and project link — there is no run/session
        // to resolve through any index — and the admit arm delegates to the
        // one canvas predicate. Absent fields extract as `None`: an unstamped
        // owner reads as the legacy operator inside `canvas_visible_to`, and
        // an unlinked canvas has no roster arm.
        aleph_protocol::canvas::TOPIC => SessionIdentity::ByCanvasScope {
            owner: str_field(data, "owner_user_id"),
            project: str_field(data, "project_id"),
        },

        // A project room's roster-visible state changed
        // (`GatewayEventFrame::ProjectsChanged`). `project_id` is always
        // present (it is the frame's own key, not an optional stamp); an
        // absent one classifies as `ByProjectScope` with an empty id, which
        // `project_or_removal_visible_to` denies to every scoped caller
        // (fail closed on a malformed frame, matching every other arm here).
        "projects.changed" => SessionIdentity::ByProjectScope {
            project_id: str_field(data, "project_id").unwrap_or_default(),
            affected_user: str_field(data, "affected_user"),
        },

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

        // The `workspace.` RPC family is admin-gated in `method_admin.rs` so a
        // member cannot enumerate workspaces; broadcasting the ids on the event
        // plane would hand back exactly what that gate withholds. `OperatorOnly`
        // in its documented sense and not as a shortcut: a workspace has no
        // owner column by decision, so there is no ownership to resolve.
        "workspace.changed" => SessionIdentity::OperatorOnly,

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

/// Cached team→(owner, scope) stamps for [`SessionIdentity::ByTeamId`].
///
/// The value is the team row's `owner_user_id`/`scope_id` pair VERBATIM, so
/// `(None, None)` here means a legacy/unstamped team — which reads as the
/// operator's through `visibility::owner_or_legacy` and ADMITS them. An id
/// that could not be resolved at all (absent team, store error) is
/// deliberately not stored: there is no invalidation hook on this cache, so a
/// cached "unresolvable" would outlive its cause. It is denied per frame and
/// re-resolved on the next one, exactly like an absent session row.
///
/// A team's owner and scope are stamped once in
/// `SqliteTeamStore::create_team` and no `TeamStore` method rewrites either,
/// which is what makes the pair cacheable for the process lifetime.
#[derive(Default)]
struct TeamOwnerCache {
    order: VecDeque<String>,
    map: HashMap<String, (Option<String>, Option<String>)>,
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

    /// Seed the run→session cache from a delivered frame. Called
    /// UNCONDITIONALLY (before filtering) on every connection's delivery
    /// loop, so the shared index stays warm regardless of which connection
    /// happens to process a given seeding frame first — first writer wins,
    /// and re-seeding an already-known run_id is harmless (same session_key
    /// every time for a given run).
    ///
    /// # The rule is "names both", not "is `RunAccepted`"
    ///
    /// This used to key off `topic == "stream.run_accepted"`, which made the
    /// resolver's reach **narrower than its frames' producers** — the shape
    /// `gateway/CLAUDE.md` calls landmine H. `RunAccepted` is emitted by
    /// `execute()`, i.e. after the admission gate; but the client is handed a
    /// `run_id` the moment `chat.send` returns, and `busy_queue::
    /// spawn_queued_run` emits a terminal `RunError` for the three outcomes
    /// where the run **never reaches the engine** (lane full, wait deadline,
    /// purged by a stop). Those frames classify `ByRunId`, resolved against a
    /// seed that by construction does not exist — so the filter fail-closed
    /// denied them to every connection, the operator's included. Three
    /// documented receipts were silently discarded, among them the one
    /// `cancel_queued_run` was written to deliver.
    ///
    /// Any frame naming BOTH `run_id` and `session_key` therefore seeds. The
    /// widening cannot be exploited: frames are built server-side, `run_id`s
    /// are never reused, and first-writer-wins means an honest producer cannot
    /// be overwritten by a later one. The alternative — teaching each new
    /// pre-admission frame to carry its own classification — is the enumeration
    /// this file's module doc already warns about, and it would have to be
    /// remembered once per frame.
    ///
    /// ⚠️ There is deliberately NO eviction arm for `RunComplete`/`RunError`,
    /// and re-adding one is not hygiene — it is a total outage of both frames.
    /// Being called before the filter is what makes seeding work; it is also
    /// what made evicting here fatal. The delivery loop calls this and THEN
    /// asks [`Self::event_admits_for`], and those two topics classify as
    /// [`SessionIdentity::ByRunId`] — so each terminal frame erased the seed
    /// its own authorization check was about to need and fail-closed denied
    /// itself. To EVERY connection, the run's owner included: the loop is per
    /// connection but this index is process-shared, so whichever one arrived
    /// first evicted on behalf of all the others. Nothing observable survived
    /// a run's end — no `run_complete`, therefore no cost/token summary, no
    /// `settle_run`, and a composer stuck "busy" until reload (2026-08-09
    /// real-machine QA). A finished run's entry costs one slot in a cache
    /// that is FIFO-capped at [`MAX_TRACKED_RUNS`] on its own, and run ids
    /// are never reused, so ageing out is the whole lifecycle it needs.
    pub async fn note_frame(&self, topic: &str, data: Option<&Value>) {
        // Only `stream.*` frames describe a run. Team topics carry a
        // `session_key`-shaped root and a fan-out tree id that is not an engine
        // run — seeding from those would map a tree id onto a session and hand
        // `ByRunId` an answer for a question it was never asked.
        if !topic.starts_with("stream.") {
            return;
        }
        // `session_key` first: this runs per frame per connection, and the
        // overwhelming majority of stream frames (chunks, traces, tool
        // lifecycle) carry a `run_id` and no session, so testing the rare
        // field first short-circuits them in one lookup.
        let Some(session_key) = str_field(data, "session_key") else {
            return;
        };
        let Some(run_id) = str_field(data, "run_id") else {
            return;
        };
        self.insert_run(run_id, session_key).await;
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
        caller_is_admin: bool,
        store: &Arc<dyn SessionStore>,
        teams: Option<&Arc<dyn TeamStore>>,
    ) -> bool {
        let caller_role = if caller_is_admin {
            Some("operator")
        } else {
            Some("member")
        };
        self.event_admits_for(topic, data, caller_user, caller_role, store, teams)
            .await
    }

    /// [`Self::event_admits`] with the caller's ROLE supplied explicitly — the
    /// exact twin of `visibility::session_visible` / `session_visible_to`, and
    /// for the same reason: one body, two ways of naming the actor.
    ///
    /// The role is read here rather than at the [`EventScopeGuard`] term
    /// because that term keys on the topic PREFIX and this decision depends on
    /// the PAYLOAD (see [`SessionIdentity::OperatorOnly`]). Production always
    /// calls this one — the delivery loop already holds the connection's
    /// `caller_role` under the same lock it reads `caller_user` from.
    ///
    /// `caller_role: None` means "no role information", which
    /// [`role_is_operator`](crate::tools::turn_context::role_is_operator) reads
    /// as trusted local/internal — the repo-wide convention, and why the
    /// boolean shim above is safe for internal callers rather than a hole.
    pub async fn event_admits_for(
        &self,
        topic: &str,
        data: Option<&Value>,
        caller_user: Option<&str>,
        caller_role: Option<&str>,
        store: &Arc<dyn SessionStore>,
        teams: Option<&Arc<dyn TeamStore>>,
    ) -> bool {
        match session_identity_of(topic, data) {
            SessionIdentity::Global => true,
            SessionIdentity::OperatorOnly => {
                crate::tools::turn_context::role_is_operator(caller_role)
            }
            // Direct owner compare — the same shape `team_admits` uses once it
            // has resolved a team's owner, with no store round-trip because the
            // producer already stamped the answer into the payload.
            SessionIdentity::ByUserId(owner) => caller_user == Some(owner.as_str()),
            // A scoped caller is denied; an unscoped one (internal, or a
            // single-user box where nothing resolves an identity) is not.
            SessionIdentity::Unattributed => caller_user.is_none(),
            SessionIdentity::BySessionKeyOrAdmin(session_key) => {
                if crate::tools::turn_context::role_is_operator(caller_role) {
                    return true;
                }
                let Some(caller) = caller_user else {
                    return false;
                };
                self.session_admits(&session_key, caller, store).await
            }
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
            // Only delegation, same reason as the canvas arm below: this is
            // the delivery-side face of `handlers::pty::require_owned` /
            // `handle_list`'s filter, and it must resolve the SAME predicate
            // rather than re-derive ownership here. `owner_of` answers for a
            // session that is already gone — see `SessionIdentity::ByPtySession`'s
            // doc — so there is no store round-trip and no "unresolvable"
            // case to fail closed on the way `ByRunId` does.
            SessionIdentity::ByPtySession(session_id) => crate::gateway::pty::manager()
                .owner_of(&session_id)
                .admits(caller_user),
            // Only delegation, by ruling: this is the third face of the
            // canvas verb, and it must resolve the SAME predicate as the RPC
            // and tool faces rather than re-derive membership here. The
            // predicate's `actor == None ⇒ unrestricted` convention is
            // intentional for this arm too — an unscoped delivery loop
            // (internal/single-user wiring) matches the unscoped RPC caller
            // byte for byte.
            SessionIdentity::ByCanvasScope { owner, project } => {
                crate::gateway::visibility::canvas_visible_to(
                    owner.as_deref(),
                    project.as_deref(),
                    caller_user,
                )
            }
            // Only delegation, same ruling as the canvas arm above: the
            // roster predicate is the single authority for "who can see this
            // room", and this event face must resolve it rather than
            // re-derive membership here.
            SessionIdentity::ByProjectScope {
                project_id,
                affected_user,
            } => crate::gateway::visibility::project_or_removal_visible_to(
                &project_id,
                affected_user.as_deref(),
                caller_user,
            ),
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
        if let Some((owner, scope)) = {
            let inner = self.team_owners.read().await;
            inner.map.get(team_id).cloned()
        } {
            return crate::gateway::visibility::owner_and_scope_visible_to(
                owner.as_deref(),
                scope.as_deref(),
                caller,
            );
        }

        // No team database wired ⇒ nothing can answer "whose team is this".
        let Some(store) = teams else {
            return false;
        };
        let (owner, scope) = match store.get_team(team_id).await {
            Ok(Some(team)) => (team.owner_user_id, team.scope_id),
            // Absent (deleted mid-fan-out, or an id from a producer this
            // deployment does not have) or a store error: deny this frame and
            // re-resolve on the next, never cache the denial (see
            // `TeamOwnerCache`).
            Ok(None) | Err(_) => return false,
        };
        self.cache_team_owner(team_id.to_string(), owner.clone(), scope.clone())
            .await;
        crate::gateway::visibility::owner_and_scope_visible_to(
            owner.as_deref(),
            scope.as_deref(),
            caller,
        )
    }

    async fn cache_team_owner(
        &self,
        team_id: String,
        owner: Option<String>,
        scope: Option<String>,
    ) {
        let mut inner = self.team_owners.write().await;
        let TeamOwnerCache { order, map } = &mut *inner;
        remember(order, map, team_id, (owner, scope), MAX_CACHED_TEAM_OWNERS);
    }

    /// Drop the cached ownership / scope for `session_key` so the next
    /// `session_admits` call falls through to `SessionStore::get_metadata`
    /// and re-derives the truth. Required after any session row mutation
    /// (`sessions.delete`, `sessions.patch`, `sessions.compaction.restore`,
    /// `sessions.set_project_root`) that may change `owner_user_id` or
    /// `scope_id` — without it the cache keeps serving the pre-mutation
    /// pair until FIFO eviction.
    ///
    /// Idempotent: dropping a missing key is a no-op. Never wired into the
    /// per-frame read path.
    pub async fn forget_session(&self, session_key: &str) {
        let mut inner = self.owners.write().await;
        let OwnershipCache { order, map } = &mut *inner;
        forget(order, map, session_key);
    }

    /// Drop the cached team ownership for `team_id` so the next
    /// `team_admits` call falls through to `TeamStore::get_team` and
    /// re-derives the truth. Required after any team row mutation that may
    /// change `owner_user_id` or `scope_id`.
    pub async fn forget_team(&self, team_id: &str) {
        let mut inner = self.team_owners.write().await;
        let TeamOwnerCache { order, map } = &mut *inner;
        forget(order, map, team_id);
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
    use crate::gateway::source_census;
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

    /// A run that never reached the engine has no `RunAccepted`, so the only
    /// frame it will ever produce is its own terminal `RunError` — and that
    /// frame classifies `ByRunId`, against a seed nothing could have written.
    ///
    /// This is the whole failure `busy_queue::spawn_queued_run` was silently
    /// suffering: the lane-full rejection, the wait-deadline timeout and the
    /// stop-purge receipt were each built, emitted, and then denied to every
    /// connection — the run's own owner included. The frame now names its
    /// session, which both seeds the index (this call) and makes the lookup on
    /// the very next line succeed.
    #[tokio::test]
    async fn a_queued_run_that_never_reached_the_engine_can_still_report_its_failure() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-queued");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);

        // Exactly what `spawn_queued_run` puts on the wire for `Rejected` /
        // `TimedOut` / `Purged`. Note the absence of any prior frame.
        let run_error = serde_json::json!({
            "run_id": "r-never-admitted",
            "seq": 0,
            "error": "Agent is busy",
            "error_code": "AGENT_BUSY",
            "session_key": key.to_key_string(),
        });

        let index = EventVisibilityIndex::new();
        index.note_frame("stream.run_error", Some(&run_error)).await;
        assert!(
            index
                .event_admits(
                    "stream.run_error",
                    Some(&run_error),
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await,
            "the owner must receive the receipt for their own dropped message"
        );
        assert!(
            !index
                .event_admits(
                    "stream.run_error",
                    Some(&run_error),
                    Some("bob"),
                    false,
                    &store,
                    None
                )
                .await,
            "naming the session must not widen the audience beyond that session"
        );

        // …and the counter-example that pins WHY the field exists: the same
        // frame without it is denied to its own owner, because `ByRunId` has
        // nothing to resolve against.
        let unnamed = serde_json::json!({
            "run_id": "r-also-never-admitted",
            "seq": 0,
            "error": "Agent is busy",
            "error_code": "AGENT_BUSY",
        });
        let bare = EventVisibilityIndex::new();
        bare.note_frame("stream.run_error", Some(&unnamed)).await;
        assert!(
            !bare
                .event_admits(
                    "stream.run_error",
                    Some(&unnamed),
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await,
            "an unresolvable run id still fails closed — the seed is what changed, not the rule"
        );
    }

    /// `RunQueued` is a run's FIRST frame — the first chance to seed the
    /// run→session index, and the reason the seed moved from admission to
    /// arrival. `note_frame` is generic over "names both", so this works with
    /// no code mentioning the variant, which is exactly why it needs a test:
    /// renaming the field or nesting it breaks the seed silently and every
    /// queued-run frame then fails closed for everyone, its owner included.
    #[tokio::test]
    async fn a_queued_frame_reaches_its_session_and_nobody_else() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-still-waiting");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let run_queued = serde_json::json!({
            "run_id": "r-still-waiting",
            "session_key": key.to_key_string(),
            "ahead": 1,
        });

        let index = EventVisibilityIndex::new();
        index
            .note_frame("stream.run_queued", Some(&run_queued))
            .await;
        assert!(
            index
                .event_admits(
                    "stream.run_queued",
                    Some(&run_queued),
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await,
            "the owner must see their own message waiting"
        );
        assert!(
            !index
                .event_admits(
                    "stream.run_queued",
                    Some(&run_queued),
                    Some("bob"),
                    false,
                    &store,
                    None
                )
                .await,
            "naming the session must not widen the audience beyond it"
        );
    }

    /// Seeding is keyed on "names both", not on a topic allowlist — but only
    /// within `stream.*`. A team fan-out tree id is not an engine run, and
    /// mapping one onto a session would answer a `ByRunId` question that was
    /// never asked about it.
    #[tokio::test]
    async fn only_stream_frames_seed_the_run_index() {
        let index = EventVisibilityIndex::new();
        let looks_like_a_run = serde_json::json!({
            "run_id": "fanout-tree-1",
            "session_key": SessionKey::main("conv-x").to_key_string(),
        });
        index
            .note_frame("team.t1.fanout", Some(&looks_like_a_run))
            .await;
        assert_eq!(
            index.session_key_for_run("fanout-tree-1").await,
            None,
            "a non-stream topic must not write the run→session index"
        );

        index
            .note_frame("stream.run_error", Some(&looks_like_a_run))
            .await;
        assert!(
            index.session_key_for_run("fanout-tree-1").await.is_some(),
            "the same payload on a stream topic does seed"
        );
    }

    /// The producer half of the pair above. `spawn_queued_run` is the one
    /// `RunError` producer whose run never reached the engine, so it is the one
    /// that must name its session; the runtime tests cannot reach it (it needs
    /// a live `ExecutionEngine` and an `AgentInstance`), and dropping the field
    /// there would silently restore the outage while every test above stays
    /// green.
    #[test]
    fn the_queued_run_receipt_names_its_session() {
        let src = include_str!("busy_queue/spawn.rs");
        let ctor = src
            .split("StreamEvent::RunError {")
            .nth(1)
            .expect("spawn_queued_run still emits a RunError");
        let ctor = &ctor[..ctor.find("})").unwrap_or(ctor.len())];
        assert!(
            ctor.contains("session_key: Some("),
            "the queued-run receipt must name its session or the delivery \
             filter drops it before any client sees it; found:\n{ctor}"
        );
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                    false,
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
                .event_admits(
                    "sessions.changed",
                    Some(&data),
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await
        );
        assert!(
            !index
                .event_admits(
                    "sessions.changed",
                    Some(&data),
                    Some("bob"),
                    false,
                    &store,
                    None
                )
                .await
        );
    }

    /// The approval plane's whole ruling in one test, and the home of the
    /// assertion `event_scope`'s role tests used to make.
    ///
    /// Four principals, four different right answers:
    /// - the session's owner — YES, this is the carve-out's entire purpose
    ///   (`Auto` is the default tier, so their own non-idempotent tool calls
    ///   park, and they are now the principal allowed to release them);
    /// - any admin — YES, unchanged; an operator answering on a member's behalf
    ///   is what the operator role is for, and a plain owner check would have
    ///   taken it away;
    /// - another member — NO; this is what stops the role gate's removal from
    ///   being a leak;
    /// - a walled / chat-tier connection carrying no user — NO, and NOT because
    ///   a permission list said so: it resolves no owner, and unresolvable
    ///   fails closed.
    #[tokio::test]
    async fn an_approval_frame_reaches_its_owner_and_every_admin_and_nobody_else() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-approval");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        let data = serde_json::json!({
            "approval_id": "a1",
            "session_key": key.to_key_string(),
            "channel_id": "",
            "conversation_id": "",
        });

        for topic in [
            "approval.requested",
            "approval.resolved",
            "approval.expired",
        ] {
            assert!(
                index
                    .event_admits(topic, Some(&data), Some("alice"), false, &store, None)
                    .await,
                "{topic}: the owner must receive the card for their own parked \
                 tool call — without it the default tier is a dead end"
            );
            assert!(
                index
                    .event_admits(topic, Some(&data), Some("bob"), true, &store, None)
                    .await,
                "{topic}: an admin must still receive a member's card — that is \
                 the workflow the previous `Global` ruling protected"
            );
            assert!(
                !index
                    .event_admits(topic, Some(&data), Some("bob"), false, &store, None)
                    .await,
                "{topic}: another member must NOT receive alice's card — this is \
                 what replaces the role gate, not something added on top of it"
            );
            assert!(
                !index
                    .event_admits(topic, Some(&data), None, false, &store, None)
                    .await,
                "{topic}: a walled / chat-tier connection resolves no owner and \
                 must fail closed"
            );
        }

        // A payload with no session at all: admins only, never a broadcast.
        // `Global` here would make a malformed frame the widest delivery there
        // is, which is the opposite of what a missing field should buy.
        let headless = serde_json::json!({ "approval_id": "a2" });
        assert!(
            !index
                .event_admits(
                    "approval.requested",
                    Some(&headless),
                    Some("alice"),
                    false,
                    &store,
                    None
                )
                .await,
            "an unresolvable approval frame must not reach a non-admin"
        );
        assert!(
            index
                .event_admits(
                    "approval.requested",
                    Some(&headless),
                    None,
                    true,
                    &store,
                    None
                )
                .await,
            "an admin still receives it — they are the fallback resolver"
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
                    .event_admits("tools.changed", None, caller, false, &store, None)
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
                        false,
                        &sessions,
                        Some(&teams)
                    )
                    .await,
                "{topic}: the team's owner must still receive her own team's frames"
            );
            assert!(
                !index
                    .event_admits(
                        &topic,
                        Some(&body),
                        Some("u-bob"),
                        false,
                        &sessions,
                        Some(&teams)
                    )
                    .await,
                "{topic}: a second logged-in user must not receive another user's team chat"
            );
            assert!(
                !index
                    .event_admits(
                        &topic,
                        Some(&body),
                        Some(OWNER_USER_ID),
                        false,
                        &sessions,
                        Some(&teams)
                    )
                    .await,
                "{topic}: the operator is not exempt from team ownership either"
            );
            assert!(
                !index
                    .event_admits(&topic, Some(&body), None, false, &sessions, Some(&teams))
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
                .event_admits(
                    &topic,
                    None,
                    Some(OWNER_USER_ID),
                    false,
                    &sessions,
                    Some(&teams)
                )
                .await,
            "an unstamped team belongs to the legacy operator — loopback must still \
             see its own team chat"
        );
        assert!(
            !index
                .event_admits(&topic, None, Some("u-bob"), false, &sessions, Some(&teams))
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
                        false,
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
                .event_admits(
                    &topic,
                    None,
                    Some("u-alice"),
                    false,
                    &sessions,
                    Some(&teams)
                )
                .await
        );
        assert!(
            index
                .event_admits(
                    &topic,
                    None,
                    Some("u-alice"),
                    false,
                    &sessions,
                    Some(&teams)
                )
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
                    .event_admits("team.t1.message", None, caller, false, &sessions, None)
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
    /// SOURCE-level pin for the voice relay, owed for the same reason the team
    /// one is: `voice.transcribe.delta` is published as a raw
    /// `TopicEvent::new("…")` string with no `GatewayEventFrame` variant, so
    /// `every_frame_variant_is_classified` is structurally blind to it — which
    /// is exactly how it sat on `_ => Global`, broadcasting the text of what
    /// one user said to every connection, for as long as nobody looked.
    ///
    /// Reads the producer's own source so that renaming the topic on one side
    /// fails here rather than silently re-broadcasting.
    #[test]
    fn the_voice_delta_topic_is_classified_at_its_producer() {
        const RELAY: &str = include_str!("voice/streaming/relay.rs");
        let production = source_census::production_prefix(RELAY);
        let topics = source_census::topic_event_literals(&production);

        assert!(
            !topics.is_empty(),
            "relay.rs publishes no TopicEvent — the scanner stopped matching \
             the call shape, so this pin has quietly become vacuous"
        );
        for topic in &topics {
            let owned = serde_json::json!({ "owner_user_id": "u-alice" });
            assert_eq!(
                session_identity_of(topic, Some(&owned)),
                SessionIdentity::ByUserId("u-alice".to_string()),
                "`{topic}` carries live speech and must reach its speaker only"
            );
            // No stamp ⇒ denied to anyone scoped, NOT broadcast. `Global` here
            // would be the original bug with an arm in front of it.
            assert_eq!(
                session_identity_of(topic, None),
                SessionIdentity::Unattributed,
                "`{topic}` without an owner stamp must fail closed"
            );
        }
    }

    /// SOURCE-level pin, the constant-topic sibling of the voice test above:
    /// `pty.screen` / `pty.exit` are published through
    /// `aleph_protocol::pty::PTY_SCREEN_TOPIC` / `PTY_EXIT_TOPIC` rather than
    /// string literals, so `topic_event_literals` — which deliberately skips
    /// a composed first argument (see its own doc) — cannot scrape them.
    /// This scans for the constant names directly, for the same reason the
    /// voice pin reads the producer's own source: renaming or dropping the
    /// classification arm must fail here, not silently re-broadcast a raw
    /// shell to every connection.
    ///
    /// Deliberately NOT anchored to `TopicEvent::new(` on the same line —
    /// `session.rs`'s call wraps the constant onto its own line once rustfmt
    /// widens it, which is exactly the brittleness `source_census`'s module
    /// doc records breaking the old literal-scraper. Each producer file is
    /// asserted to contain BOTH the call shape and the constant name, which
    /// is loose enough to survive reformatting and specific enough that
    /// deleting the call (not just moving it) still fails the assertion.
    #[test]
    fn every_pty_topic_the_center_publishes_is_owner_scoped() {
        const MANAGER: &str = include_str!("pty/manager.rs");
        const SESSION: &str = include_str!("pty/session.rs");

        for (file, src, const_name) in [
            ("pty/manager.rs", MANAGER, "PTY_SCREEN_TOPIC"),
            ("pty/session.rs", SESSION, "PTY_EXIT_TOPIC"),
        ] {
            let production = source_census::production_prefix(src);
            assert!(
                production.contains("TopicEvent::new("),
                "{file} no longer publishes any TopicEvent — this pin has \
                 quietly become vacuous"
            );
            assert!(
                production.contains(const_name),
                "{file} no longer references aleph_protocol::pty::{const_name} \
                 — either it stopped publishing that topic, or it started \
                 publishing a bare string literal that this scan and \
                 `session_identity_of`'s pty arm could silently disagree about"
            );
        }

        for topic in [
            aleph_protocol::pty::PTY_SCREEN_TOPIC,
            aleph_protocol::pty::PTY_EXIT_TOPIC,
        ] {
            let named = serde_json::json!({ "session_id": "s-owner-scoped" });
            assert_eq!(
                session_identity_of(topic, Some(&named)),
                SessionIdentity::ByPtySession("s-owner-scoped".to_string()),
                "`{topic}` carries a live shell's screen/status and must be \
                 owner-scoped through PtyManager::owner_of, not broadcast"
            );
            // A malformed frame (no `session_id`) must not fall to `Global`
            // either — see the arm's own doc for why `OperatorOnly` and not
            // a broadcast is the safe read of "we could not tell whose this
            // is".
            assert_ne!(
                session_identity_of(topic, None),
                SessionIdentity::Global,
                "`{topic}` without a session id must not fall through to Global"
            );
        }
    }

    /// `pty.screen` / `pty.exit`'s full delivery chain (`event_admits_for`,
    /// not classification alone), through the REAL global `PtyManager` —
    /// `ByPtySession` resolves via `pty::manager()`, so nothing shorter than
    /// the actual singleton exercises the wire this test is for.
    ///
    /// Covers the property `handlers::pty`'s own tests cannot: F2 named FOUR
    /// facts that only matter together, and the addressed-method tests only
    /// cover the first two (the RPC methods, `pty.list`'s filter). This is
    /// the third — a second operator must not receive the FIRST operator's
    /// live screen frames either, which is the one a client-side `WrongSession`
    /// filter cannot be trusted for (the frames already arrived; see
    /// `handlers::pty`'s module doc).
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn pty_frames_reach_only_the_owner_and_outlive_the_session() {
        let (store, _temp) = test_store();
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        let spawn = crate::gateway::pty::manager()
            .spawn(&crate::gateway::pty::SpawnOptions {
                created_by: Some("u-alice".to_string()),
                ..Default::default()
            })
            .expect("spawn");
        let sid = spawn.session_id.clone();

        for topic in [
            aleph_protocol::pty::PTY_SCREEN_TOPIC,
            aleph_protocol::pty::PTY_EXIT_TOPIC,
        ] {
            let data = serde_json::json!({ "session_id": sid });
            assert!(
                index
                    .event_admits_for(
                        topic,
                        Some(&data),
                        Some("u-alice"),
                        Some("operator"),
                        &store,
                        None
                    )
                    .await,
                "the creator must receive their own `{topic}` frames"
            );
            assert!(
                !index
                    .event_admits_for(
                        topic,
                        Some(&data),
                        Some("u-bob"),
                        Some("operator"),
                        &store,
                        None
                    )
                    .await,
                "a second OPERATOR must not receive another operator's \
                 `{topic}` — role admits the topic prefix, ownership is a \
                 separate filter term and this arm has no admin carve-out"
            );
        }

        // `pty.exit` fires one line before `remove` and delivery is async —
        // the owner must still be admitted for a session already gone.
        crate::gateway::pty::manager().remove(&sid);
        let data = serde_json::json!({ "session_id": sid });
        assert!(
            index
                .event_admits_for(
                    aleph_protocol::pty::PTY_EXIT_TOPIC,
                    Some(&data),
                    Some("u-alice"),
                    Some("operator"),
                    &store,
                    None
                )
                .await,
            "the owner must still receive pty.exit for their own session \
             after PtyManager::remove — see OWNER_RETENTION's doc"
        );

        // An id nothing ever spawned is Unknown: fails closed for a scoped
        // caller, matching `SessionOwner::Unknown::admits`.
        let unknown = serde_json::json!({ "session_id": "never-existed" });
        assert!(
            !index
                .event_admits_for(
                    aleph_protocol::pty::PTY_SCREEN_TOPIC,
                    Some(&unknown),
                    Some("u-alice"),
                    Some("operator"),
                    &store,
                    None
                )
                .await
        );
    }

    /// The `host.` namespace stays operator-only even with zero producers.
    ///
    /// This test used to read the topic literal out of each of the two host
    /// reporters' own source. Those reporters are gone (2026-08-09), and the
    /// naive follow-up is to delete their classification with them — which
    /// puts `host.*` back on `_ => Global` and hands the next host reporter
    /// the original bug on its first tick. So the pin changed shape rather
    /// than being retired: it no longer names a producer, it asserts the
    /// **policy** that survives them.
    ///
    /// The clarification event face must admit exactly whom its RPC faces admit
    /// — and this pin DERIVES the expected verdict from the RPC predicate
    /// rather than restating it as a literal `SessionIdentity`.
    ///
    /// That distinction is the whole point. The previous version of this test
    /// asserted the literal `BySessionKeyOrAdmin` and explained itself with
    /// "`clarification.pending` already lists it to both, and
    /// `clarification.resolve` already accepts it from both" — a sentence about
    /// ANOTHER MODULE'S behaviour, restated here by hand, that was false.
    /// `resolve_connection_identity` scopes an operator's connection with
    /// `CALLER_USER = Some(OWNER_USER_ID)` (loopback and every admin-bound
    /// device alike), so `visible_owner_filter()` is `Some(..)` for them and
    /// both RPC faces compare ownership. A restated fact has no compiler; three
    /// copies of it drifted together and the event face shipped one rung wider
    /// than the two verbs it was pinned to.
    ///
    /// So: build a session owned by someone else, put the caller in an
    /// OPERATOR's real task-local shoes, and require the two faces to return
    /// the same boolean. Whichever way a future change moves the policy, it can
    /// only move both faces at once.
    ///
    /// The empty-key arm stays a literal: a frame that names no session names
    /// nobody, there is no RPC verdict to derive it from, and `Global` there
    /// would make a malformed payload the widest possible delivery.
    #[tokio::test]
    async fn the_clarification_frames_admit_the_same_callers_their_rpc_faces_do() {
        use crate::gateway::security::store::OWNER_USER_ID;

        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-clarify-owned-by-a-member");
        stamp_owner(&store, &key, "u-alice").await;
        let meta = store.get_metadata(&key).await.unwrap().unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        // The RPC half, evaluated exactly as `clarification.pending` /
        // `clarification.resolve` evaluate it — through the ONE predicate they
        // both call, with the actor an operator connection actually carries.
        let rpc_admits = crate::gateway::visibility::session_visible_to(&meta, OWNER_USER_ID);

        for topic in ["stream.ask_user", "stream.clarification_ended"] {
            let payload = serde_json::json!({ "session_key": key.to_key_string() });
            let event_admits = index
                .event_admits_for(
                    topic,
                    Some(&payload),
                    Some(OWNER_USER_ID),
                    Some("operator"),
                    &store,
                    None,
                )
                .await;
            assert_eq!(
                event_admits, rpc_admits,
                "`{topic}` must admit an operator exactly when \
                 `visibility::session_visible_to` does — the event face said \
                 {event_admits}, the RPC faces say {rpc_admits}. Move both or \
                 neither."
            );

            // The owner of the session is admitted on both faces; this arm
            // proves the equality above is not vacuously `false == false`.
            assert!(
                index
                    .event_admits_for(
                        topic,
                        Some(&payload),
                        Some("u-alice"),
                        Some("member"),
                        &store,
                        None,
                    )
                    .await,
                "`{topic}` must always reach the asker — a parked tool whose \
                 card nobody receives is a 600s stall"
            );
            assert!(
                crate::gateway::visibility::session_visible_to(&meta, "u-alice"),
                "self-guard: the RPC predicate must admit the owner, else the \
                 assertion above is comparing two unrelated falsehoods"
            );

            assert_eq!(
                session_identity_of(topic, Some(&serde_json::json!({ "session_key": "" }))),
                SessionIdentity::OperatorOnly,
                "`{topic}` with an empty session key names nobody; fail closed"
            );
            assert_eq!(
                session_identity_of(topic, None),
                SessionIdentity::OperatorOnly,
                "`{topic}` with no payload names nobody; fail closed"
            );
        }
    }

    /// The unregistered names below matter more than any real topic would:
    /// they are what an author who has never read this file will invent.
    #[test]
    fn the_host_namespace_stays_operator_only() {
        for topic in [
            "host.presence.update",
            "host.mic_level.update",
            "host.battery.update",
            "host.anything.someone.adds.later",
        ] {
            assert_eq!(
                session_identity_of(topic, None),
                SessionIdentity::OperatorOnly,
                "`{topic}` is host telemetry: it belongs to whoever runs the \
                 daemon, not to every connected member. If this failed because \
                 the `host.` arm was cleaned up as unused, read its doc — the \
                 arm is the default, not plumbing for a deleted feature."
            );
        }

        // The prefix must be a prefix, not a substring: a topic that merely
        // contains "host." elsewhere is a different question and must not be
        // silently narrowed to operators.
        assert_ne!(
            session_identity_of("session.host.changed", None),
            SessionIdentity::OperatorOnly,
            "the host rule is anchored at the start of the topic"
        );
    }

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
    /// The approval family's two shapes, pinned on the classifier directly.
    ///
    /// The discriminator is deliberately STRUCTURAL — "is there a session key"
    /// — and not a topic list: a suffix whitelist only covers the world as of
    /// the day it was written, which is how `team.*` stayed on the broadcast
    /// path for a whole arc. A fourth `approval.*` topic added tomorrow gets
    /// the right answer for free.
    #[test]
    fn an_approval_is_scoped_by_its_session_and_fleet_approvals_are_operator_only() {
        for topic in [
            "approval.requested",
            "approval.resolved",
            "approval.expired",
            // The R5 banner. It is the same question — "whose approval is
            // this" — and used to be answered `Global` + role-gated purely
            // because the frame had dropped the session key on its way through
            // `r5_router::approval_for`.
            "surface.approval",
        ] {
            let owned = serde_json::json!({ "session_key": "agent:main:s1" });
            assert_eq!(
                session_identity_of(topic, Some(&owned)),
                SessionIdentity::BySessionKeyOrAdmin("agent:main:s1".to_string()),
                "{topic} names a session, so that session's roster — or an admin \
                 acting on it — decides"
            );

            // `node_requester` publishes exactly this: an approval for a
            // command a cluster node wants to run, owned by no local session.
            let fleet = serde_json::json!({ "session_key": "" });
            assert_eq!(
                session_identity_of(topic, Some(&fleet)),
                SessionIdentity::OperatorOnly,
                "{topic} with an empty session_key is a FLEET approval — there \
                 is no owner to compare against, so it is the operator's"
            );

            // A malformed frame carrying no key at all is treated as fleet,
            // i.e. the narrower answer. Fail closed toward fewer recipients.
            assert_eq!(
                session_identity_of(topic, None),
                SessionIdentity::OperatorOnly
            );
        }
    }

    /// Every `approval.*` topic the frame enum can emit is named by the
    /// owner-scoped arm of [`session_identity_of`].
    ///
    /// The arm is a hand-written list of topic STRINGS, while the frames are an
    /// enum — two derivations of one fact, and only one of them has a compiler.
    /// Adding a variant to the family forces an arm in
    /// `every_frame_variant_is_classified`'s exhaustive `expected()`, and forces
    /// nothing at all here: the new topic falls through to `_ =>
    /// SessionIdentity::Global` and is broadcast to every connection, carrying
    /// an approval id and the session key of whoever it belongs to. That is how
    /// `approval.reminder` was nearly shipped.
    ///
    /// So this reads the topic names out of `frame.rs` rather than listing them
    /// — a sixth member of the family reds this test by name on the day it is
    /// written, not on the day someone audits the arm.
    #[test]
    fn every_approval_topic_is_owner_scoped() {
        let frames = crate::utils::source_scan::production_prefix(include_str!(
            "../gateway/events/frame.rs"
        ));
        // `Self::X { .. } => "approval.y",` — the one place a variant's topic
        // is decided.
        let topics: Vec<String> = frames
            .split('"')
            .filter(|t| t.starts_with("approval."))
            .map(str::to_string)
            .collect();
        assert!(
            topics.len() >= 4,
            "self-guard: expected at least the four historical approval topics,              scanned {topics:?} — a scan that finds nothing passes every              assertion below vacuously"
        );

        let arm = crate::utils::source_scan::production_prefix(include_str!("event_visibility.rs"));
        for topic in &topics {
            // Classification, not just spelling: a real session key must reach
            // its owner, and a blank one must stay with the operator.
            assert_eq!(
                session_identity_of(
                    topic,
                    Some(&serde_json::json!({"session_key": "agent:main:s1"}))
                ),
                SessionIdentity::BySessionKeyOrAdmin("agent:main:s1".to_string()),
                "{topic} must be owner-scoped, not Global"
            );
            assert_eq!(
                session_identity_of(topic, Some(&serde_json::json!({"session_key": ""}))),
                SessionIdentity::OperatorOnly,
                "{topic} with no owner must fail closed to the operator"
            );
            assert!(
                arm.contains(&format!("\"{topic}\"")),
                "{topic} is not named in session_identity_of's arm — it would                  fall to `_ => Global`"
            );
        }
    }

    #[test]
    fn every_frame_variant_is_classified() {
        fn expected(frame: &GatewayEventFrame) -> SessionIdentity {
            match frame {
                GatewayEventFrame::RunAccepted { session_key, .. }
                | GatewayEventFrame::RunQueued { session_key, .. } => {
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
                // The clarification pair is owner-scoped because its two RPC
                // faces are — see
                // `the_clarification_frames_admit_the_same_callers_their_rpc_faces_do`,
                // which DERIVES that verdict from the RPC predicate rather than
                // restating it.
                GatewayEventFrame::AskUser { session_key, .. }
                | GatewayEventFrame::ClarificationEnded { session_key, .. } => {
                    SessionIdentity::BySessionKey(session_key.clone())
                }
                // Carries session_key directly — routed by session, not run
                // (see the frame's own doc comment).
                GatewayEventFrame::SessionUpdated { session_key, .. }
                | GatewayEventFrame::SessionUserMessage { session_key, .. } => {
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
                // Owner-scoped with an admin arm (2026-08-08). No longer
                // role-gated upstream: a member must receive the card for their
                // OWN parked tool call, and the admin arm is what preserves the
                // operator-resolves-a-member's-approval workflow the previous
                // `Global` ruling was built around. A fleet approval (empty
                // `session_key`, raised by a cluster node over reverse RPC) has
                // no owner to compare against and stays operator-only.
                GatewayEventFrame::ApprovalRequested { session_key, .. }
                // A reminder is the same fact re-announced, so it is scoped by
                // the same key: it must not reach anyone the request itself was
                // withheld from.
                | GatewayEventFrame::ApprovalReminder { session_key, .. }
                | GatewayEventFrame::ApprovalResolved { session_key, .. }
                | GatewayEventFrame::ApprovalExpired { session_key, .. }
                // The banner leg joined the family once it started carrying the
                // session key it is derived from.
                | GatewayEventFrame::SurfaceApproval { session_key, .. } => {
                    if session_key.is_empty() {
                        SessionIdentity::OperatorOnly
                    } else {
                        SessionIdentity::BySessionKeyOrAdmin(session_key.clone())
                    }
                }
                GatewayEventFrame::SessionLifecycleChanged { session_key, .. } => {
                    SessionIdentity::BySessionKey(session_key.clone())
                }
                // Admin-gated family: the ids are what `method_admin.rs`
                // withholds from a member, so the event plane must not
                // volunteer them. See the frame's own doc.
                GatewayEventFrame::WorkspaceChanged { .. } => SessionIdentity::OperatorOnly,
                GatewayEventFrame::AcpSessionsChanged
                | GatewayEventFrame::TokenRotated
                | GatewayEventFrame::DeviceRevoked { .. }
                | GatewayEventFrame::CronJobChanged { .. }
                | GatewayEventFrame::HeartbeatTaskChanged { .. }
                | GatewayEventFrame::TeamChanged { .. }
                | GatewayEventFrame::SurfaceNotify { .. } => SessionIdentity::Global,
                // Self-reported canvas attribution; the admit arm delegates
                // to `visibility::canvas_visible_to` (owner OR roster member).
                GatewayEventFrame::CanvasUpdated {
                    owner_user_id,
                    project_id,
                    ..
                } => SessionIdentity::ByCanvasScope {
                    owner: owner_user_id.clone(),
                    project: project_id.clone(),
                },
                // Roster-scoped; see the frame's own doc and
                // `SessionIdentity::ByProjectScope`'s.
                GatewayEventFrame::ProjectsChanged {
                    project_id,
                    affected_user,
                    ..
                } => SessionIdentity::ByProjectScope {
                    project_id: project_id.clone(),
                    affected_user: affected_user.clone(),
                },
            }
        }

        let samples: Vec<GatewayEventFrame> = vec![
            GatewayEventFrame::RunAccepted {
                run_id: "r1".into(),
                session_key: "agent:main:main".into(),
                accepted_at: "t".into(),
            },
            GatewayEventFrame::RunQueued {
                run_id: "r1".into(),
                session_key: "agent:main:main".into(),
                ahead: 2,
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
                session_key: None,
            },
            GatewayEventFrame::AskUser {
                run_id: "r1".into(),
                seq: 1,
                session_key: "agent:main:main".into(),
                question: "q".into(),
                options: vec![],
                questions: vec![],
                answered: 0,
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
            GatewayEventFrame::SessionUserMessage {
                session_key: "agent:main:main".into(),
                author_user_id: "u-alice".into(),
                content: "hi".into(),
                timestamp: "2026-08-10T00:00:00Z".into(),
                seq: 7,
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
            GatewayEventFrame::ApprovalReminder {
                approval_id: "a1".into(),
                session_key: "agent:main:main".into(),
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
            GatewayEventFrame::WorkspaceChanged {
                workspace_id: "crypto".into(),
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
                session_key: "agent:main:s1".into(),
                title: "t".into(),
                body: "b".into(),
            },
            GatewayEventFrame::CanvasUpdated {
                canvas_id: "cv-1".into(),
                revision: 2,
                ops: vec![],
                actor: Some("u-alice".into()),
                owner_user_id: Some("u-alice".into()),
                project_id: Some("p-room".into()),
            },
            // The unstamped shape too: absent optionals are SKIPPED on the
            // wire, so this pins that classification reads absence as `None`
            // rather than as a missing arm falling to `Global`.
            GatewayEventFrame::CanvasUpdated {
                canvas_id: "cv-2".into(),
                revision: 1,
                ops: vec![],
                actor: None,
                owner_user_id: None,
                project_id: None,
            },
            GatewayEventFrame::ProjectsChanged {
                project_id: "p-room".into(),
                change: ChangeKind::Updated,
                affected_user: None,
            },
            // The member-removal shape: `affected_user` is skipped on the
            // wire when absent, so this pins that the removed-user carve-out
            // survives the same round trip the unstamped canvas sample
            // above pins for its own optionals.
            GatewayEventFrame::ProjectsChanged {
                project_id: "p-room".into(),
                change: ChangeKind::Updated,
                affected_user: Some("u-mallory".into()),
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

    /// The canvas frame's full delivery chain (`event_admits_for`, not the
    /// classification alone): the owner reads through ownership, a roster
    /// member of the linked room reads through the roster, and everyone
    /// else — including an operator-ROLE caller whose user id matches
    /// neither arm — is refused, because the arm delegates to the same
    /// `canvas_visible_to` the RPC face resolves and that predicate has no
    /// admin arm (P1: the answer to "may this person read this person's
    /// work" is not "admins may read everything").
    #[tokio::test]
    async fn canvas_updated_admits_owner_and_roster_member_and_refuses_stranger() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::projects::roster::publish(crate::projects::roster::RosterSnapshot::from_pairs([(
            "p-board-room".to_string(),
            "u-bob".to_string(),
        )]));

        let (store, _temp) = test_store();
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        let frame = GatewayEventFrame::CanvasUpdated {
            canvas_id: "cv-adm".into(),
            revision: 2,
            ops: vec![],
            actor: Some("u-alice".into()),
            owner_user_id: Some("u-alice".into()),
            project_id: Some("p-board-room".into()),
        };
        let topic = frame.topic_name();
        let data = serde_json::to_value(&frame).unwrap();

        for (caller, role, admitted, why) in [
            ("u-alice", "member", true, "the owner"),
            (
                "u-bob",
                "member",
                true,
                "a roster member of the linked room",
            ),
            ("u-carol", "member", false, "a stranger"),
            (
                "u-carol",
                "operator",
                false,
                "an operator who is neither owner nor member",
            ),
        ] {
            assert_eq!(
                index
                    .event_admits_for(&topic, Some(&data), Some(caller), Some(role), &store, None)
                    .await,
                admitted,
                "{why} ({caller}/{role})"
            );
        }

        // Without the project link the roster arm is gone: the member who
        // read through it above is refused like any stranger.
        let unlinked = GatewayEventFrame::CanvasUpdated {
            canvas_id: "cv-adm2".into(),
            revision: 2,
            ops: vec![],
            actor: None,
            owner_user_id: Some("u-alice".into()),
            project_id: None,
        };
        let data = serde_json::to_value(&unlinked).unwrap();
        assert!(
            !index
                .event_admits_for(
                    &topic,
                    Some(&data),
                    Some("u-bob"),
                    Some("member"),
                    &store,
                    None
                )
                .await,
            "no project link ⇒ no roster arm"
        );
    }

    /// `GatewayEventFrame::ProjectsChanged`'s full delivery chain: a roster
    /// member is admitted, a stranger is refused, an operator who is on
    /// neither the roster nor named as `affected_user` is refused too (no
    /// admin carve-out — mirrors the canvas test above and `SECURITY.md`'s
    /// P2 ruling "Visibility is the roster, full stop"), and the ONE user
    /// named in `affected_user` is admitted despite not being on the roster
    /// — the member-removal frame reaching the person it just removed.
    #[tokio::test]
    async fn projects_changed_admits_roster_member_and_removed_user_and_refuses_stranger() {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::projects::roster::publish(crate::projects::roster::RosterSnapshot::from_pairs([(
            "p-room".to_string(),
            "u-alice".to_string(),
        )]));

        let (store, _temp) = test_store();
        let store: Arc<dyn SessionStore> = Arc::new(store);
        let index = EventVisibilityIndex::new();

        // The ordinary case (rename/archive/bind_workspace/…): no
        // `affected_user`.
        let frame = GatewayEventFrame::ProjectsChanged {
            project_id: "p-room".into(),
            change: ChangeKind::Updated,
            affected_user: None,
        };
        let topic = frame.topic_name();
        let data = serde_json::to_value(&frame).unwrap();

        for (caller, role, admitted, why) in [
            ("u-alice", "member", true, "a roster member"),
            ("u-mallory", "member", false, "a stranger"),
            (
                "u-mallory",
                "operator",
                false,
                "an operator who is neither on the roster nor the affected user \
                 — there is no admin carve-out for a room's visibility",
            ),
        ] {
            assert_eq!(
                index
                    .event_admits_for(&topic, Some(&data), Some(caller), Some(role), &store, None)
                    .await,
                admitted,
                "{why} ({caller}/{role})"
            );
        }

        // The member-removal case: `u-mallory` was just dropped from the
        // roster (not published above, so the roster arm already refuses
        // them) and is named as `affected_user`. They must still receive
        // THIS frame so their own client learns to drop the room.
        let removal = GatewayEventFrame::ProjectsChanged {
            project_id: "p-room".into(),
            change: ChangeKind::Updated,
            affected_user: Some("u-mallory".into()),
        };
        let data = serde_json::to_value(&removal).unwrap();
        assert!(
            index
                .event_admits_for(
                    &topic,
                    Some(&data),
                    Some("u-mallory"),
                    Some("member"),
                    &store,
                    None
                )
                .await,
            "the removed member reads their own removal frame"
        );
        // A THIRD party who is neither on the roster nor the one named
        // `affected_user` must not ride along on the removal frame.
        assert!(
            !index
                .event_admits_for(
                    &topic,
                    Some(&data),
                    Some("u-carol"),
                    Some("member"),
                    &store,
                    None
                )
                .await,
            "a bystander does not inherit the removed member's carve-out"
        );
    }

    /// The 2026-08-09 real-machine QA's F1, as a regression: `run_complete`
    /// and `run_error` never reached ANY client, because `note_frame` evicted
    /// the run→session seed and the delivery loop then asked
    /// `event_admits_for` — which resolves those two topics THROUGH that seed.
    ///
    /// The calls below are in the production order (`handler.rs`'s delivery
    /// loop: note, then filter). Re-adding the eviction arm turns this red.
    #[tokio::test]
    async fn terminal_frames_survive_their_own_note_frame() {
        let (store, _temp) = test_store();
        let key = SessionKey::main("conv-terminal");
        stamp_owner(&store, &key, "alice").await;
        let store: Arc<dyn SessionStore> = Arc::new(store);

        let index = EventVisibilityIndex::new();
        index
            .note_frame(
                "stream.run_accepted",
                Some(&serde_json::json!({
                    "run_id": "r-term",
                    "session_key": key.to_key_string(),
                    "accepted_at": "t",
                })),
            )
            .await;

        for topic in ["stream.run_complete", "stream.run_error"] {
            let frame = serde_json::json!({
                "run_id": "r-term",
                "seq": 2,
                "summary": {},
                "total_duration_ms": 1,
                "error": "boom",
            });
            index.note_frame(topic, Some(&frame)).await;
            assert!(
                index
                    .event_admits(topic, Some(&frame), Some("alice"), false, &store, None)
                    .await,
                "{topic} must still reach the run's owner after note_frame ran"
            );
            assert!(
                !index
                    .event_admits(topic, Some(&frame), Some("bob"), false, &store, None)
                    .await,
                "{topic} must not reach a stranger"
            );
        }
    }

    #[tokio::test]
    async fn the_run_index_stays_capacity_bounded() {
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
