//! Agent Handlers
//!
//! RPC handlers for agent operations: run, wait, cancel, status.

use crate::sync_primitives::Arc;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::agent_instance::AgentRegistry;
use super::super::event_bus::GatewayEventBus;
use super::super::event_emitter::{EventEmitter, GatewayEventEmitter, OutputMode, StreamEvent};
use super::super::execution_adapter::ExecutionAdapter;
use super::super::execution_engine::RunRequest;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::router::{AgentRouter, SessionKey};
use super::parse_params;
use crate::projects::binding::ClaimSource;

/// A file attachment sent with a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// File name
    pub name: String,
    /// MIME type (e.g., "image/png", "application/pdf")
    pub mime_type: String,
    /// Base64-encoded file content
    pub data: String,
}

/// Parameters for agent.run request
#[derive(Debug, Clone, Deserialize)]
pub struct AgentRunParams {
    /// User input message
    pub input: String,
    /// Optional session key (auto-generated if not provided)
    #[serde(default)]
    pub session_key: Option<String>,
    /// Channel identifier (e.g., "gui:window1", "cli:term1")
    #[serde(default)]
    pub channel: Option<String>,
    /// Peer identifier for per-peer sessions
    #[serde(default)]
    pub peer_id: Option<String>,
    /// Whether to stream events (default: true)
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// Thinking level for LLM reasoning depth
    ///
    /// Supports: "off", "minimal", "low", "medium", "high", "xhigh"
    /// Also supports aliases: "think", "ultrathink", "max", etc.
    /// Default is "minimal" if not specified.
    #[serde(default)]
    pub thinking: Option<String>,
    /// File attachments sent with the message
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Explicit target agent ID (bypasses channel binding resolution)
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Optional absolute project root. When set, the agent's tool calls
    /// run inside this directory instead of `~/.aleph/workspaces/{agent_id}`.
    /// See `gateway/handlers/chat.rs` for the user-facing flow.
    #[serde(default)]
    pub project_root: Option<String>,
    /// Per-turn model override forwarded from `chat.send`. Lights up the
    /// chat-window model picker — see
    /// [`crate::gateway::model_override::ModelOverride`].
    #[serde(default)]
    pub model_override: Option<crate::gateway::model_override::ModelOverride>,
    /// Execution tier chosen in the composer for a conversation whose session
    /// does not exist yet.
    ///
    /// The tier is normally read off the session's identity metadata, but a
    /// brand-new conversation HAS no session until this very message creates
    /// one — so a picker that only wrote to the session could not govern the
    /// first turn, which is the one the user was trying to protect. Riding on
    /// the request closes that window; the run stamps it onto the session, so
    /// later turns (which carry nothing) and a page reload both keep it.
    ///
    /// Same shape as `project_root`, which solves the identical
    /// chosen-before-the-session-exists problem. Client-supplied. An explicit
    /// tier REPLACES the global one and may raise it as well as lower it — an
    /// explicit decision by an authorized caller. Safe because the undisableable
    /// `[sandbox.command_policy]` floor holds under every tier, and `Full` from a
    /// chat-level channel is still clamped to `Auto` by
    /// [`crate::gateway::channel_policy::clamp_tier_for_channel`]. Pinned by
    /// `execution_engine::turn_permissions`'s precedence tests.
    #[serde(default)]
    pub exec_tier: Option<String>,
    /// Session usage mode (chat / work / code) chosen in the composer for a
    /// conversation whose session does not exist yet. Third twin of
    /// `exec_tier` / `thinking` above: rides the request to govern the first
    /// turn, is stamped onto the session by `resolve_turn_mode`, and later
    /// turns read it back from `identity_meta.custom["session_mode"]`.
    /// Orthogonal to the tier — a mode repartitions the tool presentation
    /// surface (schema-resident vs deferred), never permissions.
    #[serde(default)]
    pub mode: Option<String>,
    /// Per-session memory mode (`"on"` / `"off"`) chosen by the caller. Fifth
    /// twin of `exec_tier` / `thinking` / `mode`: rides the request so it can
    /// govern the first turn of a conversation that has no session row yet, is
    /// stamped onto the session by `resolve_turn_memory_mode`, and later turns
    /// read it back from `identity_meta.custom["memory_mode"]`.
    ///
    /// Gates the three INJECTED memory envelopes only — never the memory tools,
    /// never writes. See `memory::session_memory_mode`.
    #[serde(default)]
    pub memory: Option<String>,
    /// Marks this run's user input as ASR-transcribed speech (the Panel voice
    /// loop). Wires the session voice-mode registry so `VoiceModeLayer`
    /// injects spoken-reply guidance into this turn's prompt, and applies the
    /// `[voice]` low-TTFT model pin when no explicit override is set —
    /// mirroring the channel voice path. Recomputed every turn: a typed turn
    /// (default `false`) clears the flag.
    #[serde(default)]
    pub voice_input: bool,
    /// Open this turn's session in a project room (P2, spec §6.2).
    ///
    /// Only consulted when the session does not exist yet. A session's scope is
    /// immutable (spec §10) — re-sending to an existing session with a
    /// different `project_id` does NOT move it, because its memory partition,
    /// its transcript and every artifact already written under the old scope
    /// cannot follow. See [`build_run_request`]'s attribution branch.
    #[serde(default)]
    pub project_id: Option<String>,
}

const fn default_stream() -> bool {
    true
}

/// Result of agent.run request (immediate response)
#[derive(Debug, Clone, Serialize)]
pub struct AgentRunResult {
    /// Unique run identifier
    pub run_id: String,
    /// Resolved session key
    pub session_key: String,
    /// Timestamp when accepted
    pub accepted_at: String,
}

/// Run state for tracking active runs
#[derive(Debug, Clone)]
pub struct RunState {
    pub run_id: String,
    pub session_key: SessionKey,
    pub started_at: Instant,
    pub status: RunStatus,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

/// Manager for agent runs
pub struct AgentRunManager {
    router: Arc<AgentRouter>,
    event_bus: Arc<GatewayEventBus>,
    active_runs: Arc<RwLock<HashMap<String, RunState>>>,
    agent_registry: Arc<AgentRegistry>,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    /// Live application config, used to read `behavior.output_mode` fresh per
    /// run so the global typewriter/instant switch reaches Panel runs too.
    /// `None` for test/host-only constructions → falls back to typewriter,
    /// matching the inbound channel path (`inbound_router::executor`).
    app_config: Option<Arc<RwLock<crate::Config>>>,
}

impl AgentRunManager {
    pub fn new(
        router: Arc<AgentRouter>,
        event_bus: Arc<GatewayEventBus>,
        agent_registry: Arc<AgentRegistry>,
        execution_adapter: Arc<dyn ExecutionAdapter>,
    ) -> Self {
        Self {
            router,
            event_bus,
            active_runs: Arc::new(RwLock::new(HashMap::new())),
            agent_registry,
            execution_adapter,
            app_config: None,
        }
    }

    /// Inject the live application config so Panel runs honor the global
    /// `behavior.output_mode` toggle. Mirrors
    /// `InboundRouterExecutor::with_app_config`.
    pub fn with_app_config(mut self, config: Arc<RwLock<crate::Config>>) -> Self {
        self.app_config = Some(config);
        self
    }

    /// Resolve the global output mode fresh from the live config. Reads the
    /// same `behavior.output_mode` key the chat channels read, so a runtime
    /// toggle takes effect on the very next Panel run with no restart. Absent
    /// config (tests, host-only) defaults to typewriter.
    async fn resolved_output_mode(&self) -> OutputMode {
        match &self.app_config {
            Some(cfg) => {
                let cfg = cfg.read().await;
                OutputMode::from_config(
                    cfg.behavior
                        .as_ref()
                        .map_or("typewriter", |b| b.output_mode.as_str()),
                )
            }
            None => OutputMode::Typewriter,
        }
    }

    /// Start a new agent run
    pub async fn start_run(&self, params: AgentRunParams) -> Result<AgentRunResult, String> {
        // Generate run ID
        let run_id = uuid::Uuid::new_v4().to_string();

        // Resolve session key
        let session_key = self
            .router
            .route(
                params.session_key.as_deref(),
                params.channel.as_deref(),
                params.peer_id.as_deref(),
                params.agent_id.as_deref(),
            )
            .await;

        let session_key_str = session_key.to_key_string();
        let accepted_at = chrono::Utc::now().to_rfc3339();

        // Create run state
        let run_state = RunState {
            run_id: run_id.clone(),
            session_key: session_key.clone(),
            started_at: Instant::now(),
            status: RunStatus::Running,
            input: params.input.clone(),
        };

        // Store in active runs
        {
            let mut runs = self.active_runs.write().await;
            runs.insert(run_id.clone(), run_state);
        }

        info!("Started run {} for session {}", run_id, session_key_str);

        // Emit run accepted event
        if params.stream {
            let event = StreamEvent::RunAccepted {
                run_id: run_id.clone(),
                session_key: session_key_str.clone(),
                accepted_at: accepted_at.clone(),
            };

            if let Ok(event_value) = serde_json::to_value(&event) {
                let notification = super::super::protocol::JsonRpcRequest::notification(
                    "stream.run_accepted",
                    Some(event_value),
                );
                if let Ok(json) = serde_json::to_string(&notification) {
                    self.event_bus.publish(json);
                }
            }
        }

        let agent_id = session_key.agent_id();
        let agent = match self.agent_registry.get(agent_id).await {
            Some(a) => a,
            None => {
                error!("Agent not found: {}", agent_id);
                let mut runs = self.active_runs.write().await;
                if let Some(run) = runs.get_mut(&run_id) {
                    run.status = RunStatus::Failed(format!("Agent not found: {agent_id}"));
                }
                return Err(format!("Agent not found: {agent_id}"));
            }
        };

        // `sessions: None` — the Simulated-execution fallback has no
        // `SessionStore` (see this type's doc and `method_visibility.rs`'s
        // carve-out for the same reason). Errors are surfaced as strings on
        // this path; the real-engine handlers keep the typed variants.
        let request = build_run_request(
            run_id.clone(),
            &session_key,
            params,
            self.app_config.as_ref(),
            None,
            agent.config(),
        )
        .await
        .map_err(|e| e.to_string())?;

        // Capture the originating surface before `request` is moved into the
        // spawn — used below to decide cross-surface reply fan-out.
        let current_channel = request
            .metadata
            .get("channel_id")
            .cloned()
            .unwrap_or_default();

        // Primary emitter: streams to the Panel via the gateway event bus.
        // Read the global output_mode fresh per run so the live
        // typewriter/instant switch reaches Panel runs — mirroring the inbound
        // channel path. Previously this hardcoded `GatewayEventEmitter::new`
        // (always typewriter), so the Panel — the primary surface — ignored the
        // global toggle entirely; instant mode only worked on chat channels.
        let output_mode = self.resolved_output_mode().await;
        let base_emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(
            GatewayEventEmitter::with_output_mode(self.event_bus.clone(), output_mode),
        );

        // Sub-gap (b): if this gateway/Panel run continues a session that
        // originated on an *external* channel (Telegram, ...), wrap the emitter
        // so the final reply is also delivered back to that origin channel —
        // keeping the two surfaces in sync. Inbound channel runs never reach
        // this handler (they deliver via ReplyEmitter), so there is no double
        // delivery; the surface check below skips same-channel continuations.
        let emitter: Arc<dyn EventEmitter + Send + Sync> = match (
            agent.origin_route(&session_key).await,
            crate::gateway::event_emitter::origin_fanout::channel_registry(),
        ) {
            (Some((origin_channel, origin_conversation)), Some(registry))
                if origin_channel != current_channel =>
            {
                Arc::new(
                    crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter::new(
                        base_emitter,
                        registry,
                        origin_channel,
                        origin_conversation,
                    ),
                )
            }
            _ => base_emitter,
        };

        let execution_adapter = self.execution_adapter.clone();
        let active_runs = self.active_runs.clone();
        let run_id_for_spawn = run_id.clone();

        tokio::spawn(async move {
            let result = execution_adapter.execute(request, agent, emitter).await;

            let mut runs = active_runs.write().await;
            if let Some(run) = runs.get_mut(&run_id_for_spawn) {
                run.status = match result {
                    Ok(()) => RunStatus::Completed,
                    Err(e) => RunStatus::Failed(e.to_string()),
                };
            }
        });

        Ok(AgentRunResult {
            run_id,
            session_key: session_key_str,
            accepted_at,
        })
    }

    /// Get status of a run
    pub async fn get_run_status(&self, run_id: &str) -> Option<RunState> {
        self.active_runs.read().await.get(run_id).cloned()
    }

    /// Cancel an active run
    pub async fn cancel_run(&self, run_id: &str) -> bool {
        // Forward to the execution engine FIRST: firing the run's
        // CancellationToken is the only thing that actually interrupts the
        // in-flight Think→Act loop (LLM call, tool execution, and between
        // iterations). Updating local status alone never reaches the running
        // task, which is why the chat-window Stop button used to do nothing.
        //
        // Resolve the session and stop the whole tree, not just this run. A
        // leader that delegated work (`task_manage`, delegate workflows,
        // background sub-agents) has its member runs registered under the
        // leader's session key, and only `cancel_session` walks them — bare
        // `cancel` fires one token and leaves the members running, detached,
        // still billing, with no surface left that can reach them. That walk
        // was added for `/stop` in 2026-07-24 and every run-id-addressed stop
        // (Panel `chat.abort`, TUI `/stop`, `aleph chat abort`) landed here and
        // missed it, while `cancel_session`'s own doc and FEATURE_LOCATOR §4.8
        // both claimed `chat.abort` took that path.
        //
        // Per-session mutual exclusion makes this exact, not approximate: if
        // the engine holds `run_id`, it IS the run on that session, so
        // `cancel_session` cancels that run and nothing else's. A run the
        // engine does not hold (finished, or still queued in its busy lane)
        // resolves to `None` and falls through to the run-id primitive — which
        // is also what `cancel_queued_run` below needs.
        let signalled = match self.execution_adapter.session_of_run(run_id).await {
            Some(session_key) => self
                .execution_adapter
                .cancel_session(&session_key)
                .await
                .is_ok_and(|cancelled| cancelled.is_some()),
            None => self.execution_adapter.cancel(run_id).await.is_ok(),
        };

        // A run can also be waiting in its session's busy lane — the client has
        // its `run_id` (both RPC handlers return one up front) but the engine
        // has never admitted it, so `cancel` above is a miss and Stop would do
        // nothing at all. Reaching it here is the only way to abandon a queued
        // message by run id (`/stop` covers the whole session instead).
        let dequeued = crate::gateway::busy_queue::cancel_queued_run(run_id);

        // Reflect the request in our local bookkeeping so status queries
        // report Cancelled even if the run finished between the signal and now.
        let mut runs = self.active_runs.write().await;
        let was_running = matches!(
            runs.get(run_id).map(|r| &r.status),
            Some(RunStatus::Running)
        );
        if was_running {
            if let Some(run) = runs.get_mut(run_id) {
                run.status = RunStatus::Cancelled;
            }
        }

        signalled || dequeued || was_running
    }

