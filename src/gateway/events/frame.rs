//! Typed Gateway Event Frame
//!
//! Replaces `broadcast::Sender<String>` in `GatewayEventBus` with a typed enum.
//! Subscribers receive `GatewayEventFrame` directly — no String deserialization.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::event_emitter::{RunSummary, StreamEvent, ToolResult};
use crate::gateway::{ChannelId, ChannelStatus};

/// Typed event frame for the gateway event bus.
///
/// This enum replaces `TopicEvent { topic: String, data: Value }` with
/// compile-time type safety. Each variant carries its own typed payload.
///
/// Serializes with `#[serde(tag = "type")]` to produce the same JSON-RPC
/// wire format that WebSocket clients expect:
/// `{"type": "AgentRunStarted", "run_id": "...", ...}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayEventFrame {
    RunAccepted {
        run_id: String,
        session_key: String,
        accepted_at: String,
    },
    RunQueued {
        run_id: String,
        session_key: String,
        ahead: u16,
    },
    Reasoning {
        run_id: String,
        seq: u64,
        content: String,
        is_complete: bool,
    },
    ToolStart {
        run_id: String,
        seq: u64,
        tool_name: String,
        tool_id: String,
        params: Value,
    },
    ToolUpdate {
        run_id: String,
        seq: u64,
        tool_id: String,
        progress: String,
    },
    ToolEnd {
        run_id: String,
        seq: u64,
        tool_id: String,
        result: ToolResult,
        duration_ms: u64,
    },
    AgentTrace {
        run_id: String,
        seq: u64,
        event: aleph_protocol::AgentTraceEvent,
    },
    ResponseChunk {
        run_id: String,
        seq: u64,
        delta: String,
        full_text: String,
        /// Wire-only back-compat alias mirroring `delta`. Retained because
        /// `aleph_protocol::StreamEvent::ResponseChunk` (TUI/CLI) deserializes the
        /// wire JSON and requires `content` (no `delta`/`full_text`, no serde
        /// default). The internal `StreamEvent` dropped this field; here it is
        /// re-derived from `delta`.
        content: String,
        chunk_index: u32,
        is_final: bool,
        #[serde(default)]
        is_intermediate: bool,
    },
    /// Mid-run context-window occupancy after one LLM call (see
    /// `StreamEvent::ContextGauge`).
    ContextGauge {
        run_id: String,
        seq: u64,
        context_tokens: u32,
        context_window: u32,
        total_tokens: u64,
    },
    RunComplete {
        run_id: String,
        seq: u64,
        summary: RunSummary,
        total_duration_ms: u64,
    },
    RunError {
        run_id: String,
        seq: u64,
        error: String,
        error_code: Option<String>,
        /// Present only when the run may not be resolvable by id — see
        /// `event_emitter::types::StreamEvent::RunError::session_key`. Read by
        /// `EventVisibilityIndex::note_frame`, which seeds the run→session
        /// index from any frame naming both.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_key: Option<String>,
    },
    /// The agent is parked on a human. Built in exactly one place —
    /// [`crate::clarification::session::ask_user_frame`] — so the two
    /// projections below cannot drift apart.
    AskUser {
        run_id: String,
        seq: u64,
        /// Clarification registry key — the value a client must pass back to
        /// `clarification.resolve` to unblock the parked `ask_user` tool. The
        /// frame carries it because the reply is routed by session, not by run.
        session_key: String,
        /// Legacy single-question projection: the prompt of the question **at
        /// the cursor** (not `questions[0]`). A client that understands only
        /// this and `options` still drives a multi-question request to
        /// completion one reply at a time — that is the plain-text fallback,
        /// and it is why this field is a projection rather than a duplicate.
        question: String,
        /// Choice labels for that same question.
        options: Vec<String>,
        /// Full-fidelity view of every question in the request, including the
        /// per-option `description` the flat `options` array cannot carry.
        /// Empty only on a frame minted before this field existed.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        questions: Vec<crate::clarification::ClarificationQuestionView>,
        /// How many of `questions` already have answers — where a client that
        /// renders the full set resumes.
        #[serde(default)]
        answered: usize,
    },
    /// Terminal twin of [`Self::AskUser`]: the question on `session_key` is
    /// over and nothing is parked on an answer any more.
    ///
    /// Without it every consumer has to guess when a question ended — the card
    /// outlives the question, a second Panel window keeps hijacking Enter, and
    /// a reply typed after the timeout is swallowed. Keyed by session (not run)
    /// for the same reason `AskUser` is: the clarification registry is
    /// per-session and the answer is routed by session.
    ClarificationEnded {
        session_key: String,
        outcome: ClarificationOutcome,
    },
    ReasoningBlock {
        run_id: String,
        seq: u64,
        step_type: crate::gateway::event_emitter::ReasoningStepType,
        label: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        confidence: Option<crate::gateway::event_emitter::ConfidenceLevel>,
        is_final: bool,
    },
    UncertaintySignal {
        run_id: String,
        seq: u64,
        uncertainty: String,
        suggested_action: crate::gateway::event_emitter::UncertaintyAction,
    },
    ModelResolved {
        run_id: String,
        model_info: crate::providers::health::ModelInfo,
    },
    RunRetrying {
        run_id: String,
        seq: u64,
        provider: String,
        attempt: u32,
        max_attempts: u32,
        reason: String,
    },
    SessionUpdated {
        session_key: String,
        /// Channel that triggered the update (`metadata["channel_id"]` of the
        /// run), e.g. "telegram". `Some("gui:chat")` for Panel-originated runs
        /// — every Panel connection hardcodes that literal (`api/chat.rs`), so
        /// this names the surface CLASS and can never name a connection.
        /// `None` only for topic/title/state edits, which no run caused
        /// (`SessionManager::emit_session_updated`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_channel: Option<String>,
        /// The run that caused this update. `None` exactly where
        /// `origin_channel` is `None` — same producer, same reason.
        ///
        /// This is the identity `origin_channel` structurally cannot be. A
        /// client holding the run id its own `chat.send` returned knows the
        /// update is its own and must NOT re-hydrate over a transcript it
        /// already streamed; every other client — a second tab of the same
        /// user, a second member of a project room — sees a run it did not
        /// start and re-hydrates. Buys no new exposure: this frame is
        /// classified `BySessionKey`, the same audience `RunAccepted` already
        /// hands the identical run id to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_run_id: Option<String>,
    },
    /// A human's message became a transcript row — the live half of what
    /// `chat.history` replays.
    ///
    /// # Why this is its own frame and not a field on `RunAccepted`
    ///
    /// `RunAccepted` is the only other frame carrying `{session_key}` at turn
    /// start, and it is emitted BEFORE the harness seeds
    /// `SessionEvent::UserMessage`. Two consequences, both of which make it the
    /// wrong carrier: it fires for turns that never persist a user row at all
    /// (a fast-path slash command — `fast_path.rs` re-emits the event by hand
    /// precisely "so the projector writes them to messages"), and the text it
    /// would carry is `request.input`, which is not always what lands in the
    /// transcript (`/moa X` is rewritten before seeding; multimodal turns take
    /// another path entirely). A bubble the next reload contradicts is worse
    /// than a bubble that arrives late, so this frame is published from the
    /// projector at the instant the row exists instead.
    ///
    /// # Why `author_user_id` is not an `Option`
    ///
    /// It is the delivery contract, not a convenience. A viewer decides
    /// whether to render this by asking "is the author someone other than
    /// me?", and an unattributed message cannot answer that — rendering it
    /// would duplicate the sender's own optimistic bubble. Producers therefore
    /// skip the frame entirely rather than send an absent author, which is
    /// also why single-author sessions (`ambient_room_author` is `None`
    /// outside a project room) cost zero bytes on this topic.
    SessionUserMessage {
        session_key: String,
        author_user_id: String,
        content: String,
        /// RFC3339, taken from the SAME `MessageRecord` the history handler
        /// will later serve. Derived rather than re-formatted because
        /// `MessageRecord::rfc3339` owns the seconds-vs-milliseconds ambiguity
        /// of the stored stamp (`session_store::types`); formatting the raw
        /// value here would re-open it on exactly one of the two backends.
        timestamp: String,
        /// Source event seq — the row's identity (`session::projection::row_id`).
        /// Carried so a client can order this against a concurrently-arriving
        /// rehydrate without comparing text.
        seq: u64,
    },
    /// Authoritative running-session set changed (a run was claimed or
    /// released). `seq` is a monotonic version stamped by the
    /// `SessionRunRegistry` under its map lock; consumers keep the highest seq
    /// and ignore any older-or-equal frame so a reordered delivery self-heals.
    /// `running` is the full set of backend session keys with an in-flight run
    /// — the sidebar red-dot reads this directly (server-authoritative, no
    /// client refcount). **As PUBLISHED that set spans every user; what a
    /// connection receives is narrowed to the sessions that connection may see**
    /// (`event_visibility::EventVisibilityIndex::project_for` rewrites the array
    /// per socket, and still sends the frame when the result is empty). So a
    /// client's `running` is already its own, and `seq` is unchanged by the
    /// rewrite. Payload is intentionally the registry's own data only
    /// (no `ConcurrencySnapshot`) so the registry can emit it without reaching
    /// the limiter; gauge consumers re-fetch `run_concurrency` on receipt.
    RunningSetChanged {
        seq: u64,
        running: Vec<String>,
    },
    ChannelMessage {
        channel_id: ChannelId,
        conversation_id: crate::gateway::channel::ConversationId,
        message: InboundMessagePayload,
    },
    ChannelTyping {
        channel_id: ChannelId,
        conversation_id: crate::gateway::channel::ConversationId,
    },
    ChannelStatusChanged {
        channel_id: ChannelId,
        status: ChannelStatus,
    },
    ChannelError {
        channel_id: ChannelId,
        error: String,
    },
    ConfigChanged {
        section: Option<String>,
        value: Value,
    },
    PairingRequested {
        device_name: String,
    },
    PairingCompleted {
        device_id: String,
    },
    ApprovalRequested {
        approval_id: String,
        session_key: String,
        channel_id: String,
        conversation_id: String,
        /// Harness tool-call id (`ToolStart.tool_id`) this approval gates, when
        /// the approval was raised from the tool-dispatch chokepoint. Clients
        /// key `tool_id → approval` on it; pairing an approval to a tool row by
        /// position is wrong the moment two tool calls run concurrently, since
        /// `exec.approvals.pending` is an unordered map. `None` for approvals
        /// with no owning tool call (cluster node approvals, raw exec commands).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
    },
    ApprovalResolved {
        approval_id: String,
        session_key: String,
        decision: ApprovalDecisionType,
        resolved_by: Option<String>,
    },
    ApprovalExpired {
        approval_id: String,
        session_key: String,
    },
    SessionLifecycleChanged {
        session_key: String,
        old_state: Option<String>,
        new_state: String,
        reason: Option<String>,
    },
    /// Fired whenever the ACP session pool changes (created / removed /
    /// respawned). Payload-free on purpose: panels re-fetch via
    /// `acp.sessions.list` so the truth source stays single.
    AcpSessionsChanged,
    /// Emitted when the shared Gateway token is rotated. The connection loop
    /// intercepts it to close *remote* (token-authorized) sessions with
    /// 4001/`token_rotated`; loopback sessions ignore it. Payload-free.
    TokenRotated,
    /// Emitted when one paired Panel device is revoked (`gateway.devices.revoke`).
    /// The connection loop intercepts it and closes only the sessions bound to
    /// that `device_id` (4001/`device_revoked`); every other session — loopback,
    /// legacy shared-token, other devices — is unaffected. This is the per-device
    /// counterpart to [`Self::TokenRotated`]'s kick-everyone hammer; without it a
    /// revoked device kept operator authority on its open socket until it happened
    /// to reconnect. Never forwarded to clients (it names a device id and no
    /// client renders it).
    DeviceRevoked {
        device_id: String,
    },
    /// Emitted whenever a cron job is mutated server-side (created / updated
    /// / deleted / enabled / disabled / forced-run / state-changed by a
    /// scheduler tick). The panel subscribes to `cron.job.changed` so it can
    /// drop polling. Payload is intentionally minimal — clients fetch the
    /// full job via `cron.get` if they need fresh data.
    CronJobChanged {
        job_id: String,
        change: ChangeKind,
    },
    /// Heartbeat-task analogue of `CronJobChanged`. Topic: `heartbeat.task.changed`.
    HeartbeatTaskChanged {
        task_id: String,
        change: ChangeKind,
    },
    /// Emitted when a team's sidebar-visible metadata changes (LLM
    /// auto-name on first message, manual rename, or disband). Topic:
    /// `team.changed`. Mirrors `CronJobChanged` — payload-minimal; the
    /// Panel re-fetches `agents.teams` to refresh the group-chat list.
    TeamChanged {
        team_id: String,
        change: ChangeKind,
    },
    /// Emitted whenever a workspace row is mutated (created / renamed / archived
    /// / restored). Topic: `workspace.changed`. Mirrors `CronJobChanged` —
    /// payload-minimal; clients re-fetch `workspace.list`.
    ///
    /// Published from [`crate::gateway::agent_env::AgentEnvStore`] rather than
    /// from the RPC handlers, so the CLI (which reaches the same handlers over
    /// IPC) and any future in-process mutator emit it too, and so the four
    /// handler signatures — and the ~15 test call sites that build them — stay
    /// as they are.
    ///
    /// `change` is a rendering hint, not a lifecycle claim. **Archive and
    /// restore both report `Updated`**: archiving is reversible and keeps the id
    /// taken, so `Deleted` would be a statement a consumer could act on and be
    /// wrong about. What actually left is the row's place in the default list,
    /// and re-fetching is what tells a client that.
    ///
    /// **One `workspace.create` is two frames — `Created` then `Updated`.**
    /// Emitting from the store means the count follows the store writes, and
    /// `handle_create` is two of them (the INSERT, then the name/icon write
    /// `AgentEnvStore::create` cannot take). Observed on a real machine
    /// 2026-08-09 as two byte-identical `workspace.list` re-fetches in the same
    /// millisecond. Harmless for a client that re-fetches — that is idempotent
    /// — but a client that *renders* `change` will be told "created" and then
    /// immediately "updated" for a single user action. Coalescing was rejected:
    /// it would couple the store's emission to a handler's write count, which
    /// is exactly the knowledge the store-level emission exists to not have.
    /// Pinned by `handlers::workspace::tests::
    /// create_reaches_the_wire_as_created_then_updated`.
    ///
    /// Classified `OperatorOnly` in `event_visibility::session_identity_of`,
    /// not `Global`: the whole `workspace.` family is admin-gated in
    /// `method_admin.rs` precisely so a member cannot enumerate workspaces, and
    /// broadcasting the ids would hand back what that gate withholds. This is
    /// `OperatorOnly`'s documented meaning rather than a convenience — a
    /// workspace has no owner column *by decision* (see `method_admin.rs`'s
    /// "NOT an owner column" note), so there is no ownership to resolve.
    WorkspaceChanged {
        workspace_id: String,
        change: ChangeKind,
    },
    /// Emitted whenever a project room's roster-visible state changes
    /// (create / add / create_blank / rename / archive / bind_workspace /
    /// remove / member add / member remove). Topic: `projects.changed`.
    /// Mirrors `TeamChanged` — payload-minimal; the Panel re-fetches
    /// `projects.list` (sidebar) and `projects.get` (an open room's page)
    /// to refresh.
    ///
    /// Classified `ByProjectScope` in `event_visibility::session_identity_of`,
    /// NOT `Global`: unlike `team.changed` (whose payload is inert — a team
    /// id nobody outside its roster can act on), this frame's own existence
    /// says "a room named `project_id` exists", and P2's whole predicate is
    /// that membership decides visibility (`SECURITY.md`'s project-rooms
    /// section: "Visibility is the roster, full stop"). A member-add or
    /// member-remove frame doubles as roster content, so a stranger
    /// receiving it would learn who is on a roster they cannot see.
    ///
    /// `Created` is used only where the store guarantees a genuinely NEW row
    /// (`create`, `create_blank`); `add` can instead collapse onto an
    /// existing row for the same path (`ProjectStore::add_for`'s "collapses
    /// onto their existing row" doc), so it reports `Updated` rather than
    /// overclaiming — the same reasoning `WorkspaceChanged`'s doc applies to
    /// archive/restore.
    ProjectsChanged {
        project_id: String,
        change: ChangeKind,
        /// Set ONLY for a member-removal mutation, naming the user who was
        /// just dropped from the roster. By the time this fires the roster
        /// projection no longer admits them
        /// (`ProjectStore::republish_roster_locked` runs inside the same
        /// write lock as the delete), so without this the roster-scoped
        /// classification below would never deliver them the one frame that
        /// tells their own client to drop the room from its list. Every
        /// other verb leaves this `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        affected_user: Option<String>,
    },
    /// Core-decided R5 interrupt addressed to one or more delivery surfaces.
    /// Unlike the raw agent-lifecycle frames, the "is this worth interrupting
    /// the user" policy has already been applied by the core R5 router; the
    /// shell only focus-gates and renders. `audience` lists the `SurfaceKind`
    /// wire strings (e.g. `["desktop"]`) the gateway forward-filter routes to.
    SurfaceNotify {
        audience: Vec<String>,
        title: String,
        body: String,
        /// Originating topic (e.g. `agent.run.complete`) — diagnostics only.
        source_topic: String,
    },
    /// Core-decided approval banner addressed to one or more delivery surfaces.
    /// The raw `approval.requested` frame drives the Panel card; this is the
    /// *banner* leg, routed by the R5 router so the shell renders it through the
    /// same unified path as `SurfaceNotify`. The payload is otherwise
    /// intentionally sparse — approval detail lives in the Panel card (via
    /// `exec.approvals.pending`); the banner only needs to get attention.
    ///
    /// `session_key` mirrors `ApprovalRequested`'s (empty for a cluster-node
    /// approval) and exists purely so `event_visibility` can scope the banner
    /// the same way as its three siblings. Before it existed the frame had
    /// nothing to be scoped by, so it was `Global` plus a role rule — which
    /// meant the person whose own tool call was parked got the card but never
    /// the banner, exactly the audience the R5 interrupt is for.
    SurfaceApproval {
        audience: Vec<String>,
        approval_id: String,
        session_key: String,
        title: String,
        body: String,
    },
    /// A committed whiteboard apply (`CanvasStore::apply`), published from
    /// INSIDE the per-canvas critical section so frame order can never
    /// diverge from commit order (see `canvas::doc_io`'s module doc).
    ///
    /// The Panel parses the payload as `aleph_protocol::canvas::CanvasUpdated`
    /// (`canvas_id`/`revision`/`ops`/`actor`); the two extra fields exist for
    /// the delivery plane alone. The frame SELF-REPORTS its attribution
    /// (§4.8 mine H: a resolution handle must not be installed under narrower
    /// conditions than the frame is produced under — there is no run or
    /// session to seed an index from, so the frame carries the answer), and
    /// `event_visibility` classifies it `ByCanvasScope`, whose arm only
    /// delegates to `gateway::visibility::canvas_visible_to` — the same
    /// predicate the RPC face and the tool face resolve.
    ///
    /// Deliberately NO `stream_method()` arm: this is a TopicEvent-form frame
    /// (`{"topic":"canvas.updated","data":…}`); terminal clients have no
    /// surface for a live board and `frame_census` would otherwise demand a
    /// `StreamEvent` twin nothing decodes.
    CanvasUpdated {
        canvas_id: String,
        revision: u64,
        ops: Vec<aleph_protocol::canvas::CanvasOp>,
        /// Who applied the batch (user id or agent label); `None` for
        /// anonymous local sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
        /// Stamped owner of the canvas; absent reads as the legacy operator,
        /// the same `owner_or_legacy` convention the predicate applies.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_user_id: Option<String>,
        /// Project (room) link, when the canvas is roster-shared.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
    },
}