    /// List active runs
    pub async fn list_runs(&self) -> Vec<RunState> {
        self.active_runs.read().await.values().cloned().collect()
    }

    /// Snapshot of the execution engine's run-concurrency slot usage, for
    /// `gateway.metrics.run_concurrency` (Task 8, audit 3.4).
    pub fn concurrency_snapshot(&self) -> crate::gateway::execution_engine::ConcurrencySnapshot {
        self.execution_adapter.concurrency_snapshot()
    }

    /// Session keys with a run currently in flight (authoritative in-memory
    /// admission gate), for `gateway.metrics.run_concurrency` — lets the Panel
    /// paint per-session running indicators regardless of which interface
    /// started the run.
    #[must_use]
    pub fn running_sessions(&self) -> Vec<String> {
        self.execution_adapter.running_sessions()
    }

    /// The run currently in flight on `session_key`, from the same
    /// authoritative registry [`Self::running_sessions`] reads.
    ///
    /// Deliberately NOT resolved from `self.active_runs`: that map only holds
    /// runs this manager started (Panel `agent.run` / `chat.send`), so a
    /// session busy with a CLI, channel, cron or resumed turn would answer
    /// "nothing running" there — which is exactly the case a joining client
    /// needs answered.
    #[must_use]
    pub fn active_run_for_session(&self, session_key: &str) -> Option<String> {
        self.execution_adapter.active_run_for_session(session_key)
    }

    /// How long the run in flight has been running, in milliseconds.
    ///
    /// Same registry-vs-`active_runs` split as
    /// [`Self::active_run_for_session`], from the other side: that one reads
    /// the authoritative session registry because the run may not be ours,
    /// and this one reads the adapter's run table because the registry holds
    /// no timing. A run started by a CLI, channel or cron therefore answers
    /// here as long as this process is the one executing it.
    pub async fn run_elapsed_ms(&self, run_id: &str) -> Option<u64> {
        self.execution_adapter.run_elapsed_ms(run_id).await
    }
}

/// Why a turn could not be turned into a [`RunRequest`].
///
/// One variant per JSON-RPC code the transports must produce. Collapsing any
/// two would either turn a visibility denial into a chatty `INVALID_PARAMS`
/// (which tells the caller the project exists), or a param complaint into a
/// silent not-found, or an authorization refusal into a puzzle — and the
/// refusals differ on purpose: one has an existence secret to keep and one
/// does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildRunError {
    /// Malformed or unacceptable params → `INVALID_PARAMS`.
    Invalid(String),
    /// The turn named a project the caller cannot reach — or the check itself
    /// could not complete. Maps to `RESOURCE_NOT_FOUND` with the SAME message
    /// a project id that was never minted produces, so `chat.send` cannot be
    /// used as a cross-user existence oracle.
    ProjectNotFound(String),
    /// The turn asked to run **as** an agent this caller is not on the
    /// `allowed_users` list of. Maps to `PERMISSION_DENIED` and names the
    /// agent — deliberately NOT the `not_found` shape its `ProjectNotFound`
    /// sibling uses, because there is no existence secret to keep here:
    /// `agents.list` already returns every agent to every authenticated
    /// caller. Shaping this as "not found" would only mean an operator who
    /// forgot to add themselves to their own list gets a puzzle instead of an
    /// answer, and a puzzle is what pushes people to widen the setting.
    AgentForbidden(String),
}

impl From<String> for BuildRunError {
    fn from(s: String) -> Self {
        BuildRunError::Invalid(s)
    }
}

impl std::fmt::Display for BuildRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildRunError::Invalid(s) => f.write_str(s),
            BuildRunError::ProjectNotFound(id) => write!(f, "project not found: {id}"),
            BuildRunError::AgentForbidden(id) => write!(
                f,
                "not authorized to run as agent '{id}' (its `allowed_users` list does not include you)"
            ),
        }
    }
}

/// Resolve this turn's owner/scope attribution. **Two paths — writing only
/// one of them is the P2 bug this function exists to prevent.**
///
/// 1. **The session already exists** → its stored scope wins, full stop
///    (spec §10: a session's scope is immutable). The request's `project_id`
///    has no say. Getting this backwards means a run's memory writes land in a
///    different partition than the session's own stored scope — silently, with
///    no error anywhere, visible only as "the agent forgot".
/// 2. **The session does not exist yet** → the request decides. A
///    `project_id` the caller is not on the roster of is refused with the same
///    response an id that was never minted produces.
///
/// A legacy/unstamped EXISTING session keeps legacy semantics (personal to the
/// caller), matching `SessionMetadata`'s adoption-by-absence rule rather than
/// retro-stamping a row nobody asked to move.
///
/// `sessions: None` (the Simulated-execution fallback, which has no store)
/// cannot distinguish the two paths and takes path 2 for every turn. That is
/// the documented carve-out, not an oversight: without a store there is no
/// stored scope to honor.
async fn resolve_attribution(
    sessions: Option<&Arc<dyn crate::gateway::session_store::SessionStore>>,
    session_key: &SessionKey,
    project_id: Option<&str>,
) -> Result<Option<crate::scope::ScopeAttribution>, BuildRunError> {
    let user = crate::gateway::caller_identity::current_caller_user();

    if let Some(store) = sessions {
        match store.get_metadata(session_key).await {
            Ok(Some(meta)) => {
                // Path 1: the row owns its scope.
                return Ok(crate::scope::ScopeAttribution::from_persisted(
                    meta.owner_user_id.as_deref(),
                    meta.scope_id.as_deref(),
                )
                .or_else(|| {
                    user.as_deref()
                        .map(crate::scope::ScopeAttribution::personal)
                }));
            }
            Ok(None) => {}
            Err(e) => {
                // Fail closed: an unresolvable session must not fall through to
                // the request's own claim about which project this is.
                tracing::warn!(error = %e, "projects: session scope lookup failed closed");
                return Err(BuildRunError::ProjectNotFound(
                    project_id.unwrap_or_default().to_string(),
                ));
            }
        }
    }

    // Path 2: a session that does not exist yet.
    match project_id {
        // An explicit, reachable `project_id` ALWAYS produces a project stamp,
        // even for an unrestricted caller (cron opening a room's session). The
        // owner falls back to the legacy owner by absence — the same rule the
        // rest of P1 reads an unstamped row with. Deriving the owner from
        // `user` alone would silently drop the project scope for exactly the
        // callers that named it most explicitly.
        Some(pid) if crate::gateway::visibility::project_visible(pid) => {
            Ok(Some(crate::scope::ScopeAttribution {
                owner_user_id: user
                    .clone()
                    .unwrap_or_else(|| crate::gateway::security::store::OWNER_USER_ID.to_string()),
                scope: crate::scope::ScopeId::Project(pid.to_string()),
            }))
        }
        // Absent OR foreign project: one refusal, no oracle.
        Some(pid) => Err(BuildRunError::ProjectNotFound(pid.to_string())),
        // No `project_id` — but the KEY may already have been declared a room's
        // by `projects.room_session`, and that declaration is the only thing
        // that knows it in this window.
        //
        // `claim_session_key` writes `projects.current_session_key` when the
        // room is opened; the session ROW is created later, by whoever speaks
        // first. A bare `chat.send` into that gap (the Panel always sends
        // `project_id`, a plain RPC client need not) used to fall straight to
        // the personal arm below and stamp the row `personal:<first speaker>` —
        // PERMANENTLY, since `stamp_attribution` is create-only and
        // `attribution_backfill` only heals rows that are NULL/NULL, not rows
        // that are stamped wrong. The room then became invisible to every other
        // member including its owner, while `projects.list` kept listing it.
        // Recorded as still-open by round-3's real-machine QA.
        //
        // Asked only in the `None` arm: an explicit `project_id` is already
        // decided above (and refused if foreign), so this adds no second
        // answer to a question that has one.
        None => {
            // Which project may govern this turn FOR THIS CALLER. The two
            // arms of a room's claim are gated differently, and the asymmetry
            // is the point — see each arm.
            let governing = match crate::projects::ProjectStore::shared()
                .room_claiming(session_key)
            {
                // Arm 1 — an explicit `projects.room_session` claim naming
                // this exact key. Visibility still decides, and a caller the
                // roster does not admit gets the same refusal a NAMED foreign
                // project gets: for a room that declared this key its own, the
                // refusal shape must not depend on how the caller spelled it,
                // or the two spellings become an existence oracle.
                //
                // The refusal is load-bearing in the other direction too. This
                // key IS the room's own conversation, so falling through to
                // the personal arm below is exactly the permanent mis-stamp
                // the comment above describes: `personal:<first speaker>` on
                // the room's own row, forever.
                Some((pid, ClaimSource::ExplicitClaim)) => {
                    if !crate::gateway::visibility::project_visible(&pid) {
                        return Err(BuildRunError::ProjectNotFound(pid));
                    }
                    Some(pid)
                }
                // Arm 2 — a channel conversation an operator bound to a room.
                // A caller the roster does not admit falls through to the
                // personal arm, exactly as if nothing had claimed the key.
                //
                // NOT arm 1's ruling, deliberately. Arm 1 is a declaration
                // about this key; arm 2 is an inference from a conversation,
                // and the SAME principal in the SAME conversation speaking
                // through the CHANNEL never reaches this function at all —
                // `inbound_router/executor.rs` stamps `personal:<speaker>` and
                // `run_loop::request_scope`'s roster gate leaves it alone.
                // Refusing here would answer one question two different ways
                // depending on which door the caller came through, and would
                // answer it by handing them a project id they never typed —
                // telling them, on a room they cannot see, that this
                // conversation is bound to it. Falling through says nothing:
                // a bound conversation and an unbound one become
                // indistinguishable, which is one existence oracle fewer than
                // the refusal.
                //
                // The permanent-mis-stamp hazard arm 1's refusal prevents is
                // not prevented here by refusing: whatever this row ends up
                // stamped, `request_scope` stamps it identically for the same
                // principal arriving through the channel — the door that
                // carries a bound conversation's real traffic.
                Some((pid, ClaimSource::BoundConversation)) => {
                    crate::gateway::visibility::project_visible(&pid).then_some(pid)
                }
                None => None,
            };
            match governing {
                Some(pid) => Ok(Some(crate::scope::ScopeAttribution {
                    owner_user_id: user.clone().unwrap_or_else(|| {
                        crate::gateway::security::store::OWNER_USER_ID.to_string()
                    }),
                    scope: crate::scope::ScopeId::Project(pid),
                })),
                None => Ok(user
                    .as_deref()
                    .map(crate::scope::ScopeAttribution::personal)),
            }
        }
    }
}

/// The folder a run under `attribution` should work in, or `None` for a
/// personal/org session or an unbound room.
///
/// No visibility check here on purpose: `attribution` is what
/// [`resolve_attribution`] already admitted — either the session's own stored
/// scope or a `project_id` that passed `project_visible`. Re-asking would be
/// the second derivation, and asking a DIFFERENT question (is the caller on
/// the roster *now*) would mean a member removed mid-session gets a run with
/// no workspace instead of no access.
///
/// A store failure reads as "unbound" rather than propagating: the fallback is
/// the agent's own workspace, which is where the turn would have run before P2
/// anyway. The alternative — refusing the turn because SQLite hiccuped — turns
/// a degraded catalogue into a dead chat room.
fn bound_workspace_of(
    attribution: Option<&crate::scope::ScopeAttribution>,
) -> Option<std::path::PathBuf> {
    let crate::scope::ScopeId::Project(project_id) = &attribution?.scope else {
        return None;
    };
    match crate::projects::ProjectStore::shared().get(project_id) {
        Ok(project) => project?.workspace_path,
        Err(e) => {
            tracing::warn!(
                project_id = %project_id,
                error = %e,
                "projects: workspace lookup failed; falling back to the agent workspace"
            );
            None
        }
    }
}