/// How a clarification ended, carried on [`GatewayEventFrame::ClarificationEnded`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClarificationOutcome {
    /// The user answered; the parked `ask_user` was unblocked with the reply.
    Resolved,
    /// Nobody can answer any more (run aborted, no transport, superseded).
    Cancelled,
    /// The question outlived its timeout without an answer.
    Expired,
}

/// Tagging value for the panel so it can pick the right local action
/// (re-fetch list vs drop row) without re-issuing a list query.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
    /// Runtime-state-only change (e.g. `last_run_at_ms`, `consecutive_errors`).
    /// Cheaper than `Updated` so clients can skip re-rendering the whole form
    /// while still refreshing the timeline indicators.
    StateChanged,
}

impl From<StreamEvent> for GatewayEventFrame {
    fn from(event: StreamEvent) -> Self {
        match event {
            StreamEvent::RunAccepted {
                run_id,
                session_key,
                accepted_at,
            } => Self::RunAccepted {
                run_id,
                session_key,
                accepted_at,
            },
            StreamEvent::RunQueued {
                run_id,
                session_key,
                ahead,
            } => Self::RunQueued {
                run_id,
                session_key,
                ahead,
            },
            StreamEvent::Reasoning {
                run_id,
                seq,
                content,
                is_complete,
            } => Self::Reasoning {
                run_id,
                seq,
                content,
                is_complete,
            },
            StreamEvent::ToolStart {
                run_id,
                seq,
                tool_name,
                tool_id,
                params,
            } => Self::ToolStart {
                run_id,
                seq,
                tool_name,
                tool_id,
                params,
            },
            StreamEvent::ToolUpdate {
                run_id,
                seq,
                tool_id,
                progress,
            } => Self::ToolUpdate {
                run_id,
                seq,
                tool_id,
                progress,
            },
            StreamEvent::ToolEnd {
                run_id,
                seq,
                tool_id,
                result,
                duration_ms,
            } => Self::ToolEnd {
                run_id,
                seq,
                tool_id,
                result,
                duration_ms,
            },
            StreamEvent::AgentTrace { run_id, seq, event } => {
                Self::AgentTrace { run_id, seq, event }
            }
            StreamEvent::ResponseChunk {
                run_id,
                seq,
                delta,
                full_text,
                chunk_index,
                is_final,
                is_intermediate,
            } => Self::ResponseChunk {
                run_id,
                seq,
                content: delta.clone(), // wire alias (see field doc); must precede `delta`
                delta,
                full_text,
                chunk_index,
                is_final,
                is_intermediate,
            },
            StreamEvent::ContextGauge {
                run_id,
                seq,
                context_tokens,
                context_window,
                total_tokens,
            } => Self::ContextGauge {
                run_id,
                seq,
                context_tokens,
                context_window,
                total_tokens,
            },
            StreamEvent::RunComplete {
                run_id,
                seq,
                mut summary,
                total_duration_ms,
            } => {
                // Sanitize ONCE at the wire boundary (§4.7 invariant: any path
                // delivering `final_response` to a surface must pass the single
                // sanitize atom). The source is RAW — `runner_impl` copies
                // `content.text` verbatim and even falls back to
                // `content.thinking` — so without this, every WS consumer
                // (Panel `finalize_answer` bubble overwrite + voice TTS, CLI
                // no-stream fallback and `--output-last-message`) re-leaked
                // `<think>` reasoning that the live-chunk scrubber had already
                // stripped. Pure-thinking summaries sanitize to `None`, so a
                // client keeps its (scrubbed) streamed text instead.
                summary.final_response = summary
                    .final_response
                    .as_deref()
                    .and_then(crate::gateway::reply_emitter::sanitize_final_response);
                Self::RunComplete {
                    run_id,
                    seq,
                    summary,
                    total_duration_ms,
                }
            }
            StreamEvent::RunError {
                run_id,
                seq,
                error,
                error_code,
                session_key,
            } => Self::RunError {
                run_id,
                seq,
                error,
                error_code,
                session_key,
            },
            StreamEvent::AskUser {
                run_id,
                seq,
                session_key,
                question,
                options,
                questions,
                answered,
            } => Self::AskUser {
                run_id,
                seq,
                session_key,
                question,
                options,
                questions,
                answered,
            },
            StreamEvent::ReasoningBlock {
                run_id,
                seq,
                step_type,
                label,
                content,
                confidence,
                is_final,
            } => Self::ReasoningBlock {
                run_id,
                seq,
                step_type,
                label,
                content,
                confidence,
                is_final,
            },
            StreamEvent::UncertaintySignal {
                run_id,
                seq,
                uncertainty,
                suggested_action,
            } => Self::UncertaintySignal {
                run_id,
                seq,
                uncertainty,
                suggested_action,
            },
            StreamEvent::ModelResolved { run_id, model_info } => {
                Self::ModelResolved { run_id, model_info }
            }
            StreamEvent::RunRetrying {
                run_id,
                seq,
                provider,
                attempt,
                max_attempts,
                reason,
            } => Self::RunRetrying {
                run_id,
                seq,
                provider,
                attempt,
                max_attempts,
                reason,
            },
        }
    }
}