/// Build the [`RunRequest`] for one Panel-originated turn (`agent.run` /
/// `chat.send`).
///
/// Single source of truth for the whole gateway→engine hand-off: run metadata
/// (surface / authorization / conversation routing), the `project_root` gate,
/// base64 attachment decoding, the MoA selector semantics and the voice model
/// pin. Both the real-`ExecutionEngine` handlers in `aleph-server`'s
/// `server_init` and the Simulated-fallback [`AgentRunManager::start_run`] call
/// it, so a field can never again be honored on one path and silently dropped
/// on the other (which is exactly how Panel attachments and the model picker
/// were lost on every real deployment).
///
/// `app_config` is `None` for test / host-only constructions: the locale and
/// the `[voice]` model pin are then skipped rather than guessed.
///
/// `sessions` is `None` on the Simulated-execution fallback path, which has no
/// `SessionStore` at all — the SAME carve-out `method_visibility.rs` already
/// records for `chat.send` / `agent.run`. Without a store this cannot tell an
/// existing session from a new one, so it treats every turn as new; see
/// [`resolve_attribution`].
///
/// Callers map [`BuildRunError`] to their transport's error shape, and the
/// variants must NOT be flattened into one another: a bad-params complaint, a
/// visibility denial that has to stay byte-identical to a project id that was
/// never minted, and an authorization refusal that deliberately does NOT hide
/// behind "not found" (see each variant's doc for which is which and why).
pub async fn build_run_request(
    run_id: String,
    session_key: &SessionKey,
    params: AgentRunParams,
    app_config: Option<&Arc<RwLock<crate::Config>>>,
    sessions: Option<&Arc<dyn crate::gateway::session_store::SessionStore>>,
    agent: &crate::gateway::agent_instance::AgentInstanceConfig,
) -> Result<RunRequest, BuildRunError> {
    // A human is here. This builder is the one funnel behind every Panel /
    // TUI / CLI run entrance, which makes it one of the DreamDaemon idle
    // sensor's two producers (the other is the channel inbound router) —
    // machine-originated runs (cron, heartbeat, teams, A2A, model-driven
    // `session_send`) build their requests elsewhere and deliberately do not
    // stamp, or overnight automation would starve dreaming forever. Stamped
    // before the authorization gate below on purpose: a refused attempt is
    // still a person at the keyboard. Guarded by `run_loop::tests::
    // every_run_producer_declares_whether_a_human_is_at_the_other_end`, which
    // derives that whitelist from the run-producer census rather than naming
    // it — so a third human entrance is asked the question when it appears.
    crate::memory::dreaming::record_activity();

    // The agent axis of authorization, and it runs FIRST — before the voice
    // registry write below, which is a side effect a refused turn must not
    // leave behind.
    //
    // `agent` is the config of the instance the caller's `agent_id` actually
    // resolved to, so this asks about the identity that would really run, not
    // about the string on the wire (the two differ whenever the registry falls
    // back to the default agent). Every run-start path funnels through this
    // builder — `handle_run_with_engine`, `handle_chat_send_with_engine` and
    // the Simulated-fallback `AgentRunManager::start_run` — which is why the
    // parameter is required rather than `Option`: a new surface cannot acquire
    // this hole by forgetting to opt in.
    if !crate::gateway::caller_identity::caller_may_act_as_agent(agent.allowed_users.as_deref()) {
        return Err(BuildRunError::AgentForbidden(agent.agent_id.clone()));
    }

    let session_key_str = session_key.to_key_string();

    // Record voice mode for this session BEFORE the run spawns, so prompt
    // assembly (`VoiceModeLayer` via the harness bridge) sees it on THIS
    // turn. Same registry the channel inbound path writes
    // (`inbound_router::executor`); channel sessions use distinct keys, so
    // the unconditional per-turn recompute here cannot clobber them.
    // A Panel voice turn is always ASR-transcribed input; the domain
    // vocabulary rides along so `VoiceModeLayer` can invite term-accurate
    // transcription repair — the same `[voice] vocabulary` hint that biases
    // ASR (one dictionary, two consumers).
    let voice_state = if params.voice_input {
        let vocabulary = match app_config {
            Some(cfg) => cfg.read().await.voice_local.vocabulary_hint(),
            None => None,
        };
        Some(crate::gateway::voice::voice_mode::VoiceTurnState::new(
            true, vocabulary,
        ))
    } else {
        None
    };
    crate::gateway::voice::voice_mode::set(&session_key_str, voice_state);

    let mut metadata = HashMap::new();
    metadata.insert(
        "channel_id".to_string(),
        params.channel.clone().unwrap_or_default(),
    );
    metadata.insert("sender_id".to_string(), "websocket".to_string());
    // The Panel is a WebRich surface. Stamp `platform=webchat` so the
    // run-loop (`run_loop::inner`) derives a WebRich InteractionManifest.
    // Without it `metadata["platform"]` is absent → the manifest is `None`
    // → `prompt_build` falls back to the `Background` paradigm, whose
    // `SilentReply` capability makes `ProtocolTokensLayer` teach the model
    // `ALEPH_SILENT_COMPLETE`. The model then emits that literal token as a
    // silent completion and it leaks verbatim into the chat bubble — the
    // interactive path has no protocol-token suppression (only cron's
    // `deliverable_text` strips it). Mirrors the team-broadcast
    // (`member_run_metadata`) and channel-inbound paths that already tag
    // platform.
    metadata.insert("platform".to_string(), "webchat".to_string());
    // A Panel chat has no channel-side conversation id, but HITL tools
    // (`ask_user`, approval routing) refuse to run when `conversation_id` is
    // empty (`TurnContext::is_channel_routable`). The session key IS the
    // Panel's conversation identity — stamp it so blocking clarification is
    // reachable from the Panel instead of failing closed.
    metadata.insert("conversation_id".to_string(), session_key_str.clone());
    if let Some(peer_id) = &params.peer_id {
        metadata.insert("peer_id".to_string(), peer_id.clone());
    }

    // Stamp the originating connection's authorization role (set by the
    // gateway dispatch loop via CALLER_ROLE) so the tool-dispatch tier gate
    // can reject config-mutating tools for chat-tier devices. Absent for
    // non-gateway runs (cron/internal) and for the local no-auth daemon → the
    // gate treats those as trusted.
    if let Some(role) = crate::gateway::caller_identity::current_caller_role() {
        metadata.insert("caller_role".to_string(), role);
    }

    // P1 data isolation + P2 project rooms: stamp the run's owner/scope
    // attribution. Two paths, and writing only the second is the silent bug
    // this comment exists to prevent — see `resolve_attribution`.
    let attribution =
        resolve_attribution(sessions, session_key, params.project_id.as_deref()).await?;
    if let Some(attr) = attribution.as_ref() {
        crate::scope::stamp_metadata(&mut metadata, attr);
    }

    // Who typed THIS message — a different fact from the scope owner above,
    // and in a project room they genuinely differ (every run in a room carries
    // the room's attribution). See `AUTHOR_USER_KEY`.
    if let Some(author) = crate::gateway::caller_identity::current_caller_user() {
        metadata.insert(
            crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
            author,
        );
    }

    if let Some(cfg) = app_config {
        let cfg = cfg.read().await;
        let lang = cfg.general.language.as_deref().unwrap_or("zh");
        metadata.insert("locale".to_string(), lang.to_string());
    }

    // Resolve this turn's working directory. Three tiers, highest first:
    //
    //   1. `params.project_root` — the caller names a folder for THIS turn.
    //      Gated: naming an arbitrary server directory is a config-tier
    //      capability (`caller_may_choose_directory`).
    //   2. the room's bound `workspace_path` — the session is scoped to a
    //      project that has a folder. NOT gated here, because the binding was
    //      already written through the same gate (`projects.bind_workspace` /
    //      `add` / `create_blank` all call it). A remote member chatting in a
    //      room the owner bound is not choosing a directory; they are using
    //      one that was chosen for them.
    //   3. neither — the agent's own default workspace, resolved downstream.
    //
    // Tier 2 keys off the RESOLVED attribution, not `params.project_id`: a
    // session's stored scope wins over the request (see `resolve_attribution`),
    // and the Panel only sends `project_id` on the turn that opens the room.
    // Reading the param would give the first message a workspace and every
    // later one the agent default — silently, mid-conversation.
    let workspace_override = match params.project_root.as_deref() {
        Some(raw) => {
            if !crate::gateway::caller_identity::caller_may_choose_directory() {
                return Err(format!(
                    "choosing a working directory requires config-tier authorization \
                     or a local (loopback) connection; this connection is paired at \
                     chat level from a remote address (project_root: {raw})"
                )
                .into());
            }
            let path = std::path::PathBuf::from(raw);
            if !path.is_absolute() {
                return Err(format!("project_root must be absolute: {raw}").into());
            }
            if !path.is_dir() {
                return Err(format!("project_root is not a directory: {raw}").into());
            }
            metadata.insert("project_root".to_string(), path.display().to_string());
            Some(path)
        }
        None => bound_workspace_of(attribution.as_ref())
            .map(|path| {
                // Fail loud rather than falling back to the agent's default
                // workspace. A room whose folder was unmounted or deleted
                // would otherwise keep answering — with every file tool
                // silently operating somewhere nobody in the room expects.
                // The cost is real and deliberate: an offline external drive
                // makes the room unusable until somebody re-binds it (bind to
                // `null` to go back to the agent default).
                if !path.is_dir() {
                    return Err(BuildRunError::from(format!(
                        "this project's folder is missing: {} — re-bind the project \
                         to another folder (projects.bind_workspace) or restore it",
                        crate::utils::paths::display_string(&path)
                    )));
                }
                // Same fact, same representation as tier 1: anything reading
                // `metadata["project_root"]` must not be able to tell which
                // tier put the run there (`resume_coordinator` mirrors the
                // override the same way).
                //
                // Deliberately NOT `paths::display_string` — this is the
                // machine-facing mirror of `workspace_override`, and it has
                // THREE producers (here, `resume_coordinator`,
                // `sessions::send_tool`) that all spell it `p.display()`.
                // Rendering only this one would make the tiers distinguishable,
                // which is the exact property the line above promises. The
                // path a human reads is a different carrier —
                // `identity_meta.custom["project_root"]`, surfaced as
                // `SessionInfo.project_root` — and the `\\?\` prefix is kept
                // off it at the boundary that serves it.
                metadata.insert("project_root".to_string(), path.display().to_string());
                Ok(path)
            })
            .transpose()?,
    };

    // Composer-chosen execution tier. Unknown ids are rejected rather than
    // ignored: silently falling back to the global tier would run the turn at
    // a WEAKER gate than the one the user believes they picked.
    if let Some(raw) = params.exec_tier.as_deref() {
        let tier = crate::config::types::policies::ExecTier::from_id(raw)
            .ok_or_else(|| format!("unknown exec_tier: {raw}"))?;
        metadata.insert(
            crate::config::types::policies::EXEC_TIER_SESSION_KEY.to_string(),
            tier.id().to_string(),
        );
    }

    // Composer-chosen session mode. Same fail-loud contract as exec_tier: a
    // silently-ignored unknown id would leave the user in a different mode
    // than the one the pill shows.
    if let Some(raw) = params.mode.as_deref() {
        let mode = crate::config::types::policies::SessionMode::from_id(raw)
            .ok_or_else(|| format!("unknown mode: {raw}"))?;
        metadata.insert(
            crate::config::types::policies::MODE_SESSION_KEY.to_string(),
            mode.id().to_string(),
        );
    }

    // Caller-chosen thinking depth. The exact same argument as exec_tier above:
    // a caller who asks for `xhigh` and silently gets the default is paying for
    // one thing and receiving another — and reasoning tokens bill at the OUTPUT
    // rate, so the discrepancy is expensive in both directions. This param was
    // declared and documented on the RPC, threaded through SendParams →
    // AgentRunParams, and then dropped on the floor here; every client that set
    // `thinking` was being lied to.
    if let Some(raw) = params.thinking.as_deref() {
        let level = crate::agents::thinking::normalize_think_level(raw)
            .ok_or_else(|| format!("unknown thinking level: {raw}"))?;
        metadata.insert(
            crate::agents::thinking::THINK_LEVEL_SESSION_KEY.to_string(),
            level.id().to_string(),
        );
    }

    // Caller-chosen memory mode. Same fail-loud contract as its four twins: a
    // caller who asks for a clean-room turn and silently gets their curated
    // memory folded in has been told the opposite of what happened.
    if let Some(raw) = params.memory.as_deref() {
        let mode = crate::memory::session_memory_mode::MemoryMode::from_id(raw)
            .ok_or_else(|| format!("unknown memory mode: {raw}"))?;
        metadata.insert(
            crate::memory::session_memory_mode::MEMORY_MODE_SESSION_KEY.to_string(),
            mode.id().to_string(),
        );
    }

    // Convert RPC attachments (base64 payload from chat.send / agent.run)
    // into channel attachments so the engine's media processor sees them.
    // Undecodable entries are skipped with a warning rather than failing the
    // whole run.
    let attachments: Vec<crate::gateway::channel::Attachment> = params
        .attachments
        .iter()
        .filter_map(|a| match BASE64.decode(a.data.as_bytes()) {
            Ok(bytes) => Some(crate::gateway::channel::Attachment {
                id: uuid::Uuid::new_v4().to_string(),
                mime_type: a.mime_type.clone(),
                filename: Some(a.name.clone()),
                size: Some(bytes.len() as u64),
                url: None,
                path: None,
                data: Some(bytes),
            }),
            Err(e) => {
                error!(name = %a.name, error = %e, "Dropping attachment with invalid base64 data");
                None
            }
        })
        .collect();

    // Round-2 E3: intercept the EXPLICIT picker override BEFORE the voice-pin
    // merge below. Only a deliberate user pick may arm or clear session MoA;
    // the voice fallback synthesizes an override from config (not user intent)
    // and must not disturb an armed MoA session — see
    // `apply_moa_selector_semantics`.
    let explicit_override = apply_moa_selector_semantics(&session_key_str, params.model_override);

    // Voice turns may pin a low-TTFT model (config `[voice]
    // llm_provider/llm_model`) so the spoken reply starts faster — the same pin
    // the channel voice path applies. An explicit per-turn override from the
    // model picker always wins.
    let model_override = match (explicit_override, params.voice_input, app_config) {
        (Some(o), _, _) => Some(o),
        (None, true, Some(cfg)) => {
            let cfg = cfg.read().await;
            crate::gateway::model_override::ModelOverride::from_voice(
                &cfg.voice_local.llm_provider,
                &cfg.voice_local.llm_model,
            )
        }
        (None, _, _) => None,
    };

    Ok(RunRequest {
        run_id,
        input: params.input,
        session_key: session_key.clone(),
        timeout_secs: None,
        metadata,
        attachments,
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override,
        max_iterations_override: None,
        model_override,
    })
}

/// Round-2 E3: a picker selection of the "moa" pseudo-provider arms session
/// MoA (sticky) instead of riding as a per-turn model override. Returns the
/// override that should continue down the run path (None when consumed).
/// An explicit NON-moa override clears any MoA sticky — the selector is one
/// exclusive slot. No override touches nothing (bare sends must not disturb
/// an armed MoA session).
///
/// Contract: call this with the RAW explicit `params.model_override` only,
/// BEFORE any synthesized fallback (e.g. the `[voice]` low-TTFT pin) is
/// merged in. A synthesized override is config-derived, not user intent —
/// it must never clear an armed MoA session.
///
/// Single call site: [`build_run_request`], which every `chat.send` /
/// `agent.run` path funnels through. (The `select_model` tool's own
/// `"moa:<preset>"` pick is a SEPARATE, inline arm/clear in `select_model.rs`
/// — this function does not back it.)
pub(crate) fn apply_moa_selector_semantics(
    session_key: &str,
    model_override: Option<crate::gateway::model_override::ModelOverride>,
) -> Option<crate::gateway::model_override::ModelOverride> {
    use crate::gateway::model_override::ModelOverride;
    match model_override {
        Some(ModelOverride::Qualified { provider, model }) if provider == "moa" => {
            // Round-3 F1: validate via the single arm source. An unknown preset
            // is NOT silently armed — notify the user and leave the session
            // unarmed (the override is still consumed, not passed through, so a
            // "moa" provider never rides the run path).
            if let Err(msg) =
                crate::providers::moa::activation::arm_sticky(session_key, Some(model))
            {
                crate::builtin_tools::notify_tool_result("moa", &msg, false);
            }
            None
        }
        Some(other) => {
            crate::providers::moa::activation::disarm(session_key);
            Some(other)
        }
        None => None,
    }
}