impl GatewayEventFrame {
    #[must_use]
    pub fn topic_name(&self) -> String {
        match self {
            Self::RunAccepted { .. } => "run.accepted",
            Self::RunQueued { .. } => "run.queued",
            Self::Reasoning { .. } => "agent.reasoning",
            Self::ToolStart { .. } => "agent.tool.start",
            Self::ToolUpdate { .. } => "agent.tool.update",
            Self::ToolEnd { .. } => "agent.tool.end",
            Self::AgentTrace { .. } => "agent.trace",
            Self::ResponseChunk { .. } => "agent.response.chunk",
            Self::ContextGauge { .. } => "agent.context.gauge",
            Self::RunComplete { .. } => "agent.run.complete",
            Self::RunError { .. } => "agent.run.error",
            Self::AskUser { .. } => "agent.ask.user",
            Self::ClarificationEnded { .. } => "agent.clarification.ended",
            Self::ReasoningBlock { .. } => "agent.reasoning.block",
            Self::UncertaintySignal { .. } => "agent.uncertainty",
            Self::ModelResolved { .. } => "agent.model.resolved",
            Self::RunRetrying { .. } => "agent.run.retrying",
            Self::SessionUpdated { .. } => "session.updated",
            Self::SessionUserMessage { .. } => "session.user.message",
            Self::RunningSetChanged { .. } => "running.set.changed",
            Self::ChannelMessage { .. } => "channel.message",
            Self::ChannelTyping { .. } => "channel.typing",
            Self::ChannelStatusChanged { .. } => "channel.status",
            Self::ChannelError { .. } => "channel.error",
            Self::ConfigChanged { .. } => "config.changed",
            Self::PairingRequested { .. } => "pairing.requested",
            Self::PairingCompleted { .. } => "pairing.completed",
            Self::ApprovalRequested { .. } => "approval.requested",
            Self::ApprovalResolved { .. } => "approval.resolved",
            Self::ApprovalExpired { .. } => "approval.expired",
            Self::SessionLifecycleChanged { .. } => "session.lifecycle.changed",
            Self::AcpSessionsChanged => "acp.sessions.changed",
            Self::TokenRotated => "gateway.token.rotated",
            Self::DeviceRevoked { .. } => "gateway.device.revoked",
            Self::CronJobChanged { .. } => "cron.job.changed",
            Self::HeartbeatTaskChanged { .. } => "heartbeat.task.changed",
            Self::TeamChanged { .. } => "team.changed",
            Self::WorkspaceChanged { .. } => "workspace.changed",
            Self::ProjectsChanged { .. } => "projects.changed",
            Self::SurfaceNotify { .. } => "surface.notify",
            Self::SurfaceApproval { .. } => "surface.approval",
            // The protocol const, not a second literal: the Panel subscribes
            // by this exact string and a drifted copy here would be silent.
            Self::CanvasUpdated { .. } => aleph_protocol::canvas::TOPIC,
        }
        .to_string()
    }