/// Handle agent.run RPC request
pub async fn handle_run(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    // Parse params
    let params: AgentRunParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Validate input
    if params.input.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Input cannot be empty");
    }

    // Start the run
    match run_manager.start_run(params).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

/// Handle agent.status RPC request
/// Parameters for agent.status / agent.cancel
#[derive(Debug, Deserialize)]
struct RunIdParams {
    run_id: String,
}

/// May this caller address `run_id`?
///
/// The third addressing shape. `visibility::session_visible` answers "may I
/// touch this session key" and `teams::gate_task` answers "may I touch this
/// task id"; a bare `run_id` was the one caller-supplied identifier in the
/// gateway with no predicate at all. It is not a weaker capability than a
/// session key — `AgentRunManager::cancel_run` reaches into two process-global
/// structures (the execution adapter's cancellation token registry and every
/// busy-queue lane), so a run id is a cross-user *kill* verb, and
/// `handle_status` hands back `session_key`, which is exactly the fact §5.22 ⑤
/// spent a round projecting out of `stream.running_set_changed`.
///
/// Resolution is run → session key → metadata → the ONE visibility predicate.
/// It deliberately does not shape its own refusal: each caller returns the
/// response it already returned for an unknown run id, byte-for-byte, so this
/// gate opens no existence oracle of its own.
///
/// A run the manager has no record of resolves to `None` — the caller then
/// answers as it always has for an unknown id.
pub(crate) async fn caller_may_address_run(
    run_id: &str,
    run_manager: &AgentRunManager,
    session_store: &dyn crate::gateway::session_store::SessionStore,
) -> bool {
    let Some(state) = run_manager.get_run_status(run_id).await else {
        // Unknown run: nothing to protect, and the caller's existing
        // "not found" answer is the correct one either way.
        return true;
    };
    match session_store.get_metadata(&state.session_key).await {
        Ok(Some(meta)) => crate::gateway::visibility::session_visible(&meta),
        // No row yet: a run is accepted before `ensure_session` writes its row,
        // so this is the ordinary first-turn window, not a foreign session.
        Ok(None) => true,
        // Fail closed: we could not establish whose run this is.
        Err(_) => false,
    }
}

/// The engine's run status as `agent.status` reports it: the wire word, and
/// the reason when there is one.
///
/// The exhaustive match is the point. A new `RunStatus` variant is a compile
/// error here, which forces whoever adds it to answer "what word does a client
/// see" rather than inheriting a `_ =>` arm — and an unrecognized word is read
/// by every client as "not running" (see [`aleph_protocol::RunPhase`]).
///
/// The reason used to be dropped. All four states flattened to a bare word, so
/// a client told `failed` had nothing to show its user but that word, while the
/// engine held the provider error the whole time.
fn reported_status(status: &RunStatus) -> (&'static str, Option<String>) {
    use aleph_protocol::AgentRunStatusReport as Report;
    match status {
        RunStatus::Running => (Report::RUNNING, None),
        RunStatus::Completed => (Report::COMPLETED, None),
        RunStatus::Failed(reason) => (Report::FAILED, Some(reason.clone())),
        RunStatus::Cancelled => (Report::CANCELLED, None),
    }
}