    /// Returns the `stream.*` method name for agent streaming events,
    /// or `None` for non-streaming events (config, channel, etc.).
    ///
    /// Streaming events are sent over WebSocket as JSON-RPC notifications:
    /// `{"method": "stream.<type>", "params": <frame_data>}`
    ///
    /// The frontend subscribes to `stream.*` and converts the method prefix
    /// from `stream.` to `run.` for internal event dispatch.
    #[must_use]
    pub const fn stream_method(&self) -> Option<&'static str> {
        match self {
            Self::RunAccepted { .. } => Some("stream.run_accepted"),
            Self::RunQueued { .. } => Some("stream.run_queued"),
            Self::Reasoning { .. } => Some("stream.reasoning"),
            Self::ToolStart { .. } => Some("stream.tool_start"),
            Self::ToolUpdate { .. } => Some("stream.tool_update"),
            Self::ToolEnd { .. } => Some("stream.tool_end"),
            Self::AgentTrace { .. } => Some("stream.agent_trace"),
            Self::ResponseChunk { .. } => Some("stream.response_chunk"),
            Self::ContextGauge { .. } => Some("stream.context_gauge"),
            Self::RunComplete { .. } => Some("stream.run_complete"),
            Self::RunError { .. } => Some("stream.run_error"),
            Self::AskUser { .. } => Some("stream.ask_user"),
            Self::ClarificationEnded { .. } => Some("stream.clarification_ended"),
            Self::ReasoningBlock { .. } => Some("stream.reasoning_block"),
            Self::UncertaintySignal { .. } => Some("stream.uncertainty_signal"),
            Self::ModelResolved { .. } => Some("stream.model_resolved"),
            Self::RunRetrying { .. } => Some("stream.run_retrying"),
            Self::SessionUpdated { .. } => Some("stream.session_updated"),
            Self::SessionUserMessage { .. } => Some("stream.session_user_message"),
            Self::RunningSetChanged { .. } => Some("stream.running_set_changed"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessagePayload {
    pub text: String,
    pub sender: MessageSender,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSender {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_rotated_serializes_with_snake_case_tag() {
        let f = GatewayEventFrame::TokenRotated;
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(json, r#"{"type":"token_rotated"}"#);
    }

    /// Panel contract for the mid-run gauge feed: `stream.context_gauge`
    /// method (the panel rewrites the prefix to `run.context_gauge`), tag
    /// `"type":"context_gauge"`, and the same field names as the
    /// `RunSummary` gauge fields so the panel projector applies unchanged.
    #[test]
    fn context_gauge_frame_wire_shape() {
        let f = GatewayEventFrame::ContextGauge {
            run_id: "r1".into(),
            seq: 7,
            context_tokens: 42_000,
            context_window: 200_000,
            total_tokens: 55_000,
        };
        assert_eq!(f.stream_method(), Some("stream.context_gauge"));
        assert_eq!(f.topic_name(), "agent.context.gauge");
        let v: serde_json::Value = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "context_gauge");
        assert_eq!(v["run_id"], "r1");
        assert_eq!(v["context_tokens"], 42_000);
        assert_eq!(v["context_window"], 200_000);
        assert_eq!(v["total_tokens"], 55_000);
    }

    /// Panel contract for the terminal clarification frame: it rides the same
    /// `stream.*` delivery as `stream.ask_user` (its opening twin) and is keyed
    /// by the session key the Panel answered on.
    #[test]
    fn clarification_ended_wire_shape() {
        let f = GatewayEventFrame::ClarificationEnded {
            session_key: "agent:main|conv-1".into(),
            outcome: ClarificationOutcome::Expired,
        };
        assert_eq!(f.stream_method(), Some("stream.clarification_ended"));
        assert_eq!(f.topic_name(), "agent.clarification.ended");
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "clarification_ended");
        assert_eq!(v["session_key"], "agent:main|conv-1");
        assert_eq!(v["outcome"], "expired");
    }

    /// The approval → tool-row pairing key. Absent when the approval has no
    /// owning tool call, so the field must not serialize as `null`.
    #[test]
    fn approval_requested_carries_tool_call_id() {
        let f = GatewayEventFrame::ApprovalRequested {
            approval_id: "a1".into(),
            session_key: "s1".into(),
            channel_id: String::new(),
            conversation_id: String::new(),
            tool_call_id: Some("toolu_01".into()),
        };
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["tool_call_id"], "toolu_01");

        let f = GatewayEventFrame::ApprovalRequested {
            approval_id: "a2".into(),
            session_key: "s1".into(),
            channel_id: String::new(),
            conversation_id: String::new(),
            tool_call_id: None,
        };
        let v = serde_json::to_value(&f).unwrap();
        assert!(v.get("tool_call_id").is_none());
    }

    #[test]
    fn running_set_changed_wire_shape() {
        let f = GatewayEventFrame::RunningSetChanged {
            seq: 42,
            running: vec!["agent:main|conv-1".into(), "agent:main|conv-2".into()],
        };
        // Streaming path so it rides the same stream.* delivery as session_updated.
        assert_eq!(f.stream_method(), Some("stream.running_set_changed"));
        assert_eq!(f.topic_name(), "running.set.changed");
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "running_set_changed");
        assert_eq!(v["seq"], 42);
        assert_eq!(v["running"][0], "agent:main|conv-1");
        assert_eq!(v["running"][1], "agent:main|conv-2");
    }

    /// A new variant that nobody gives a `stream_method` arm falls into the
    /// catch-all `_ => None` and is silently unroutable — it is built,
    /// converted, and then dropped before any client sees it. That failure is
    /// not a compile error, so it needs this.
    #[test]
    fn run_queued_has_a_wire_method_and_survives_conversion() {
        let frame = GatewayEventFrame::from(StreamEvent::RunQueued {
            run_id: "run-1".to_string(),
            session_key: "agent/main".to_string(),
            ahead: 2,
        });
        assert_eq!(frame.stream_method(), Some("stream.run_queued"));

        let GatewayEventFrame::RunQueued {
            run_id,
            session_key,
            ahead,
        } = frame
        else {
            panic!("conversion dropped the variant");
        };
        assert_eq!(run_id, "run-1");
        assert_eq!(ahead, 2);
        assert_eq!(
            session_key, "agent/main",
            "the frame must name its session: it is the FIRST frame of a run, \
             so nothing has seeded the run→session index yet"
        );
    }
}

#[cfg(test)]
mod surface_notify_tests {
    use super::*;