pub async fn handle_status(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
    session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
) -> JsonRpcResponse {
    let params: aleph_protocol::AgentRunStatusRequest = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if !caller_may_address_run(&params.run_id, &run_manager, session_store.as_ref()).await {
        // The same response an unknown run id gets — a foreign run and a
        // missing one must be indistinguishable, or this handler stays the
        // session-key oracle it was.
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Run not found");
    }

    match run_manager.get_run_status(&params.run_id).await {
        Some(state) => {
            let (status, error) = reported_status(&state.status);
            // Built FROM the shared contract type, not from a `json!` literal:
            // a client reads this through `aleph_protocol::AgentRunStatusReport`,
            // and constructing the response from that type makes both halves of
            // the drift impossible at compile time — a field the client expects
            // and the server stopped sending, and a field the server sends that
            // nothing reads. A parse-side assertion can only ever catch the
            // first, which is how a whole `AgentEnv` once went out on
            // `workspace.get`.
            JsonRpcResponse::success(
                request.id,
                json!(aleph_protocol::AgentRunStatusReport {
                    run_id: state.run_id,
                    session_key: state.session_key.to_key_string(),
                    status: status.to_string(),
                    elapsed_ms: state.started_at.elapsed().as_millis() as u64,
                    error,
                }),
            )
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Run not found"),
    }
}

/// Handle agent.cancel RPC request
pub async fn handle_cancel(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
    session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
) -> JsonRpcResponse {
    let params: RunIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if !caller_may_address_run(&params.run_id, &run_manager, session_store.as_ref()).await {
        // `cancelled: false` is what an unknown run id already produces, so a
        // refusal reads as "that run isn't cancellable", never as "that run is
        // someone else's".
        return JsonRpcResponse::success(
            request.id,
            json!({
                "run_id": params.run_id,
                "cancelled": false,
            }),
        );
    }

    let cancelled = run_manager.cancel_run(&params.run_id).await;
    JsonRpcResponse::success(
        request.id,
        json!({
            "run_id": params.run_id,
            "cancelled": cancelled,
        }),
    )
}

/// Handle agents.list RPC request
pub async fn handle_list(request: JsonRpcRequest, router: Arc<AgentRouter>) -> JsonRpcResponse {
    let agents = router.list_agents().await;
    JsonRpcResponse::success(
        request.id,
        json!({
            "agents": agents,
            "default": router.default_agent(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::execution_engine::{
        ExecutionError, RunRequest, RunState, RunStatus as EngineRunStatus,
    };
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::SessionStore;
    use async_trait::async_trait;
    use chrono::Utc;

    /// The `agent.run` contract, reconciled across the crate boundary.
    ///
    /// `aleph-tui` and `aleph-cli` cannot depend on `alephcore`, so they build
    /// their request from [`aleph_protocol::AgentRunRequest`]. This crate
    /// depends on both — it is the only place an assertion can compare the two
    /// sides instead of comparing a literal to itself, which is exactly how the
    /// original defect survived: the TUI sent `message`, the handler required
    /// `input`, and every send since the TUI was written came back
    /// `INVALID_PARAMS` with no test to notice.
    ///
    /// The check runs in the deserialize direction on purpose — a client
    /// **constructs** the wire from the shared type, so what must hold is that
    /// the handler accepts everything that type can emit. Renaming
    /// `AgentRunParams::input` turns this red; renaming
    /// `AgentRunRequest::input` turns the clients into compile errors.
    #[test]
    fn the_shared_run_request_deserializes_into_the_handler_params() {
        let shared = aleph_protocol::AgentRunRequest {
            input: "hello".into(),
            session_key: Some("agent:main:main".into()),
            channel: Some("cli:term1".into()),
            agent_id: Some("main".into()),
            thinking: Some("high".into()),
            mode: Some("code".into()),
            exec_tier: Some("auto".into()),
        };
        let wire = serde_json::to_value(&shared).expect("serialize shared request");
        let parsed: AgentRunParams =
            serde_json::from_value(wire.clone()).unwrap_or_else(|e| panic!("{e}: {wire}"));

        assert_eq!(parsed.input, shared.input);
        assert_eq!(parsed.session_key, shared.session_key);
        assert_eq!(parsed.channel, shared.channel);
        assert_eq!(parsed.agent_id, shared.agent_id);
        assert_eq!(parsed.thinking, shared.thinking);
        assert_eq!(parsed.mode, shared.mode);
        assert_eq!(parsed.exec_tier, shared.exec_tier);
    }

    /// The response half of the same contract, checked by **key-set equality**,
    /// not by parsing.
    ///
    /// Parsing only proves the server sends a superset: serde ignores unknown
    /// keys, so a `from_value` assertion is structurally blind both to a field
    /// the server stopped sending under a new name and to fields it sends that
    /// nothing reads. Equality catches the first — which for this response
    /// means the TUI silently keeping its own (possibly unroutable) session key
    /// forever — and the second, which is how a whole `AgentEnv` once went out
    /// on `workspace.get`.
    ///
    /// The expected set is derived from the contract type by serializing it, so
    /// it cannot drift into a hand-maintained literal.
    #[test]
    fn the_run_result_carries_exactly_what_the_shared_type_declares() {
        fn keys<T: serde::Serialize>(value: &T) -> std::collections::BTreeSet<String> {
            serde_json::to_value(value)
                .expect("serialize")
                .as_object()
                .expect("object")
                .keys()
                .cloned()
                .collect()
        }

        let server = AgentRunResult {
            run_id: "r-1".into(),
            session_key: "agent:main:main:s2".into(),
            accepted_at: "2026-08-11T00:00:00+00:00".into(),
        };
        let contract = aleph_protocol::AgentRunAccepted::default();

        assert_eq!(
            keys(&server),
            keys(&contract),
            "agent.run's response and the shared client contract disagree about which fields \
             exist; a client cannot adopt a canonical session key it is not sent"
        );

        // And the value a thin client actually depends on survives the trip.
        let parsed: aleph_protocol::AgentRunAccepted =
            serde_json::from_value(serde_json::to_value(&server).expect("serialize"))
                .expect("parse");
        assert_eq!(parsed.session_key, "agent:main:main:s2");
    }

    /// Every state the engine can be in has a word the shared contract reads.
    ///
    /// `RunPhase::Unrecognized` is the client's honest "I have never heard of
    /// this", and it settles a `/btw` overlay rather than leaving it spinning —
    /// correct for an old client against a new gateway, and a silent
    /// mis-report if the *current* gateway ever emits a word its own contract
    /// does not name. The `reported_status` match is exhaustive, so a new
    /// `RunStatus` variant is a compile error there; this is what stops the
    /// author answering that compile error with a word nobody reads.
    #[test]
    fn every_engine_run_status_has_a_word_the_shared_contract_recognizes() {
        use aleph_protocol::{AgentRunStatusReport, RunPhase};

        let cases = [
            (RunStatus::Running, RunPhase::Running),
            (RunStatus::Completed, RunPhase::Completed),
            (RunStatus::Failed("boom".into()), RunPhase::Failed),
            (RunStatus::Cancelled, RunPhase::Cancelled),
        ];
        for (status, expected) in cases {
            let (word, _) = reported_status(&status);
            let report = AgentRunStatusReport {
                status: word.to_string(),
                ..AgentRunStatusReport::default()
            };
            assert_eq!(
                report.phase(),
                expected,
                "{status:?} is reported as {word:?}, which no client can read"
            );
        }
    }

    /// A failure reaches the client with its reason attached.
    ///
    /// The wire dropped it for as long as this handler existed: `RunStatus`
    /// carries the provider error, the response flattened to a bare `"failed"`,
    /// and a client that had to explain the failure to a human had only the
    /// word. Every other state carries no reason — `Some("")` there would be a
    /// failure with nothing to say.
    #[test]
    fn only_a_failed_run_reports_a_reason_and_it_reports_the_real_one() {
        assert_eq!(
            reported_status(&RunStatus::Failed("provider 429".into())).1,
            Some("provider 429".to_string())
        );
        for quiet in [
            RunStatus::Running,
            RunStatus::Completed,
            RunStatus::Cancelled,
        ] {
            assert_eq!(
                reported_status(&quiet).1,
                None,
                "{quiet:?} invented a reason"
            );
        }
    }

    /// The request half of the same contract. `agent.status` is the only
    /// run-id-keyed read a thin client has, and the `/btw` overlay is the
    /// caller that cannot fall back to a session key — a side question runs on
    /// a derived session whose key is hashed server-side.
    #[test]
    fn the_shared_status_request_deserializes_into_the_handler_params() {
        let shared = aleph_protocol::AgentRunStatusRequest {
            run_id: "r-1".into(),
        };
        let wire = serde_json::to_value(&shared).expect("serialize");
        let parsed: aleph_protocol::AgentRunStatusRequest =
            serde_json::from_value(wire.clone()).unwrap_or_else(|e| panic!("{e}: {wire}"));
        assert_eq!(parsed.run_id, "r-1");
    }

    /// The minimal request — a client that only has text and a session — must
    /// parse too. The maximal case above would still pass if `input` had picked
    /// up a `#[serde(default)]`, which would turn a client's *missing* field
    /// into a silently empty prompt rather than a refusal.
    #[test]
    fn the_shared_run_request_parses_with_only_input_and_session() {
        let shared = aleph_protocol::AgentRunRequest::new("agent:main:main", "hi");
        let wire = serde_json::to_value(&shared).expect("serialize");
        let parsed: AgentRunParams =
            serde_json::from_value(wire.clone()).unwrap_or_else(|e| panic!("{e}: {wire}"));
        assert_eq!(parsed.input, "hi");
        assert!(parsed.attachments.is_empty());
        assert!(parsed.model_override.is_none());
    }

    /// Build a registry containing a "main" AgentInstance backed by a tempdir
    /// FileSessionStore. Holds onto the TempDir so callers can keep it alive
    /// for the duration of the test (dropping it tears down the workspace).
    async fn registry_with_main_agent() -> (Arc<AgentRegistry>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let config = AgentInstanceConfig {
            agent_id: "main".into(),
            workspace: tmp.path().join("workspace"),
            agent_dir: tmp.path().join("agent"),
            ..AgentInstanceConfig::default()
        };
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: tmp.path().join("sessions"),
            ..FileSessionStoreConfig::default()
        })
        .expect("FileSessionStore::new");
        let session_store: Arc<dyn SessionStore> = Arc::new(store);
        let instance = AgentInstance::new(config, session_store).expect("AgentInstance::new");
        let registry = Arc::new(AgentRegistry::new());
        registry.register(instance).await;
        (registry, tmp)
    }

    struct MockExecutionAdapter;

    #[async_trait]
    impl ExecutionAdapter for MockExecutionAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
            Err(ExecutionError::RunNotFound(run_id.to_string()))
        }

        async fn get_status(&self, run_id: &str) -> Option<EngineRunStatus> {
            Some(EngineRunStatus {
                run_id: run_id.to_string(),
                state: RunState::Completed,
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
                steps_completed: 0,
                current_tool: None,
            })
        }

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    /// Execution adapter that records every `cancel(run_id)` it receives, so a
    /// test can assert the run manager actually forwards cancellation to the
    /// execution engine (the only thing that fires the run's CancellationToken).
    #[derive(Default)]
    struct RecordingExecutionAdapter {
        cancelled: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ExecutionAdapter for RecordingExecutionAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
            self.cancelled
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(run_id.to_string());
            Ok(())
        }

        async fn get_status(&self, run_id: &str) -> Option<EngineRunStatus> {
            Some(EngineRunStatus {
                run_id: run_id.to_string(),
                state: RunState::Completed,
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
                steps_completed: 0,
                current_tool: None,
            })
        }

        async fn active_run_count(&self) -> usize {
            0
        }
    }

    #[tokio::test]
    async fn test_agent_run_manager() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        let params = AgentRunParams {
            input: "Hello, world!".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        };

        let result = manager.start_run(params).await.unwrap();
        assert!(!result.run_id.is_empty());
        assert!(result.session_key.starts_with("agent:main:"));
    }

    // The Panel voice loop sends chat.send { voice_input: true }. start_run
    // must record that in the session voice-mode registry BEFORE the run
    // spawns (VoiceModeLayer reads it at prompt assembly), and a following
    // typed turn must clear it — otherwise the spoken-reply style leaks into
    // text chat.
    #[tokio::test]
    async fn voice_input_round_trips_through_session_mode_registry() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        // Route to a DM session with a peer_id unique to this test. `session_mode`
        // is a process-global map keyed by session key; with `session_key: None`
        // this test's `AgentRouter::new()` (no session_store ⇒ no per-call epoch)
        // resolved to the shared `agent:main:main` key that every other concurrent
        // `session_key: None` test in this binary also writes — a sibling's
        // `voice_input: false` turn `remove`s the entry between this test's `set`
        // and `get`, making the assertion below flaky under `--tests` load. A
        // unique peer key gives this test its own registry entry, no one else touches.
        let voice_params = AgentRunParams {
            input: "把窗帘拉上".to_string(),
            session_key: None,
            channel: None,
            peer_id: Some("voice-roundtrip".to_string()),
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: true,
            project_id: None,
        };
        let result = manager.start_run(voice_params).await.unwrap();
        assert!(
            crate::gateway::voice::voice_mode::get(&result.session_key)
                .is_some_and(|s| s.transcribed),
            "a voice turn must mark the session voice-active with transcribed input"
        );

        // Same session, typed turn → the flag clears.
        let typed_params = AgentRunParams {
            input: "thanks".to_string(),
            session_key: Some(result.session_key.clone()),
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        };
        let result2 = manager.start_run(typed_params).await.unwrap();
        assert_eq!(result2.session_key, result.session_key);
        assert!(
            crate::gateway::voice::voice_mode::get(&result.session_key).is_none(),
            "a typed turn must clear the voice-mode flag"
        );
    }

    // Regression: a Panel run must stamp `metadata["platform"]="webchat"` so the
    // run-loop derives a WebRich (interactive) InteractionManifest. Without the
    // tag the manifest falls back to `Background`, whose `SilentReply` capability
    // makes `ProtocolTokensLayer` teach `ALEPH_SILENT_COMPLETE`; the model then
    // emits that literal token on a silent completion and it leaks into the chat
    // bubble. This captures the RunRequest the handler hands to the execution
    // adapter and asserts the platform tag is present.
    #[tokio::test]
    async fn panel_run_tags_webchat_platform() {
        struct CapturingAdapter {
            platform: Arc<std::sync::Mutex<Option<Option<String>>>>,
        }
        #[async_trait]
        impl ExecutionAdapter for CapturingAdapter {
            async fn execute(
                &self,
                request: RunRequest,
                _agent: Arc<AgentInstance>,
                _emitter: Arc<dyn EventEmitter + Send + Sync>,
            ) -> Result<(), ExecutionError> {
                *self.platform.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(request.metadata.get("platform").cloned());
                Ok(())
            }
            async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
                Err(ExecutionError::RunNotFound(run_id.to_string()))
            }
            async fn get_status(&self, _run_id: &str) -> Option<EngineRunStatus> {
                None
            }
            async fn active_run_count(&self) -> usize {
                0
            }
        }

        let captured: Arc<std::sync::Mutex<Option<Option<String>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let adapter = Arc::new(CapturingAdapter {
            platform: captured.clone(),
        });
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let manager = AgentRunManager::new(
            Arc::new(AgentRouter::new()),
            Arc::new(GatewayEventBus::new()),
            agent_registry,
            adapter,
        );
        let params = AgentRunParams {
            input: "hi".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        };
        manager.start_run(params).await.expect("start_run");

        // Execution is spawned (`tokio::spawn`), so poll until the adapter
        // records the RunRequest the handler built.
        let mut captured_platform = None;
        for _ in 0..200 {
            if let Some(p) = captured.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                captured_platform = Some(p);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            captured_platform,
            Some(Some("webchat".to_string())),
            "Panel run must tag platform=webchat (else Background paradigm → ALEPH_SILENT_COMPLETE leaks)"
        );
    }

    // Regression: Panel runs must honor the global `behavior.output_mode`
    // toggle fresh per run (the same key chat channels read). The bug this
    // guards: `start_run` used to hardcode `GatewayEventEmitter::new`
    // (typewriter), so flipping the switch to "instant" had no effect on the
    // Panel — the primary surface — even though every chat channel obeyed it.
    #[tokio::test]
    async fn panel_run_resolves_global_output_mode() {
        fn manager(cfg: Option<crate::Config>) -> AgentRunManager {
            let m = AgentRunManager::new(
                Arc::new(AgentRouter::new()),
                Arc::new(GatewayEventBus::new()),
                Arc::new(AgentRegistry::new()),
                Arc::new(MockExecutionAdapter),
            );
            match cfg {
                Some(c) => m.with_app_config(Arc::new(RwLock::new(c))),
                None => m,
            }
        }

        // Absent config (tests / host-only) → typewriter, matching the inbound
        // channel fallback in `inbound_router::executor`.
        assert!(matches!(
            manager(None).resolved_output_mode().await,
            OutputMode::Typewriter
        ));

        // Global switch = "instant" → Panel run resolves to instant.
        let instant: crate::Config =
            serde_json::from_value(json!({ "behavior": { "output_mode": "instant" } })).unwrap();
        assert!(matches!(
            manager(Some(instant)).resolved_output_mode().await,
            OutputMode::Instant
        ));

        // Global switch = "typewriter" → typewriter.
        let typewriter: crate::Config =
            serde_json::from_value(json!({ "behavior": { "output_mode": "typewriter" } })).unwrap();
        assert!(matches!(
            manager(Some(typewriter)).resolved_output_mode().await,
            OutputMode::Typewriter
        ));
    }

    /// REGRESSION (bugs #2/#9): the real `chat.send` handler used to build its
    /// own RunRequest with `attachments: Vec::new()` / `model_override: None`
    /// and no surface metadata. Both handlers now go through
    /// `build_run_request`, so one assertion covers both: attachments are
    /// base64-decoded, the picker's model override rides through, and the turn
    /// is stamped with `platform` (else the Background paradigm leaks
    /// `ALEPH_SILENT_COMPLETE` into chat), `caller_role` (else the config-tier
    /// tool gate can never fire) and `conversation_id` (else `ask_user` is
    /// unroutable).
    #[tokio::test]
    async fn build_run_request_carries_attachments_override_and_identity() {
        use crate::gateway::model_override::ModelOverride;

        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            input: "describe this".to_string(),
            session_key: None,
            channel: Some("gui:chat".to_string()),
            peer_id: None,
            stream: true,
            thinking: None,
            attachments: vec![Attachment {
                name: "shot.png".to_string(),
                mime_type: "image/png".to_string(),
                data: BASE64.encode(b"png-bytes"),
            }],
            agent_id: None,
            project_root: None,
            model_override: Some(ModelOverride::Qualified {
                provider: "anthropic".to_string(),
                model: "claude-opus-4-8".to_string(),
            }),
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        };

        let request = crate::gateway::caller_identity::CALLER_ROLE
            .scope(
                Some("guest".to_string()),
                build_run_request(
                    "run-1".to_string(),
                    &session_key,
                    params,
                    None,
                    None,
                    &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                ),
            )
            .await
            .expect("build_run_request");

        assert_eq!(request.attachments.len(), 1);
        assert_eq!(
            request.attachments[0].data.as_deref(),
            Some(&b"png-bytes"[..]),
            "attachments must be base64-decoded, not dropped"
        );
        assert_eq!(
            request.model_override,
            Some(ModelOverride::Qualified {
                provider: "anthropic".to_string(),
                model: "claude-opus-4-8".to_string(),
            }),
            "the chat-window model picker must be honored"
        );
        assert_eq!(
            request.metadata.get("platform").map(String::as_str),
            Some("webchat")
        );
        assert_eq!(
            request.metadata.get("caller_role").map(String::as_str),
            Some("guest")
        );
        assert_eq!(
            request.metadata.get("conversation_id").map(String::as_str),
            Some(session_key.to_key_string().as_str()),
            "ask_user / approval routing refuse a turn with an empty conversation_id"
        );
    }

    /// The DreamDaemon idle sensor: every human run entrance funnels through
    /// this builder, so building a request must advance the activity stamp.
    ///
    /// The stamp is one process-global atomic, so the assertion is
    /// directional only — back-date it, run the builder, require it to have
    /// jumped to roughly now. A parallel test that also stamps can only make
    /// it MORE recent, never restore the back-dated value, so interference
    /// cannot turn a real regression green... except in the narrow window
    /// where a sibling stamps between our back-date and our assert — which is
    /// why the load-bearing guard is the source-level census in
    /// `run_loop::tests::every_run_producer_declares_whether_a_human_is_at_the_
    /// other_end`; this test adds the behavioural half.
    #[tokio::test]
    async fn build_run_request_stamps_the_dream_idle_sensor() {
        let session_key = AgentRouter::new().route(None, None, None, None).await;
        crate::memory::dreaming::set_last_activity_for_test(1_000_000);
        let _ = build_run_request(
            "run-activity".to_string(),
            &session_key,
            base_params(),
            None,
            None,
            &crate::gateway::agent_instance::AgentInstanceConfig::default(),
        )
        .await
        .expect("build_run_request");
        let after = crate::memory::dreaming::last_activity_for_test();
        assert!(
            after > 1_000_000,
            "building a human run request must advance the dream idle stamp"
        );
    }

    /// A minimal Panel turn — the shape `chat.send` produces before any picker
    /// is touched.
    fn base_params() -> AgentRunParams {
        AgentRunParams {
            input: "hi".to_string(),
            session_key: None,
            channel: Some("gui:chat".to_string()),
            peer_id: None,
            stream: true,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        }
    }

    /// The composer's tier must reach THIS turn. A conversation has no session
    /// until its first message creates one, so a tier that only ever landed in
    /// session metadata could not govern the first turn — the one the user
    /// armed the gate for. It rides on the request instead.
    #[tokio::test]
    async fn build_run_request_carries_the_composer_exec_tier() {
        use crate::config::types::policies::EXEC_TIER_SESSION_KEY;

        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            exec_tier: Some("ask".to_string()),
            ..base_params()
        };

        let request = build_run_request(
            "run-tier".to_string(),
            &session_key,
            params,
            None,
            None,
            &crate::gateway::agent_instance::AgentInstanceConfig::default(),
        )
        .await
        .expect("build_run_request");

        assert_eq!(
            request
                .metadata
                .get(EXEC_TIER_SESSION_KEY)
                .map(String::as_str),
            Some("ask"),
            "a tier picked before the session existed must still govern this turn"
        );
    }

    /// An unknown tier id is refused, not ignored. Dropping it would run the
    /// turn at the GLOBAL tier — potentially weaker than the one the user
    /// believes is armed, and silently so.
    #[tokio::test]
    async fn build_run_request_rejects_an_unknown_exec_tier() {
        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            exec_tier: Some("yolo".to_string()),
            ..base_params()
        };

        let err = build_run_request(
            "run-bad".to_string(),
            &session_key,
            params,
            None,
            None,
            &crate::gateway::agent_instance::AgentInstanceConfig::default(),
        )
        .await
        .expect_err("an unknown tier must not silently fall back");
        assert!(
            err.to_string().contains("yolo"),
            "error should name the bad tier: {err}"
        );
    }

    /// Third twin of the exec-tier pair above: a mode picked before the
    /// session existed must ride the request into this turn's metadata.
    #[tokio::test]
    async fn build_run_request_carries_the_composer_mode() {
        use crate::config::types::policies::MODE_SESSION_KEY;

        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            mode: Some("code".to_string()),
            ..base_params()
        };

        let request = build_run_request(
            "run-mode".to_string(),
            &session_key,
            params,
            None,
            None,
            &crate::gateway::agent_instance::AgentInstanceConfig::default(),
        )
        .await
        .expect("build_run_request");

        assert_eq!(
            request.metadata.get(MODE_SESSION_KEY).map(String::as_str),
            Some("code"),
            "a mode picked before the session existed must still govern this turn"
        );
    }

    /// An unknown mode id is refused, not ignored — the pill would show one
    /// partition while the turn silently ran another.
    #[tokio::test]
    async fn build_run_request_rejects_an_unknown_mode() {
        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            mode: Some("game".to_string()),
            ..base_params()
        };

        let err = build_run_request(
            "run-bad-mode".to_string(),
            &session_key,
            params,
            None,
            None,
            &crate::gateway::agent_instance::AgentInstanceConfig::default(),
        )
        .await
        .expect_err("an unknown mode must not silently fall back");
        assert!(
            err.to_string().contains("game"),
            "error should name the bad mode: {err}"
        );
    }

    #[tokio::test]
    async fn test_run_status() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        let params = AgentRunParams {
            input: "Test".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: None,
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        };

        let result = manager.start_run(params).await.unwrap();

        // Should be able to get status
        let status = manager.get_run_status(&result.run_id).await;
        assert!(status.is_some());
    }

    /// Layer-2 gate: a chat-tier ("guest") connection may converse but must not
    /// pick an arbitrary working directory — even a valid absolute dir is denied.
    #[tokio::test]
    async fn chat_tier_caller_cannot_choose_project_root() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        // A real directory the run is allowed to name — bound here so the
        // guard removes it when the test ends.
        let (_scratch, project_root) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&project_root).unwrap();
        let params = AgentRunParams {
            input: "work over here".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: Some(project_root.display().to_string()),
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        };

        let result = crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("guest".to_string()), manager.start_run(params))
            .await;
        let err = result.expect_err("chat-tier project_root override must be rejected");
        assert!(err.contains("config-tier"), "unexpected error: {err}");
    }

    /// Config-tier ("operator") callers may choose a working directory. Does
    /// NOT cover an absent role — this test only ever scopes `CALLER_ROLE` to
    /// `Some("operator")` below. An absent role is refused (unless loopback)
    /// as of `546984c2b`; that path is pinned in `caller_identity`'s own
    /// tests (`an_unscoped_ambient_caller_may_not_choose_a_directory`), not
    /// here.
    #[tokio::test]
    async fn config_tier_caller_may_choose_project_root() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        // A real directory the run is allowed to name — bound here so the
        // guard removes it when the test ends.
        let (_scratch, project_root) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&project_root).unwrap();
        let params = AgentRunParams {
            input: "work over here".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: Some(project_root.display().to_string()),
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        };

        let result = crate::gateway::caller_identity::CALLER_ROLE
            .scope(Some("operator".to_string()), manager.start_run(params))
            .await;
        assert!(
            result.is_ok(),
            "operator project_root override must succeed: {result:?}"
        );
    }

    /// Desktop App: a chat-tier ("guest") connection over loopback may choose a
    /// project_root — the local Panel is the operator. This is the additive
    /// loopback allowance on top of the config-tier gate.
    #[tokio::test]
    async fn loopback_chat_tier_caller_may_choose_project_root() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        // A real directory the run is allowed to name — bound here so the
        // guard removes it when the test ends.
        let (_scratch, project_root) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&project_root).unwrap();
        let params = AgentRunParams {
            input: "work over here".to_string(),
            session_key: None,
            channel: None,
            peer_id: None,
            stream: false,
            thinking: None,
            attachments: vec![],
            agent_id: None,
            project_root: Some(project_root.display().to_string()),
            model_override: None,
            exec_tier: None,
            mode: None,
            memory: None,
            voice_input: false,
            project_id: None,
        };

        // chat-tier role but loopback connection → allowed.
        let result = crate::gateway::caller_identity::CALLER_ROLE
            .scope(
                Some("guest".to_string()),
                crate::gateway::caller_identity::CALLER_IS_LOOPBACK
                    .scope(true, manager.start_run(params)),
            )
            .await;
        assert!(
            result.is_ok(),
            "loopback chat-tier project_root override must succeed: {result:?}"
        );
    }

    // ------------------------------------------------------------------
    // Tier 2: the room's bound workspace (P2 Task 7)
    // ------------------------------------------------------------------

    /// A room in the process-global catalogue `bound_workspace_of` reads,
    /// optionally bound to `dir`, with `u-alice` on its roster. The returned
    /// guard serialises the roster projection — `republish_roster` REPLACES
    /// it, so a concurrent roster test would otherwise erase this room.
    ///
    /// `ProjectStore::shared()` is process-global and outlives each test, and
    /// its unique index is `(owner, workspace_path)`. Callers therefore bind a
    /// FRESH `TempDir` rather than `std::env::temp_dir()` — two tests sharing
    /// one path make whichever runs second fail on the index, and which one
    /// that is depends on the scheduler.
    fn room_in_the_shared_catalogue(
        dir: Option<&std::path::Path>,
    ) -> (String, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = crate::projects::ProjectStore::shared();
        let project = store
            .create("workspace binding room", Some("u-alice"), dir)
            .expect("create room");
        (project.id, guard)
    }

    /// The headline of Task 7: chatting in a bound room puts the run in the
    /// room's folder, with no `project_root` on the request at all.
    #[tokio::test]
    async fn a_project_session_defaults_its_cwd_to_the_bound_workspace() {
        let folder = tempfile::tempdir().expect("tempdir");
        let dir = std::fs::canonicalize(folder.path()).expect("canonical");
        let (project_id, _guard) = room_in_the_shared_catalogue(Some(&dir));

        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            project_id: Some(project_id),
            ..base_params()
        };

        let request = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                build_run_request(
                    "run-room".to_string(),
                    &session_key,
                    params,
                    None,
                    None,
                    &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                ),
            )
            .await
            .expect("build_run_request");

        assert_eq!(request.workspace_override.as_deref(), Some(dir.as_path()));
        assert_eq!(
            request.metadata.get("project_root").map(String::as_str),
            Some(dir.display().to_string().as_str()),
            "metadata must mirror the override on BOTH tiers — a reader must \
             not be able to tell which one put the run there"
        );
    }

    /// The plan's second test: the config-tier gate belongs to *choosing* a
    /// directory. A remote member running in a folder the owner already chose
    /// is not choosing one, so the gate must not fire here — otherwise every
    /// remote member of a bound room is locked out of the room's whole point.
    #[tokio::test]
    async fn a_member_does_not_need_the_config_gate_to_use_the_bound_workspace() {
        let folder = tempfile::tempdir().expect("tempdir");
        let dir = std::fs::canonicalize(folder.path()).expect("canonical");
        let (project_id, _guard) = room_in_the_shared_catalogue(Some(&dir));
        crate::projects::ProjectStore::shared()
            .add_member(&project_id, "u-bob")
            .expect("bob joins");

        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            project_id: Some(project_id),
            ..base_params()
        };

        // Remote AND chat-tier: exactly the connection tier 1 refuses.
        let request = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                crate::gateway::caller_identity::CALLER_ROLE.scope(
                    Some("member".to_string()),
                    crate::gateway::caller_identity::CALLER_IS_LOOPBACK.scope(
                        false,
                        build_run_request(
                            "run-remote-member".to_string(),
                            &session_key,
                            params,
                            None,
                            None,
                            &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                        ),
                    ),
                ),
            )
            .await
            .expect("a bound room must be usable by its remote members");

        assert_eq!(request.workspace_override.as_deref(), Some(dir.as_path()));
    }

    /// Unbound rooms and personal sessions are unchanged: the agent's own
    /// workspace, resolved downstream from `workspace_override: None`.
    #[tokio::test]
    async fn an_unbound_room_leaves_the_workspace_to_the_agent() {
        let (project_id, _guard) = room_in_the_shared_catalogue(None);

        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            project_id: Some(project_id),
            ..base_params()
        };

        let request = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                build_run_request(
                    "run-unbound".to_string(),
                    &session_key,
                    params,
                    None,
                    None,
                    &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                ),
            )
            .await
            .expect("build_run_request");

        assert!(request.workspace_override.is_none());
        assert!(!request.metadata.contains_key("project_root"));
    }

    /// A room whose folder went away refuses the turn instead of quietly
    /// running in the agent's default workspace — where every file tool would
    /// then operate somewhere nobody in the room expects.
    #[tokio::test]
    async fn a_room_whose_folder_vanished_refuses_the_turn_loudly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = std::fs::canonicalize(tmp.path()).expect("canonical");
        let (project_id, _guard) = room_in_the_shared_catalogue(Some(&dir));
        drop(tmp); // the folder is gone, the binding is not

        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            project_id: Some(project_id),
            ..base_params()
        };

        let err = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                build_run_request(
                    "run-gone".to_string(),
                    &session_key,
                    params,
                    None,
                    None,
                    &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                ),
            )
            .await
            .expect_err("a missing bound folder must not silently fall back");
        assert!(
            err.to_string().contains("folder is missing"),
            "unexpected error: {err}"
        );
    }

    /// Tier 1 still outranks tier 2, and still through the gate — a caller
    /// who names a folder for this turn gets that folder, not the room's.
    #[tokio::test]
    async fn an_explicit_project_root_still_outranks_the_binding() {
        let folder = tempfile::tempdir().expect("tempdir");
        let bound = std::fs::canonicalize(folder.path()).expect("canonical");
        let (project_id, _guard) = room_in_the_shared_catalogue(Some(&bound));
        let chosen = tempfile::tempdir().expect("tempdir");

        let session_key = AgentRouter::new().route(None, None, None, None).await;
        let params = AgentRunParams {
            project_id: Some(project_id),
            project_root: Some(chosen.path().display().to_string()),
            ..base_params()
        };

        // Config-tier: this test is about tier-1-vs-tier-2 precedence, not
        // about the `project_root` gate itself, so it needs a role the gate
        // admits (`caller_may_choose_directory` no longer treats an absent
        // role as admitted — see `caller_may_choose_directory_as`).
        let request = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                crate::gateway::caller_identity::CALLER_ROLE.scope(
                    Some("operator".to_string()),
                    build_run_request(
                        "run-explicit".to_string(),
                        &session_key,
                        params,
                        None,
                        None,
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                ),
            )
            .await
            .expect("build_run_request");

        assert_eq!(
            request.workspace_override.as_deref(),
            Some(chosen.path()),
            "the per-turn choice wins"
        );
    }

    /// Regression: aborting a run must forward the cancellation to the
    /// execution engine. Flipping only the local status is a no-op for the
    /// in-flight Think→Act loop — the engine's CancellationToken is the sole
    /// mechanism that actually stops it, so `cancel_run` MUST call
    /// `ExecutionAdapter::cancel`.
    #[tokio::test]
    async fn cancel_run_forwards_to_execution_adapter() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let adapter = Arc::new(RecordingExecutionAdapter::default());
        let execution_adapter: Arc<dyn ExecutionAdapter> = adapter.clone();
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        manager.cancel_run("run-under-test").await;

        let cancelled = adapter.cancelled.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            &*cancelled,
            &["run-under-test".to_string()],
            "a run the engine does not hold (finished, or still queued in its \
             busy lane) still goes through the run-id primitive"
        );
    }

    /// Adapter that holds one live run on a session which also owns a
    /// delegated child — the shape a leader run has after `task_manage` /
    /// delegate / background sub-agent work. Its `cancel_session` walks the
    /// child exactly as `ExecutionEngine::cancel_session` does; its `cancel`
    /// does not, exactly as `ExecutionEngine::cancel` does not.
    #[derive(Default)]
    struct DelegatingExecutionAdapter {
        cancelled: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ExecutionAdapter for DelegatingExecutionAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
            self.cancelled
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(run_id.to_string());
            Ok(())
        }

        async fn session_of_run(&self, run_id: &str) -> Option<SessionKey> {
            (run_id == "leader-run").then(|| SessionKey::main("conv-delegating"))
        }

        async fn cancel_session(
            &self,
            _session_key: &SessionKey,
        ) -> Result<Option<String>, ExecutionError> {
            self.cancel("leader-run").await?;
            self.cancel("delegated-child").await?;
            Ok(Some("leader-run".to_string()))
        }

        async fn get_status(&self, _run_id: &str) -> Option<EngineRunStatus> {
            None
        }

        async fn active_run_count(&self) -> usize {
            1
        }
    }

    /// Regression: stopping a run has to stop the work that run owns.
    ///
    /// Every run-id-addressed stop lands here — Panel `chat.abort`, TUI
    /// `/stop`, `aleph chat abort` — and it used to fire only the leader's
    /// token, leaving delegated member runs detached and still billing with no
    /// surface able to reach them. The channel `/stop` path, which always had
    /// the session key, always got the walk. Both `cancel_session`'s doc and
    /// FEATURE_LOCATOR §4.8 described `chat.abort` as taking that path; it did
    /// not, and nothing was red.
    #[tokio::test]
    async fn cancelling_a_leader_run_by_id_also_stops_its_delegated_children() {
        let router = Arc::new(AgentRouter::new());
        let event_bus = Arc::new(GatewayEventBus::new());
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let adapter = Arc::new(DelegatingExecutionAdapter::default());
        let execution_adapter: Arc<dyn ExecutionAdapter> = adapter.clone();
        let manager = AgentRunManager::new(router, event_bus, agent_registry, execution_adapter);

        assert!(manager.cancel_run("leader-run").await);

        let cancelled = adapter.cancelled.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            cancelled.contains(&"delegated-child".to_string()),
            "the child must be cancelled too, or the Stop button leaves it \
             running; got {cancelled:?}"
        );
        assert!(cancelled.contains(&"leader-run".to_string()));
    }

    /// Round-2 E3: selecting the "moa" pseudo-provider row in the picker
    /// arms sticky session MoA and is consumed (no override rides the run
    /// path); an explicit non-moa override clears the sticky instead; a bare
    /// send (no override) leaves an armed session alone.
    #[test]
    fn moa_override_arms_session_and_is_consumed() {
        use crate::gateway::model_override::ModelOverride;
        use crate::providers::moa::config_handle::moa_config_test_lock;
        let _guard = moa_config_test_lock();
        crate::providers::moa::store_moa_config(Some({
            let mut cfg = crate::config::MoaToml::default();
            cfg.presets.insert(
                "deep".to_string(),
                crate::config::MoaPreset {
                    enabled: true,
                    advisors: vec![crate::config::MoaSlot {
                        provider: "openai".to_string(),
                        model: "gpt-5".to_string(),
                    }],
                    aggregator: crate::config::MoaSlot {
                        provider: "anthropic".to_string(),
                        model: "claude-opus-4-8".to_string(),
                    },
                    fanout: crate::config::MoaFanout::default(),
                    advisor_timeout_secs: 120,
                    advisor_max_tokens: None,
                    advisor_temperature: None,
                    aggregator_temperature: None,
                },
            );
            cfg
        }));
        let key = "test:moa:selector";
        // Prime a session model pick — arming MoA must clear it (Task 15
        // selector-slot exclusivity, set-then-clear ordering).
        crate::providers::session_model_handle::set_session_model(key, None, "gpt-5".to_string());
        let out = apply_moa_selector_semantics(
            key,
            Some(ModelOverride::Qualified {
                provider: "moa".into(),
                model: "deep".into(),
            }),
        );
        assert!(out.is_none());
        let pref = crate::providers::session_moa_handle::get_session_moa(key).unwrap();
        assert_eq!(pref.preset.as_deref(), Some("deep"));
        assert!(!pref.one_shot);
        assert!(
            crate::providers::session_model_handle::get_session_model(key).is_none(),
            "arming MoA must clear the session model slot (mutual exclusion)"
        );

        // Non-moa override clears the sticky.
        let out = apply_moa_selector_semantics(
            key,
            Some(ModelOverride::Qualified {
                provider: "openai".into(),
                model: "gpt-5".into(),
            }),
        );
        assert!(out.is_some());
        assert!(crate::providers::session_moa_handle::get_session_moa(key).is_none());

        // No override leaves state alone.
        crate::providers::session_moa_handle::set_session_moa(key, None, false);
        assert!(apply_moa_selector_semantics(key, None).is_none());
        assert!(crate::providers::session_moa_handle::get_session_moa(key).is_some());
        crate::providers::session_moa_handle::clear_session_moa(key);

        crate::providers::moa::store_moa_config(None);
    }

    /// Round-3 F1: a `provider:"moa"` override naming an UNKNOWN preset must not
    /// silently arm — it validates, notifies, and leaves the session unarmed.
    #[test]
    fn moa_override_unknown_preset_does_not_arm() {
        use crate::gateway::model_override::ModelOverride;
        use crate::providers::moa::config_handle::moa_config_test_lock;
        let _guard = moa_config_test_lock();
        let key = "test:moa:selector:ghost";
        crate::providers::moa::store_moa_config(Some({
            let mut cfg = crate::config::MoaToml::default();
            cfg.presets.insert(
                "deep".to_string(),
                crate::config::MoaPreset {
                    enabled: true,
                    advisors: vec![crate::config::MoaSlot {
                        provider: "openai".to_string(),
                        model: "gpt-5".to_string(),
                    }],
                    aggregator: crate::config::MoaSlot {
                        provider: "anthropic".to_string(),
                        model: "claude-opus-4-8".to_string(),
                    },
                    fanout: crate::config::MoaFanout::default(),
                    advisor_timeout_secs: 120,
                    advisor_max_tokens: None,
                    advisor_temperature: None,
                    aggregator_temperature: None,
                },
            );
            cfg
        }));

        let out = apply_moa_selector_semantics(
            key,
            Some(ModelOverride::Qualified {
                provider: "moa".into(),
                model: "ghost".into(),
            }),
        );
        assert!(
            out.is_none(),
            "override is still consumed (not passed through)"
        );
        assert!(
            crate::providers::session_moa_handle::get_session_moa(key).is_none(),
            "unknown preset must not arm the session"
        );

        crate::providers::moa::store_moa_config(None);
    }

    // Round-2 E3 regression: a voice turn's synthesized `[voice]` low-TTFT
    // pin is config-derived, not a user picker action — it must NOT clear an
    // armed MoA sticky. Guards the intercept ordering in `start_run`
    // (explicit param first, voice-pin merge after): with the merge-first
    // ordering the synthesized pin reads as an explicit non-moa pick and
    // wipes the session's MoA activation.
    #[tokio::test]
    async fn moa_sticky_survives_voice_pin_turn() {
        use crate::gateway::model_override::ModelOverride;
        type Captured = Arc<std::sync::Mutex<Vec<Option<ModelOverride>>>>;

        struct OverrideCapturingAdapter {
            overrides: Captured,
        }
        #[async_trait]
        impl ExecutionAdapter for OverrideCapturingAdapter {
            async fn execute(
                &self,
                request: RunRequest,
                _agent: Arc<AgentInstance>,
                _emitter: Arc<dyn EventEmitter + Send + Sync>,
            ) -> Result<(), ExecutionError> {
                self.overrides
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(request.model_override.clone());
                Ok(())
            }
            async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
                Err(ExecutionError::RunNotFound(run_id.to_string()))
            }
            async fn get_status(&self, _run_id: &str) -> Option<EngineRunStatus> {
                None
            }
            async fn active_run_count(&self) -> usize {
                0
            }
        }

        // Execution is spawned (`tokio::spawn`); poll until the adapter has
        // recorded `n` RunRequests — same pattern as the platform-tag test.
        async fn wait_for_captures(captured: &Captured, n: usize) {
            for _ in 0..200 {
                if captured.lock().unwrap_or_else(|e| e.into_inner()).len() >= n {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("execution adapter did not record {n} run(s) in time");
        }

        // Distinct peer_id → a peer session key private to this test, so the
        // voice-mode registry writes here can't race the shared main-session
        // assertions in `voice_input_round_trips_through_session_mode_registry`.
        fn params(input: &str, session_key: Option<String>, voice_input: bool) -> AgentRunParams {
            AgentRunParams {
                input: input.to_string(),
                session_key,
                channel: None,
                peer_id: Some("moa-voice-test".to_string()),
                stream: false,
                thinking: None,
                attachments: vec![],
                agent_id: None,
                project_root: None,
                model_override: None,
                exec_tier: None,
                mode: None,
                memory: None,
                voice_input,
                project_id: None,
            }
        }

        let captured: Captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let adapter = Arc::new(OverrideCapturingAdapter {
            overrides: captured.clone(),
        });
        let (agent_registry, _tmp) = registry_with_main_agent().await;
        let mut config = crate::Config::default();
        config.voice_local.llm_provider = "siliconflow".to_string();
        config.voice_local.llm_model = "fast-voice-model".to_string();
        let manager = AgentRunManager::new(
            Arc::new(AgentRouter::new()),
            Arc::new(GatewayEventBus::new()),
            agent_registry,
            adapter,
        )
        .with_app_config(Arc::new(RwLock::new(config)));

        // Typed discovery run resolves the session key (no override → the
        // MoA intercept is a no-op).
        let result = manager
            .start_run(params("hello", None, false))
            .await
            .expect("typed run");
        let key = result.session_key.clone();
        wait_for_captures(&captured, 1).await;

        // Arm a sticky MoA, then send a voice turn with NO explicit override.
        crate::providers::session_moa_handle::set_session_moa(
            &key,
            Some("deep".to_string()),
            false,
        );
        manager
            .start_run(params("拉上窗帘", Some(key.clone()), true))
            .await
            .expect("voice run");
        wait_for_captures(&captured, 2).await;

        // The synthesized voice pin still rode downstream unchanged...
        let overrides = captured.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            overrides[1],
            Some(ModelOverride::Qualified {
                provider: "siliconflow".to_string(),
                model: "fast-voice-model".to_string(),
            }),
            "voice turn must still synthesize the [voice] pin"
        );
        // ...and the armed MoA sticky survived it.
        let pref = crate::providers::session_moa_handle::get_session_moa(&key)
            .expect("MoA sticky must survive a voice-pin turn");
        assert_eq!(pref.preset.as_deref(), Some("deep"));
        assert!(!pref.one_shot);
        crate::providers::session_moa_handle::clear_session_moa(&key);
    }

    /// P2 project rooms: the attribution branch in [`build_run_request`].
    mod project_scope {
        use super::*;
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::projects::roster::TEST_GUARD as ROSTER_TEST_GUARD;
        use crate::projects::ProjectStore;
        use crate::scope::{ScopeAttribution, ScopeId};
        use std::sync::MutexGuard;

        /// A room owned by alice with bob on the roster, plus a session store.
        /// The guard serialises the roster projection (it is process-global and
        /// `publish` REPLACES rather than merges).
        fn room() -> (
            Arc<dyn SessionStore>,
            String,
            tempfile::TempDir,
            MutexGuard<'static, ()>,
        ) {
            let guard = ROSTER_TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let projects = ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
            projects.create_schema().unwrap();
            let p = projects.create("room", Some("u-alice"), None).unwrap();
            projects.add_member(&p.id, "u-bob").unwrap();

            let tmp = tempfile::tempdir().unwrap();
            let store: Arc<dyn SessionStore> = Arc::new(
                FileSessionStore::new(FileSessionStoreConfig {
                    base_dir: tmp.path().join("sessions"),
                    ..FileSessionStoreConfig::default()
                })
                .unwrap(),
            );
            (store, p.id, tmp, guard)
        }

        fn params(project_id: Option<&str>) -> AgentRunParams {
            AgentRunParams {
                input: "hi".into(),
                session_key: None,
                channel: None,
                peer_id: None,
                stream: false,
                thinking: None,
                attachments: vec![],
                agent_id: None,
                project_root: None,
                model_override: None,
                exec_tier: None,
                mode: None,
                memory: None,
                voice_input: false,
                project_id: project_id.map(str::to_string),
            }
        }

        fn stamped(req: &RunRequest) -> (Option<&str>, Option<&str>) {
            (
                req.metadata
                    .get(crate::scope::OWNER_META_KEY)
                    .map(|s| s.as_str()),
                req.metadata
                    .get(crate::scope::SCOPE_META_KEY)
                    .map(|s| s.as_str()),
            )
        }

        /// A room is opened by `projects.room_session`, which claims the key
        /// but creates no session row. Whoever speaks first creates it — and if
        /// they speak WITHOUT `project_id` (the Panel always sends one, a plain
        /// RPC client need not), the row used to be stamped
        /// `personal:<that speaker>`, permanently: `stamp_attribution` is
        /// create-only, and `attribution_backfill` only heals NULL/NULL rows,
        /// not rows stamped with the wrong thing. The room then went invisible
        /// to every other member — including its owner — while `projects.list`
        /// kept listing it. Recorded as still-open by round-3's real-machine QA.
        ///
        /// The claim itself is the evidence: `claim_session_key` is the sole
        /// writer of `current_session_key`, so a key it names is a room by
        /// declaration, not by inference.
        #[tokio::test]
        async fn a_claimed_room_key_keeps_its_room_scope_without_a_project_id() {
            let (store, _pid, _tmp, _guard) = room();
            // The shared catalogue is what `resolve_attribution` consults, and
            // in tests it is a separate in-memory store from the fixture's.
            let shared = ProjectStore::shared();
            let p = shared.create("shared-room", Some("u-alice"), None).unwrap();
            shared.add_member(&p.id, "u-bob").unwrap();
            let key = SessionKey::project_room("main", &p.id);
            shared
                .claim_session_key(&p.id, &key.to_key_string())
                .unwrap();

            let req = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    build_run_request(
                        "r-room".into(),
                        &key,
                        // No `project_id` — the whole point.
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect("a member on the roster may speak in the room");

            assert_eq!(
                stamped(&req),
                (Some("u-bob"), Some(format!("project:{}", p.id).as_str())),
                "the scope is the ROOM the key was claimed for, not the first \
                 speaker's personal partition"
            );
        }

        /// The claim does not widen access: a key claimed by a room the caller
        /// is not on the roster of gets the same refusal a NAMED foreign
        /// project gets. The refusal shape must not depend on how the caller
        /// spelled it, or the two spellings become an existence oracle.
        #[tokio::test]
        async fn a_claimed_room_key_still_refuses_a_non_member() {
            let (store, _pid, _tmp, _guard) = room();
            let shared = ProjectStore::shared();
            let p = shared.create("closed-room", Some("u-alice"), None).unwrap();
            let key = SessionKey::project_room("main", &p.id);
            shared
                .claim_session_key(&p.id, &key.to_key_string())
                .unwrap();

            let err = CALLER_USER
                .scope(
                    Some("u-carol".to_string()),
                    build_run_request(
                        "r-room-2".into(),
                        &key,
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect_err("a non-member must not open the room's conversation");
            assert!(matches!(err, BuildRunError::ProjectNotFound(_)));
        }

        /// Ruling U's second `room_claiming` arm: a room bound to a channel
        /// conversation — never claimed via `projects.room_session` — must
        /// admit a roster member's very first message with no `project_id` on
        /// the request. This is the exact gap the long comment on
        /// `resolve_attribution`'s `None` arm documents: a room bound *before*
        /// anyone has spoken in it, whose first message arrives via
        /// `chat.send`/`agent.run` rather than through the bound channel.
        #[tokio::test]
        async fn a_bound_conversation_keeps_its_room_scope_without_a_project_id() {
            let (store, _pid, _tmp, _guard) = room();
            let shared = ProjectStore::shared();
            let p = shared
                .create("bound-conversation-room", Some("u-alice"), None)
                .unwrap();
            shared.add_member(&p.id, "u-bob").unwrap();
            shared
                .bind_conversation(
                    &p.id,
                    "telegram",
                    aleph_protocol::projects::BindingPeerKind::Group,
                    "C-admission",
                    Some("u-alice"),
                    None,
                )
                .unwrap();
            let key = SessionKey::group(
                "main",
                "telegram",
                crate::routing::session_key::PeerKind::Group,
                "C-admission",
            );

            let req = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    build_run_request(
                        "r-bound".into(),
                        &key,
                        // No `project_id` — the whole point.
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect("a member on the roster may speak in the bound conversation");

            assert_eq!(
                stamped(&req),
                (Some("u-bob"), Some(format!("project:{}", p.id).as_str())),
                "a conversation bound before anyone spoke must take the room scope \
                 on its first message, even though nothing ever claimed the session \
                 key itself"
            );
        }

        /// Arm 2 does **not** copy arm 1's refusal, and the asymmetry is the
        /// whole point of this pair.
        ///
        /// A non-member reaching a bound channel conversation through
        /// admission is admitted PERSONALLY — exactly the run they would get
        /// if nothing had ever been bound to that conversation. The same
        /// principal, in the same Telegram group, speaking through the
        /// CHANNEL, gets precisely that already: the inbound router stamps
        /// `personal:<speaker>` and `run_loop::request_scope`'s roster gate
        /// leaves it alone. Refusing here answered one question two different
        /// ways depending on which door the caller came through — and answered
        /// it by naming a project id the caller never typed, on a room they
        /// cannot see.
        ///
        /// So the oracle argument runs the other way for arm 2 than it does
        /// for arm 1. Arm 1 must refuse identically however the room was
        /// spelled ([`a_claimed_room_key_still_refuses_a_non_member`]); arm 2
        /// must be *silent*, which it is only by falling through — a bound
        /// conversation and an unbound one then produce byte-identical runs
        /// for a caller the roster does not admit. That is asserted below
        /// against a genuinely unbound key, not merely described.
        ///
        /// The refusal for an explicitly NAMED foreign project is unchanged
        /// and is re-asserted here, because it is the thing arm 2's fall-
        /// through must not be mistaken for having relaxed.
        #[tokio::test]
        async fn a_bound_conversation_admits_a_non_member_personally() {
            let (store, _pid, _tmp, _guard) = room();
            let shared = ProjectStore::shared();
            let p = shared
                .create("closed-bound-room", Some("u-alice"), None)
                .unwrap();
            shared
                .bind_conversation(
                    &p.id,
                    "telegram",
                    aleph_protocol::projects::BindingPeerKind::Group,
                    "C-admission-2",
                    Some("u-alice"),
                    None,
                )
                .unwrap();
            let key = SessionKey::group(
                "main",
                "telegram",
                crate::routing::session_key::PeerKind::Group,
                "C-admission-2",
            );

            let admitted = CALLER_USER
                .scope(
                    Some("u-carol".to_string()),
                    build_run_request(
                        "r-bound-2".into(),
                        &key,
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect(
                    "a non-member reaching a bound conversation must get the same \
                     personal run the channel door already gives them, not a refusal",
                );
            assert_eq!(
                stamped(&admitted),
                (Some("u-carol"), Some("personal:u-carol")),
                "the room the caller cannot see must not govern this turn — and must \
                 not refuse it either"
            );

            // The silence, asserted rather than described: a conversation
            // NOTHING is bound to produces the identical stamp.
            let unbound_key = SessionKey::group(
                "main",
                "telegram",
                crate::routing::session_key::PeerKind::Group,
                "C-not-bound-at-all",
            );
            let baseline = CALLER_USER
                .scope(
                    Some("u-carol".to_string()),
                    build_run_request(
                        "r-bound-2b".into(),
                        &unbound_key,
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect("an unbound conversation is an ordinary personal turn");
            assert_eq!(
                stamped(&admitted),
                stamped(&baseline),
                "for a caller the roster does not admit, a bound conversation must be \
                 indistinguishable from an unbound one: the old refusal disclosed both \
                 that this conversation is bound AND the id of the room it is bound to"
            );

            // Unchanged: a NAMED project the caller cannot reach is still one
            // refusal with no oracle. Reusing the same bound key on purpose —
            // an explicit `project_id` short-circuits `resolve_attribution`'s
            // Path 2 before the claim lookup runs at all, so this pins that
            // arm 2's fall-through did not relax the named case.
            let unknown = CALLER_USER
                .scope(
                    Some("u-carol".to_string()),
                    build_run_request(
                        "r-bound-3".into(),
                        &key,
                        params(Some("p-never-minted")),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect_err("an unminted id must be refused");
            assert!(
                matches!(unknown, BuildRunError::ProjectNotFound(ref id) if id == "p-never-minted")
            );
            let foreign = CALLER_USER
                .scope(
                    Some("u-carol".to_string()),
                    build_run_request(
                        "r-bound-4".into(),
                        &key,
                        params(Some(&p.id)),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect_err("a named room the caller is not on must still be refused");
            assert!(matches!(foreign, BuildRunError::ProjectNotFound(ref id) if *id == p.id));

            // Building a request creates no session row either way.
            assert!(store.get_metadata(&key).await.unwrap().is_none());
        }

        /// The asymmetry stated as one claim, so a later reader cannot "fix"
        /// arm 1 and arm 2 into agreement by symmetry without a named test
        /// going red.
        ///
        /// Same principal, same non-membership, two keys — and two different
        /// answers, on purpose. Arm 1's key IS the room's own conversation, so
        /// falling through would stamp the room's own row
        /// `personal:<first speaker>` permanently (`stamp_attribution` is
        /// create-only; `attribution_backfill` heals only NULL/NULL rows).
        /// Arm 2's key belongs to a channel conversation whose real traffic
        /// arrives through a door that stamps it personally anyway, so
        /// refusing buys nothing and costs an existence oracle.
        #[tokio::test]
        async fn the_two_claim_arms_are_gated_differently_for_the_same_non_member() {
            let (store, _pid, _tmp, _guard) = room();
            let shared = ProjectStore::shared();

            let claimed_room = shared.create("arm1-room", Some("u-alice"), None).unwrap();
            let claimed_key = SessionKey::project_room("main", &claimed_room.id);
            shared
                .claim_session_key(&claimed_room.id, &claimed_key.to_key_string())
                .unwrap();

            let bound_room = shared.create("arm2-room", Some("u-alice"), None).unwrap();
            shared
                .bind_conversation(
                    &bound_room.id,
                    "telegram",
                    aleph_protocol::projects::BindingPeerKind::Group,
                    "C-asymmetry",
                    Some("u-alice"),
                    None,
                )
                .unwrap();
            let bound_key = SessionKey::group(
                "main",
                "telegram",
                crate::routing::session_key::PeerKind::Group,
                "C-asymmetry",
            );

            let arm1 = CALLER_USER
                .scope(
                    Some("u-carol".to_string()),
                    build_run_request(
                        "r-arm1".into(),
                        &claimed_key,
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await;
            let arm2 = CALLER_USER
                .scope(
                    Some("u-carol".to_string()),
                    build_run_request(
                        "r-arm2".into(),
                        &bound_key,
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await;

            assert!(
                matches!(arm1, Err(BuildRunError::ProjectNotFound(ref id)) if *id == claimed_room.id),
                "arm 1 is a declaration about THIS key: refusing is what keeps the \
                 room's own row from being stamped personal forever"
            );
            let arm2 = arm2.expect("arm 2 must not refuse");
            assert_eq!(
                stamped(&arm2),
                (Some("u-carol"), Some("personal:u-carol")),
                "arm 2 is an inference from a conversation, and the same principal in \
                 the same conversation gets exactly this through the channel door"
            );
        }

        #[tokio::test]
        async fn a_project_session_is_stamped_with_the_project_scope_not_the_creator() {
            let (store, pid, _tmp, _guard) = room();
            let key = SessionKey::Main {
                agent_id: "main".into(),
                main_key: "new-1".into(),
                epoch: 0,
            };

            let req = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    build_run_request(
                        "r1".into(),
                        &key,
                        params(Some(&pid)),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect("a member may open a room session");

            assert_eq!(
                stamped(&req),
                (Some("u-alice"), Some(format!("project:{pid}").as_str())),
                "owner is the author; scope is the ROOM, not the author"
            );
        }

        #[tokio::test]
        async fn a_non_member_cannot_open_a_session_in_a_project() {
            let (store, pid, _tmp, _guard) = room();
            let key = SessionKey::Main {
                agent_id: "main".into(),
                main_key: "new-2".into(),
                epoch: 0,
            };

            let denied = CALLER_USER
                .scope(
                    Some("u-mallory".to_string()),
                    build_run_request(
                        "r2".into(),
                        &key,
                        params(Some(&pid)),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect_err("a stranger must be refused");
            let unknown = CALLER_USER
                .scope(
                    Some("u-mallory".to_string()),
                    build_run_request(
                        "r3".into(),
                        &key,
                        params(Some("p-never-minted")),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect_err("an unminted id must be refused");

            // No oracle: both are ProjectNotFound, differing only in the id the
            // caller supplied and already knows.
            assert!(matches!(denied, BuildRunError::ProjectNotFound(ref id) if *id == pid));
            assert!(
                matches!(unknown, BuildRunError::ProjectNotFound(ref id) if id == "p-never-minted")
            );
            // And the refusal happened BEFORE anything created a session row.
            assert!(store.get_metadata(&key).await.unwrap().is_none());
        }

        #[tokio::test]
        async fn an_existing_sessions_scope_wins_over_the_request() {
            let (store, pid, _tmp, _guard) = room();
            let key = SessionKey::Main {
                agent_id: "main".into(),
                main_key: "personal-1".into(),
                epoch: 0,
            };

            // alice opens a PERSONAL session first.
            crate::scope::with_scope(
                Some(ScopeAttribution::personal("u-alice")),
                store.get_or_create(&key),
            )
            .await
            .unwrap();

            // ...then re-sends to it claiming it is a project session.
            let req = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    build_run_request(
                        "r4".into(),
                        &key,
                        params(Some(&pid)),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect("re-sending to your own session is fine");

            assert_eq!(
                stamped(&req),
                (Some("u-alice"), Some("personal:u-alice")),
                "spec §10: a session's scope is immutable — the request does not move it"
            );
            // The row on disk is untouched too.
            let meta = store.get_metadata(&key).await.unwrap().unwrap();
            assert_eq!(meta.scope_id.as_deref(), Some("personal:u-alice"));
        }

        #[tokio::test]
        async fn a_second_member_may_speak_in_the_room() {
            let (store, pid, _tmp, _guard) = room();
            let key = SessionKey::Main {
                agent_id: "main".into(),
                main_key: "room-1".into(),
                epoch: 0,
            };

            // alice creates the room session.
            crate::scope::with_scope(
                Some(ScopeAttribution {
                    owner_user_id: "u-alice".into(),
                    scope: ScopeId::Project(pid.clone()),
                }),
                store.get_or_create(&key),
            )
            .await
            .unwrap();

            // bob — a member, not the creator — sends into it.
            let req = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    build_run_request(
                        "r5".into(),
                        &key,
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect("a member may speak in the room");

            assert_eq!(
                stamped(&req),
                (Some("u-alice"), Some(format!("project:{pid}").as_str())),
                "the run inherits the ROOM's attribution, so bob's memory writes \
                 land in the room's partition, not his own"
            );
        }

        /// The Simulated-execution carve-out, stated as a test rather than only
        /// as a doc comment: with no store there is no stored scope to honor,
        /// so every turn takes the new-session path.
        #[tokio::test]
        async fn without_a_session_store_every_turn_takes_the_new_session_path() {
            let (_store, pid, _tmp, _guard) = room();
            let key = SessionKey::Main {
                agent_id: "main".into(),
                main_key: "no-store".into(),
                epoch: 0,
            };

            let req = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    build_run_request(
                        "r6".into(),
                        &key,
                        params(Some(&pid)),
                        None,
                        None,
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect("no store still resolves the request's own claim");
            assert_eq!(
                stamped(&req),
                (Some("u-alice"), Some(format!("project:{pid}").as_str()))
            );
        }

        /// An unrestricted caller (cron, A2A, in-process) that names a room must
        /// still get the project stamp — deriving the owner from the caller
        /// alone would silently drop the scope for the callers that named it
        /// most explicitly.
        #[tokio::test]
        async fn an_unrestricted_caller_naming_a_room_still_gets_the_project_scope() {
            let (store, pid, _tmp, _guard) = room();
            let key = SessionKey::Main {
                agent_id: "main".into(),
                main_key: "room-2".into(),
                epoch: 0,
            };

            let req = build_run_request(
                "r7".into(),
                &key,
                params(Some(&pid)),
                None,
                Some(&store),
                &crate::gateway::agent_instance::AgentInstanceConfig::default(),
            )
            .await
            .expect("unrestricted callers are never refused");
            assert_eq!(
                stamped(&req),
                (Some("u-owner"), Some(format!("project:{pid}").as_str())),
                "owner by absence, project scope honored"
            );
        }

        /// No `project_id` at all keeps the pre-P2 personal stamp verbatim.
        #[tokio::test]
        async fn a_turn_without_a_project_id_is_unchanged_from_p1() {
            let (store, _pid, _tmp, _guard) = room();
            let key = SessionKey::Main {
                agent_id: "main".into(),
                main_key: "plain-1".into(),
                epoch: 0,
            };

            let req = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    build_run_request(
                        "r8".into(),
                        &key,
                        params(None),
                        None,
                        Some(&store),
                        &crate::gateway::agent_instance::AgentInstanceConfig::default(),
                    ),
                )
                .await
                .expect("ordinary turn");
            assert_eq!(stamped(&req), (Some("u-alice"), Some("personal:u-alice")));
        }
    }

    /// The agent axis of run-start authorization (§5.17 round 5).
    ///
    /// `chat.send` / `agent.run` let the caller name the agent to run as, and
    /// `tool_permissions` is keyed on exactly that name — so until
    /// `allowed_users` existed, a caller denied a tool under one agent could
    /// name another on the wire and inherit its allowances. The session check
    /// already here does not cover it and structurally cannot: naming a
    /// different agent mints a BRAND-NEW session key, which
    /// `existing_session_is_visible` admits by design.
    ///
    /// Effect tests through the real shared builder, under the task-local a
    /// real dispatch applies.
    mod agent_admission {
        use super::*;
        use crate::gateway::agent_instance::AgentInstanceConfig;
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::gateway::router::AgentRouter;

        fn turn() -> AgentRunParams {
            AgentRunParams {
                input: "do the thing".to_string(),
                session_key: None,
                channel: None,
                peer_id: None,
                stream: true,
                thinking: None,
                attachments: vec![],
                agent_id: None,
                project_root: None,
                model_override: None,
                exec_tier: None,
                mode: None,
                memory: None,
                voice_input: false,
                project_id: None,
            }
        }

        fn agent_admitting(users: Option<&[&str]>) -> AgentInstanceConfig {
            AgentInstanceConfig {
                agent_id: "ops".to_string(),
                allowed_users: users
                    .map(|u| u.iter().map(|s| (*s).to_string()).collect::<Vec<_>>()),
                ..AgentInstanceConfig::default()
            }
        }

        /// The defect this round closes, stated as a test: a caller who is not
        /// on the agent's list cannot start a run as it, even though the
        /// session key is brand new and therefore passes every
        /// session-visibility check there is.
        #[tokio::test]
        async fn a_caller_not_on_the_list_cannot_run_as_that_agent() {
            let key = AgentRouter::new()
                .route(None, None, None, Some("ops"))
                .await;
            let err = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    build_run_request(
                        "run-x".to_string(),
                        &key,
                        turn(),
                        None,
                        None,
                        &agent_admitting(Some(&["u-alice"])),
                    ),
                )
                .await
                .expect_err("an unlisted caller must not borrow this agent's authority");
            assert!(
                matches!(err, BuildRunError::AgentForbidden(ref id) if id == "ops"),
                "expected AgentForbidden, got {err:?}"
            );
        }

        #[tokio::test]
        async fn a_caller_on_the_list_may() {
            let key = AgentRouter::new()
                .route(None, None, None, Some("ops"))
                .await;
            CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    build_run_request(
                        "run-y".to_string(),
                        &key,
                        turn(),
                        None,
                        None,
                        &agent_admitting(Some(&["u-alice"])),
                    ),
                )
                .await
                .expect("a listed caller must be admitted");
        }

        /// The zero-change guarantee at the gate, not merely in the predicate:
        /// an agent that names nobody stays reachable by anyone — which is
        /// every agent in every config written before this field existed.
        #[tokio::test]
        async fn an_agent_that_names_nobody_is_reachable_by_anyone() {
            let key = AgentRouter::new()
                .route(None, None, None, Some("ops"))
                .await;
            CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    build_run_request(
                        "run-z".to_string(),
                        &key,
                        turn(),
                        None,
                        None,
                        &agent_admitting(None),
                    ),
                )
                .await
                .expect("an unrestricted agent must stay reachable");
        }

        /// A refused turn must leave no side effect behind. `build_run_request`
        /// writes the per-session voice pin near its top, so a gate placed
        /// after it would let a refused turn pin voice mode for a key its
        /// caller was just told they may not use. Asserting on the registry
        /// rather than on statement order is what keeps this true through a
        /// reshuffle of the function body.
        #[tokio::test]
        async fn a_refused_turn_leaves_no_side_effect_behind() {
            let key = AgentRouter::new()
                .route(None, None, None, Some("ops"))
                .await;
            let mut params = turn();
            params.voice_input = true;
            let key_str = key.to_key_string();
            crate::gateway::voice::voice_mode::set(&key_str, None);

            let _ = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    build_run_request(
                        "run-w".to_string(),
                        &key,
                        params,
                        None,
                        None,
                        &agent_admitting(Some(&["u-alice"])),
                    ),
                )
                .await
                .expect_err("refused");

            assert!(
                crate::gateway::voice::voice_mode::get(&key_str).is_none(),
                "a refused turn must not have pinned voice mode for this session"
            );
        }
    }
}