    #[test]
    fn surface_notify_topic_and_wire_shape() {
        let f = GatewayEventFrame::SurfaceNotify {
            audience: vec!["desktop".to_string()],
            title: "Aleph finished".to_string(),
            body: "Your turn is complete.".to_string(),
            source_topic: "agent.run.complete".to_string(),
        };
        // Non-streaming → TopicEvent wire shape (topic + data), no stream method.
        assert_eq!(f.topic_name(), "surface.notify");
        assert!(f.stream_method().is_none());

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "surface_notify");
        assert_eq!(v["audience"][0], "desktop");
        assert_eq!(v["title"], "Aleph finished");
        assert_eq!(v["source_topic"], "agent.run.complete");
    }

    #[test]
    fn run_retrying_topic_and_wire_shape() {
        let f = GatewayEventFrame::from(StreamEvent::RunRetrying {
            run_id: "r1".to_string(),
            seq: 7,
            provider: "kimi-for-coding".to_string(),
            attempt: 2,
            max_attempts: 3,
            reason: "Request timed out".to_string(),
        });
        // Streaming event → panel receives it as stream.run_retrying.
        assert_eq!(f.topic_name(), "agent.run.retrying");
        assert_eq!(f.stream_method(), Some("stream.run_retrying"));

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "run_retrying");
        assert_eq!(v["run_id"], "r1");
        assert_eq!(v["provider"], "kimi-for-coding");
        assert_eq!(v["attempt"], 2);
        assert_eq!(v["max_attempts"], 3);
        assert_eq!(v["reason"], "Request timed out");
    }

    #[test]
    fn surface_approval_topic_and_wire_shape() {
        let f = GatewayEventFrame::SurfaceApproval {
            audience: vec!["desktop".to_string()],
            approval_id: "a1".to_string(),
            session_key: "agent:main:s1".to_string(),
            title: "Aleph needs your approval".to_string(),
            body: "A tool call is waiting for you.".to_string(),
        };
        // Non-streaming → TopicEvent wire shape (topic + data), no stream method.
        assert_eq!(f.topic_name(), "surface.approval");
        assert!(f.stream_method().is_none());

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "surface_approval");
        assert_eq!(v["audience"][0], "desktop");
        assert_eq!(v["approval_id"], "a1");
        assert_eq!(v["title"], "Aleph needs your approval");
        assert_eq!(v["body"], "A tool call is waiting for you.");
    }

    #[test]
    fn team_changed_topic_and_wire_shape() {
        let f = GatewayEventFrame::TeamChanged {
            team_id: "t1".to_string(),
            change: ChangeKind::Updated,
        };
        // Non-streaming → TopicEvent wire shape (topic + data), no stream method.
        assert_eq!(f.topic_name(), "team.changed");
        assert!(f.stream_method().is_none());

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "team_changed");
        assert_eq!(v["team_id"], "t1");
        assert_eq!(v["change"], "updated");
    }

    /// The cross-crate half of the `canvas.updated` contract: the frame body
    /// must parse as the protocol payload the Panel decodes
    /// (`aleph_protocol::canvas::CanvasUpdated` tolerates the two extra
    /// delivery-plane fields), the topic is the protocol const (one source,
    /// not a second literal), and there is deliberately no `stream_method`
    /// arm — a TopicEvent-form frame, invisible to `frame_census`'s
    /// stream-twin requirement.
    #[test]
    fn canvas_updated_parses_as_the_protocol_payload_and_streams_nothing() {
        let f = GatewayEventFrame::CanvasUpdated {
            canvas_id: "cv-1".to_string(),
            revision: 3,
            ops: vec![aleph_protocol::canvas::CanvasOp::DeleteShape {
                id: "s1".to_string(),
            }],
            actor: Some("u-alice".to_string()),
            owner_user_id: Some("u-alice".to_string()),
            project_id: Some("p-room".to_string()),
        };
        assert_eq!(f.topic_name(), aleph_protocol::canvas::TOPIC);
        assert!(f.stream_method().is_none());

        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "canvas_updated");
        let payload: aleph_protocol::canvas::CanvasUpdated =
            serde_json::from_value(v).expect("the Panel-side payload type must parse the frame");
        assert_eq!(payload.canvas_id, "cv-1");
        assert_eq!(payload.revision, 3);
        assert_eq!(payload.ops.len(), 1);
        assert_eq!(payload.actor.as_deref(), Some("u-alice"));
    }
}
