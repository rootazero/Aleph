//! Chat Handlers
//!
//! High-level chat control handlers that wrap agent operations with
//! simpler semantics for chat-focused clients.
//!
//! Methods:
//! - `chat.send` - Send a message (wraps agent.run)
//! - `chat.abort` - Abort message generation
//! - `chat.history` - Get chat history
//! - `chat.clear` - Clear chat history
//! - `chat.rewind` - Drop the conversation from a given event seq onward
//!   (server half of edit-and-resend; the client then calls `chat.send`)

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::debug;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::super::router::SessionKey;
use super::super::session_store::SessionStore;
use super::super::visibility;
use super::agent::{AgentRunManager, AgentRunParams, Attachment};
use super::parse_params;

// ============================================================================
// Request/Response Types
// ============================================================================

/// Parameters for chat.send request
#[derive(Debug, Clone, Deserialize)]
pub struct SendParams {
    /// User message content
    pub message: String,
    /// Optional session key (auto-generated if not provided)
    #[serde(default)]
    pub session_key: Option<String>,
    /// Channel identifier (e.g., "gui:window1", "cli:term1")
    #[serde(default)]
    pub channel: Option<String>,
    /// Whether to stream events (default: true)
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// Thinking level for LLM reasoning depth
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
    /// Used by the desktop Panel's "Enter Project" flow to scope a chat to
    /// a user-picked folder. Must be an absolute path; relative paths and
    /// non-existent directories are rejected by the gateway handler.
    #[serde(default)]
    pub project_root: Option<String>,
    /// Per-turn model override sent by the chat-window model picker.
    /// When `None`, the gateway falls back to the agent's configured
    /// model + its fallback chain. See
    /// [`crate::gateway::model_override::ModelOverride`].
    #[serde(default)]
    pub model_override: Option<crate::gateway::model_override::ModelOverride>,
    /// Execution tier picked in the composer. Carried on the message because a
    /// brand-new conversation has no session to write it to yet — see
    /// [`AgentRunParams::exec_tier`].
    #[serde(default)]
    pub exec_tier: Option<String>,
    /// Session usage mode (chat / work / code) picked in the composer. Same
    /// first-message carriage as `exec_tier` — see [`AgentRunParams::mode`].
    #[serde(default)]
    pub mode: Option<String>,
    /// Per-session memory mode (`"on"` / `"off"`). Same first-message carriage
    /// as `exec_tier` / `mode` — see [`AgentRunParams::memory`].
    #[serde(default)]
    pub memory: Option<String>,
    /// True when this message is an ASR-transcribed spoken utterance (the
    /// Panel voice loop). Forwarded to [`AgentRunParams::voice_input`] so the
    /// session gets the voice-mode prompt layer and the `[voice]` model pin.
    #[serde(default)]
    pub voice_input: bool,
    /// Open this conversation in a project room (P2). Only consulted when the
    /// session does not exist yet — a session's scope is immutable. See
    /// [`AgentRunParams::project_id`].
    #[serde(default)]
    pub project_id: Option<String>,
}

const fn default_stream() -> bool {
    true
}

/// Response for chat.send request
#[derive(Debug, Clone, Serialize)]
pub struct SendResponse {
    /// Unique run identifier
    pub run_id: String,
    /// Resolved session key
    pub session_key: String,
    /// Whether streaming is enabled
    pub streaming: bool,
}

/// Parameters for chat.abort request
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AbortParams {
    /// Run ID to abort
    pub run_id: String,
    /// Session whose waiting backlog should be abandoned along with the run.
    ///
    /// Optional so older clients keep working, but a client that omits it stops
    /// one run and leaves the lane loaded: cancelling frees the session slot,
    /// the lane wakes its front waiter, and the messages the user just said they
    /// did not want start firing one full agent run at a time. `/stop` has
    /// purged the lane since Round-5; Panel and CLI were given the same lane in
    /// the same round but no way to reach `purge`.
    #[serde(default)]
    pub session_key: Option<String>,
}

/// Parameters for chat.history request
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HistoryParams {
    /// Session key to get history for
    pub session_key: String,
    /// Maximum number of messages to return
    #[serde(default)]
    pub limit: Option<usize>,
    /// Cursor for pagination: return only messages strictly older than this
    /// timestamp. Accepts an RFC 3339 / ISO 8601 string or a bare Unix-seconds
    /// integer. Pass the oldest timestamp of the previous page to fetch the
    /// next-older page. Unparseable values are treated as "no cursor".
    #[serde(default)]
    pub before: Option<String>,
}

/// Parse the `chat.history` `before` cursor into the instant it names.
///
/// Accepts an RFC 3339 timestamp — the spelling this very endpoint serves, so
/// the natural client move of echoing back the oldest row's `timestamp` works
/// — or a bare integer, which is resolved by the SAME seconds/milliseconds
/// boundary the stored rows are. That second half matters because the store
/// writes both units (see [`MessageRecord::timestamp`]); reading a bare number
/// as seconds unconditionally would put a millisecond cursor in the year
/// 58536, i.e. "everything is older than this".
///
/// An instant, not a number, because the value is about to be ranked against a
/// mixed-unit column and there is no unit a bare `i64` could carry that would
/// be right for both halves of it.
///
/// Returns `None` for empty / unparseable input so a malformed cursor degrades
/// to an un-paginated (most-recent) fetch rather than erroring the request.
///
/// [`MessageRecord::timestamp`]: crate::gateway::session_store::types::MessageRecord::timestamp
fn parse_before(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return chrono::DateTime::from_timestamp_millis(
            crate::gateway::session_store::types::stamp_millis(n),
        );
    }
    chrono::DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Parameters for chat.clear request
#[derive(Debug, Clone, Deserialize)]
pub struct ClearParams {
    /// Session key to clear
    pub session_key: String,
}

/// Parameters for chat.rewind request
#[derive(Debug, Clone, Deserialize)]
pub struct RewindParams {
    /// Session key to rewind
    pub session_key: String,
    /// First event seq to retire — INCLUSIVE. Everything with `seq >= this`
    /// leaves the live conversation (event log) and its projected rows are
    /// deleted from the transcript; everything with `seq < this` survives
    /// untouched, as do transcript rows that were never projected from an event
    /// (boot-time orphan notices, legacy rows).
    ///
    /// The caller passes the seq of the message it wants to REPLACE, taken from
    /// the id that message already carries: the projector writes row ids as
    /// `"{session_key}:{seq}"` (see
    /// [`crate::session::projection::parse_source_seq`]). To edit-and-resend a
    /// user message, rewind at that message's seq — it and everything after it
    /// disappear — then `chat.send` the new text. `chat.rewind` never appends
    /// the replacement itself.
    ///
    /// Must be >= 1 (the first append lands at seq 1).
    pub seq: u64,
}

/// Params for chat.context_estimate.
#[derive(Debug, serde::Deserialize)]
pub struct EstimateParams {
    pub session_key: String,
}

/// A chat message in the history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp (ISO 8601)
    pub timestamp: String,
    /// Optional run ID that generated this message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Last-turn context-window occupancy (provider-reported prompt tokens),
    /// persisted on assistant turns so the Panel gauge re-projects on reload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u32>,
    /// The model's authoritative context window for that turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Run-cumulative token total (gauge tooltip).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Who typed this, in a multi-human project room (spec §6.2). `None` in
    /// every single-author session, which is why the Panel must treat absence
    /// as "the session's own user" rather than as "unknown".
    ///
    /// The id, not a name: the Panel resolves names through `users.list`, so a
    /// rename is reflected in history instead of frozen into it — the same
    /// reason the event stores the id and `thinker::nudges::speaker_label`
    /// resolves at render time for the model's half.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_user_id: Option<String>,
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Handle chat.send RPC request
///
/// Sends a message and starts agent execution. This is a high-level wrapper
/// around `agent.run` with simpler semantics for chat-focused clients.
pub async fn handle_send(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    // Parse params
    let params: SendParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Validate message
    if params.message.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Message cannot be empty");
    }

    // Convert to AgentRunParams
    let agent_params = AgentRunParams {
        input: params.message,
        session_key: params.session_key,
        channel: params.channel,
        peer_id: None,
        stream: params.stream,
        thinking: params.thinking,
        attachments: params.attachments,
        agent_id: params.agent_id,
        project_root: params.project_root,
        model_override: params.model_override,
        exec_tier: params.exec_tier,
        mode: params.mode,
        memory: params.memory,
        voice_input: params.voice_input,
        project_id: params.project_id,
    };

    // Start the run
    match run_manager.start_run(agent_params).await {
        Ok(result) => {
            let response = SendResponse {
                run_id: result.run_id,
                session_key: result.session_key,
                streaming: params.stream,
            };
            JsonRpcResponse::success(request.id, json!(response))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

/// Drop every queued message belonging to one conversation: the addressed
/// session and, when this key has one, its derived `/btw` side lane.
///
/// # Why both, and why this is a named function
///
/// A `/btw` side question is ticketed on a lane DERIVED from the addressed key
/// (`busy_queue::register_run` → `btw::execution_session`), so purging the
/// addressed key alone leaves a queued side question in a lane that key can
/// never reach. `cancel_run` → `cancel_session` then cancels the *running*
/// side run, and that release wakes the survivor into a full LLM turn AFTER
/// the user pressed Stop — while the receipt says `dropped: 0`. To the person
/// pressing Stop there is one conversation, and
/// `inbound_router::command_handler::handle_stop` (the channel `/stop` face)
/// has purged both lanes all along; `handle_abort` copied that block's
/// ordering rule and not its both-lanes rule.
///
/// It takes a parsed [`SessionKey`], never the client's raw string: tickets
/// are registered under `SessionKey::to_key_string()`, so any accepted form
/// that does not round-trip byte-for-byte would purge a lane nothing ever
/// registered on and still answer success. `side_session_of` returns `None`
/// for an already-derived key, so this cannot mint a phantom lane.
///
/// It is a function rather than an inline expression so the guard test can
/// exercise the real composition — an inline expression can only be *restated*
/// by a test, which then passes whatever `handle_abort` does.
fn purge_conversation_lanes(session: &SessionKey) -> usize {
    crate::gateway::busy_queue::purge(&session.to_key_string())
        + crate::gateway::btw::side_session_of(session).map_or(0, |side| {
            crate::gateway::busy_queue::purge(&side.to_key_string())
        })
}

/// Handle chat.abort RPC request
///
/// Aborts an in-progress message generation.
pub async fn handle_abort(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
    session_store: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    // Parse params
    let params: AbortParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Visibility gate: an addressed `session_key` must belong to the caller
    // before we touch anything. `session_key` is optional (legacy clients
    // omit it), but when PRESENT it is used below to purge that session's
    // busy-queue backlog directly by string — a caller-controlled key with
    // real mutating effect, not just a hint. A malformed key that fails to
    // parse falls through unchanged: it can never match a real busy-queue
    // entry either, because every real entry is registered under a
    // canonical `SessionKey::to_key_string()` — a string that FAILS to
    // parse can therefore never equal one that succeeded. This is
    // load-bearing, not just a comment: see
    // `visibility_guards::a_canonical_session_key_always_round_trips_so_a_malformed_one_cannot_collide`.
    let parsed_session_key = match params.session_key.as_deref() {
        Some(key_str) => SessionKey::from_key_string(key_str),
        None => None,
    };
    if let Some(ref session_key) = parsed_session_key {
        let meta = match session_store.get_metadata(session_key).await {
            Ok(Some(m)) => m,
            Ok(None) => return visibility::not_found_response(request.id),
            Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
        };
        if !visibility::session_visible(&meta) {
            return visibility::not_found_response(request.id); // same error as missing (GC 4)
        }
    }

    // …and the run id needs its own gate, because the block above is inside
    // `if let Some(ref key_str)`. `session_key` is `#[serde(default)]
    // Option<String>`, so OMITTING it skipped every check and went straight to
    // `cancel_run` — the guard was real, and reaching it was optional. A
    // conditional gate on an optional field gates only the callers who chose to
    // fill it in.
    if !crate::gateway::handlers::agent::caller_may_address_run(
        &params.run_id,
        &run_manager,
        session_store.as_ref(),
    )
    .await
    {
        return visibility::not_found_response(request.id);
    }

    // Drop the backlog before cancelling, never after: cancelling releases the
    // session slot, which wakes the lane's front waiter, which can be admitted
    // (and so leave the lane) before a later purge could mark it. Same ordering
    // rule as `/stop` in `inbound_router::command_handler::handle_stop`.
    //
    // Both lanes and the parsed key: see `purge_conversation_lanes`.
    let dropped = parsed_session_key
        .as_ref()
        .map_or(0, purge_conversation_lanes);

    // Cancel the run
    let cancelled = run_manager.cancel_run(&params.run_id).await;

    debug!(
        run_id = %params.run_id,
        cancelled = cancelled,
        dropped = dropped,
        "Chat abort requested"
    );

    JsonRpcResponse::success(
        request.id,
        json!({
            "run_id": params.run_id,
            "aborted": cancelled,
            "dropped": dropped,
        }),
    )
}

/// Handle chat.history RPC request
///
/// Returns the chat history for a session, plus three facts about that
/// session's *current* state that the transcript alone cannot express:
/// `active_run` (the turn in flight right now, or `null`),
/// `active_run_elapsed_ms` (how long that turn has been going, or `null`) and
/// `plan` (the durable execution list, or `null`).
///
/// # Why the age is a sibling field and not part of `active_run`
///
/// `active_run` is `string | null` on the wire and two clients read it that
/// way — the TUI's three-way parse keys off its PRESENCE (absent means an
/// older core, which is a different answer from "nothing running"), the Panel
/// off its value. Widening it into an object would have broken both to add one
/// number. It is also a DURATION rather than a start stamp: a client cannot
/// subtract a timestamp without first answering "whose clock", and across
/// machines that answer is not free.
///
/// # Why `plan` rides here too
///
/// The scratchpad execution list is durable — a markdown file that the model,
/// the `<execution_plan>` prompt layer and the stop verifier all read as the
/// source of truth. The Panel's Todo strip did **not** read it. Its only
/// sources were live `tool_call_completed` frames, the terminal `RunSummary`,
/// and its own in-memory per-tab snapshot; a client that attached fresh
/// reconstructed the list by *replaying* the persisted trace. That replay is a
/// weaker thing than it looks:
///
/// * `agent_trace` is a deliberately lossy mirror (bounded `mpsc` + `try_send`,
///   drop on full) — which is exactly why `RunSummary.plan` exists as the live
///   path's reconciliation — and the replay path has no such reconciliation at
///   all;
/// * it only covers assistant rows inside the fetched window, so a plan older
///   than that window, or one owned by an explicitly-named `project_id` shared
///   across conversations, is simply not in the events being replayed;
/// * a run in flight has not written its trace yet, so the case this response's
///   `active_run` field was added for — join a conversation mid-turn — is
///   precisely the case with no plan to replay.
///
/// So: refresh the page, open a second tab, attach from a second device, or
/// come back tomorrow, and a half-finished checklist that the model is still
/// being held to renders as nothing at all. The durable file is the fact; this
/// field is it, and it is the last word over anything replay produced.
///
/// # Why both ride on this response instead of their own RPCs
///
/// Opening (or re-opening) a conversation that is *already running* used to
/// hand a client a transcript with no way to learn that more was coming: the
/// `stream.*` frames for that run carry a `run_id` the joiner never saw
/// accepted, so they route to nothing, and the sidebar's re-hydrate is
/// suppressed for exactly as long as the session is running. The joiner sat in
/// front of a frozen transcript for the whole turn — the "two terminals share
/// one thread" case failing silently on the second terminal.
///
/// Both belong on this response rather than beside it because the three facts
/// are one snapshot. A separate call would open a window where a client holds
/// the transcript but not the run (or the reverse), and every window of that
/// kind eventually renders a turn twice or not at all; the same is true of a
/// checklist that arrives a beat after the transcript it annotates. They also
/// cost nothing extra to authorize: this handler has already resolved the
/// session's metadata and passed `visibility::session_visible` before it asks,
/// so both inherit that gate instead of needing one each — no new method to
/// register in `method_visibility`, no new entry in `lane::override_for`, no
/// second existence oracle.
///
/// `run_manager` is `Option` because `chat.history` is registered
/// unconditionally while the run manager is not (`common_handlers.rs`); absent,
/// the field is `null`, which reads as "no live turn to join" — the same
/// honest degradation `ExecutionAdapter::active_run_for_session` makes.
pub async fn handle_history(
    request: JsonRpcRequest,
    session_manager: Arc<dyn SessionStore>,
    run_manager: Option<Arc<crate::gateway::handlers::agent::AgentRunManager>>,
) -> JsonRpcResponse {
    // Parse params
    let params: HistoryParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Parse session key
    let session_key = match SessionKey::from_key_string(&params.session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let meta = match session_manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        return visibility::not_found_response(request.id); // same error as missing (GC 4)
    }

    // Resolve the optional pagination cursor (degrades to None when absent
    // or unparseable, yielding the most-recent window).
    let before_ts = params.before.as_deref().and_then(parse_before);

    // ONE call for both answers. This used to be two — a `history_len` and
    // then the window — against a store a live run appends to, so the count
    // and the transcript described two different sessions and the client's
    // `total - received` was wrong by whatever landed in between. It was
    // managed with a comment arguing which ORDER made the skew fall the safer
    // way plus a source-level guard pinning that order, and a guard on
    // statement order is satisfiable lexically while being broken semantically
    // (move the count into a helper, call the helper after the window). There
    // is no order here to get wrong, and `SessionStore` no longer offers a
    // second call to reach for.
    match session_manager
        .history_page(&session_key, params.limit, before_ts)
        .await
    {
        Ok(page) => {
            let total = page.total;
            let chat_messages: Vec<ChatMessage> = page
                .rows
                .into_iter()
                .map(|m| {
                    // Resolved before anything moves out of `m`: the accessor
                    // needs the whole record (the stored unit is ambiguous —
                    // see `MessageRecord::timestamp`).
                    let timestamp = m.rfc3339();
                    // Occupancy was persisted as string-valued metadata (see
                    // `agent_instance::build_message_metadata`), so read every
                    // field as a string and parse the numeric ones back.
                    let meta = m.metadata;
                    let field = |k: &str| meta.as_ref().and_then(|mt| mt.get(k))?.as_str();
                    ChatMessage {
                        role: m.role,
                        content: m.content,
                        timestamp,
                        run_id: field("run_id").map(String::from),
                        context_tokens: field("context_tokens").and_then(|s| s.parse().ok()),
                        context_window: field("context_window").and_then(|s| s.parse().ok()),
                        total_tokens: field("total_tokens").and_then(|s| s.parse().ok()),
                        author_user_id: field("author_user_id").map(String::from),
                    }
                })
                .collect();

            let count = chat_messages.len();
            // Resolved AFTER the visibility gate above, so it is scoped by the
            // same decision that let the transcript out; a caller who cannot
            // see the session never reaches this line.
            //
            // Looked up by the CANONICAL key (`session_key.to_key_string()`),
            // not by `params.session_key`: the registry is keyed by what
            // `SessionKey::to_key_string` produces, so a caller whose spelling
            // parses but is not byte-identical to canonical would silently get
            // "nothing running" — a miss that looks exactly like an idle
            // session.
            // Same canonical key, same post-gate position, same reason (see the
            // rationale above). Resolved through the one "session → plan"
            // function the prompt layer and the stop verifier also call, so a
            // client and the model can never be told different lists.
            let canonical = session_key.to_key_string();
            let active_run = run_manager
                .as_ref()
                .and_then(|rm| rm.active_run_for_session(&canonical));
            // How long that run has been going, measured on the server's
            // monotonic clock at this instant. A duration and not a start
            // stamp: a client cannot subtract a timestamp without first
            // answering "whose clock", and across machines that answer is not
            // free. Without it a client joining mid-turn can only count from
            // its own arrival, which understates a turn that has been running
            // for minutes — the working indicator says 3s on a run in its
            // fourth minute, and nothing on screen says the number is a
            // floor.
            //
            // Two lookups, so the run can end between them; that answers
            // `None`, which the client reads as "count from now" and is the
            // behaviour it had before this field existed. The reverse would
            // need the registry to carry timing it does not have.
            let active_run_elapsed_ms = match (run_manager.as_ref(), active_run.as_deref()) {
                (Some(rm), Some(id)) => rm.run_elapsed_ms(id).await,
                _ => None,
            };
            let plan = crate::builtin_tools::scratchpad::session_plan_snapshot(&canonical).await;
            // The conversation's own settings — usage mode, exec tier, thinking
            // depth, memory mode, model pin, cumulative tokens, working folder.
            // Same argument as `active_run` and `plan` above, applied to the
            // facts a client's status bar and pills render: they are one
            // snapshot with the transcript, they were already resolved (this is
            // the very `meta` the visibility gate read), and a separate call
            // would open a window in which a client shows a conversation while
            // describing a different one's settings.
            //
            // Before this, no keyed surface reported them at all. A terminal
            // reopened mid-task therefore painted the *install* defaults over a
            // conversation the run loop was still governing by its own stored
            // values — the tier pill under-reporting the gate that was live,
            // the token counter restarting at zero, the model caption naming
            // the default the session had overridden.
            let session_snapshot = crate::gateway::session_snapshot::snapshot_from_metadata(&meta);
            // The lane's waiting messages, by the SAME canonical key and at
            // the same post-gate position as `active_run` and `plan` above,
            // and for the same reason: they are one snapshot with the
            // transcript. A client that attached mid-wait never saw the
            // `RunQueued` frames — they fired before its socket existed — so
            // without this it paints "thinking" over a queue it cannot see.
            let pending = crate::gateway::busy_queue::pending_for(&canonical);
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": params.session_key,
                    "messages": chat_messages,
                    "count": count,
                    // Rows in the whole session; `count` is how many of them
                    // this window carries. `null` = a core that predates the
                    // field, or a count that could not be read — both mean
                    // "no answer", not "nothing above".
                    "total": total,
                    "active_run": active_run,
                    "pending": pending,
                    // A SIBLING of `active_run`, not a field inside it. That
                    // key is `string | null` on the wire and two clients read
                    // it that way — the TUI's three-way parse keys off its
                    // PRESENCE, the Panel off its value — so widening it into
                    // an object would break both to add one number.
                    "active_run_elapsed_ms": active_run_elapsed_ms,
                    "plan": plan,
                    "session": session_snapshot,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get history: {e}"),
        ),
    }
}

/// Handle chat.clear RPC request
///
/// Clears the chat history for a session.
pub async fn handle_clear(
    request: JsonRpcRequest,
    session_manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    // Parse params
    let params: ClearParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Parse session key
    let session_key = match SessionKey::from_key_string(&params.session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let meta = match session_manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        return visibility::not_found_response(request.id); // same error as missing (GC 4)
    }

    debug!(session_key = %params.session_key, "Clearing chat history");

    // Retire the event log BEFORE the projection. `reset_session` only empties
    // the `messages` table the Panel reads; the model replays `session_events`,
    // so clearing the projection alone would blank the screen while the model
    // still remembers every word. Doing the SSOT first means a later failure
    // leaves recoverable ghost rows on screen rather than a conversation the
    // model secretly still holds.
    let retired = match crate::session::store::retire_live_events(&session_key, 1).await {
        Ok(n) => n,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to clear session event log: {e}"),
            );
        }
    };

    // Clearing the conversation must also clear the `/btw` side session it
    // derives. Not for residue — the key is unchanged, so the side session is
    // still perfectly addressable — but for content: `btw::seed` copies a
    // prefix of THIS transcript into the side session's own event log, so a
    // clear that spares it leaves the user's next `/btw` able to quote back the
    // conversation they just wiped, from inside the conversation they wiped it
    // from. Deliberately not `terminate_session_continuations`: the key stays
    // reachable here, so a running loop/goal is still stoppable and clearing
    // content is no reason to kill it.
    crate::gateway::continuation_lifecycle::retire_side_session(
        &session_key,
        "chat.clear",
        Some(session_manager.clone()),
    );

    match session_manager.reset_session(&session_key).await {
        Ok(cleared) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": params.session_key,
                "cleared": cleared || retired > 0,
                "events_retired": retired,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to clear history: {e}"),
        ),
    }
}

/// Handle chat.rewind RPC request.
///
/// Retires every event from `seq` onward, so the live conversation ends just
/// before it. This is the server half of edit-and-resend: the client rewinds to
/// the message it wants to change, then sends the new text through `chat.send`
/// as it would any other message. Rewinding does not run a turn and does not
/// append the replacement — `chat.send` already owns that path, and appending
/// here too would double-write the user message.
pub async fn handle_rewind(
    request: JsonRpcRequest,
    session_manager: Arc<dyn SessionStore>,
    run_manager: Option<Arc<crate::gateway::handlers::agent::AgentRunManager>>,
) -> JsonRpcResponse {
    let params: RewindParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let session_key = match SessionKey::from_key_string(&params.session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    // Seq is 1-based (the first append lands at 1); 0 would be a client bug
    // asking to rewind past the start of the log.
    if params.seq == 0 {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "seq must be >= 1");
    }

    let meta = match session_manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        return visibility::not_found_response(request.id); // same error as missing (GC 4)
    }

    debug!(session_key = %params.session_key, seq = params.seq, "Rewinding chat");

    let retired = match crate::session::store::retire_live_events(&session_key, params.seq).await {
        Ok(n) => n,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to rewind session event log: {e}"),
            );
        }
    };

    // A rewind that cut away a `RunFinished` and left its `RunStarted` behind
    // does not corrupt anything — it makes the log SAY a run is still open, and
    // the boot scan believes it: every later boot re-classifies this session
    // `Interrupted`, appends a crash-boundary repair and re-triggers a run the
    // user deleted, forever, because nothing else ever closes that marker.
    super::balance_run_markers_after_retire(&session_key, run_manager.as_ref()).await;

    // Realign the Panel's projection with the shortened log by deleting the
    // rows the retired events produced — matched by their source seq, never by
    // row count: `messages` is not a 1:1 image of the live event log (orphan
    // notices and other writers append rows with no source event), so a
    // count-derived ordinal cuts the wrong range.
    match session_manager
        .delete_messages_from_seq(&session_key, params.seq)
        .await
    {
        Ok(removed) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": params.session_key,
                "events_retired": retired,
                "messages_removed": removed,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to truncate chat history: {e}"),
        ),
    }
}

/// Handle chat.context_estimate RPC.
///
/// Returns an estimated next-prompt occupancy for sessions that never ran an
/// LLM turn (so the gauge can show `≈N%`). `null` when core can't resolve the
/// session/model — the panel then keeps the gauge hidden (graceful, P7).
///
/// # P1 visibility (KeyChecked)
///
/// This was recorded as "a token-count-only read" and deferred; it is more
/// than that. `used_tokens` is derived from the addressed session's whole
/// event log and `window_tokens` from the model that session is PINNED to
/// (`session_model_handle::get_session_model`), so an ungated caller learns
/// which model another user runs and roughly how much they have said to it.
///
/// The gate is [`visibility::existing_session_is_visible`], not the
/// addressed-key `session_visible` pattern: a session that does not exist yet
/// is the ordinary "the composer is open on a fresh conversation" case the
/// Panel calls this on, and it must keep flowing through to a real estimate
/// rather than becoming a denial.
///
/// A denial reuses this method's OWN `null` — the shape it already returns
/// when core cannot resolve the key at all — rather than an error code that
/// would exist only for denials. The residual, stated rather than implied: a
/// well-formed key that answers `null` tells the caller "not resolvable BY
/// YOU", which is weaker than the `not_found_response` contract elsewhere in
/// this file but is the strongest shape available without fabricating an
/// estimate. It is byte-identical to the malformed-key answer.
pub async fn handle_context_estimate(
    request: JsonRpcRequest,
    harness: Arc<dyn crate::orchestrator::dispatch::HarnessRunner>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: EstimateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let unresolved = |id| JsonRpcResponse::success(id, serde_json::Value::Null);

    // A key core cannot even parse has always answered `null` (the estimator
    // parses it again and gives up); keep that answer here so the visibility
    // gate does not become the thing that changes it.
    let Some(session_key) = SessionKey::from_key_string(&params.session_key) else {
        return unresolved(request.id);
    };
    if !visibility::existing_session_is_visible(sessions.as_ref(), &session_key).await {
        return unresolved(request.id);
    }

    match harness.estimate_context(&params.session_key).await {
        Some(est) => JsonRpcResponse::success(
            request.id,
            json!({
                "used_tokens": est.used_tokens,
                "window_tokens": est.window_tokens,
            }),
        ),
        None => unresolved(request.id),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `handle_history` answers both of its questions — "here is a window" and
    /// "here is how long the whole conversation is" — with ONE store call.
    ///
    /// It used to make two, and two reads of a store a live run is appending to
    /// describe two different sessions: the client's `total - received` came
    /// out wrong by whatever landed in between. That was managed by arguing
    /// which ORDER made the skew fall the safer way, pinned by a guard that
    /// asserted the count appeared earlier in this function than the window
    /// did. The hole in it is that statement order is not the property: moving
    /// the count into a helper and calling the helper after the window
    /// satisfies the guard and breaks the thing it was protecting.
    ///
    /// So the property asserted here is arity, not order. One read has no order
    /// to get wrong, and `SessionStore` deliberately exposes no second method
    /// (`history_len` is gone) for a caller holding a trait object to reach
    /// for — this is what remains to catch someone reintroducing one on the
    /// concrete type or via a second `get_history`.
    ///
    /// `assert_eq!(reads, 1)` is its own self-check: it fails at zero (the
    /// scanner stopped finding the call and would otherwise pass vacuously) as
    /// well as at two.
    ///
    /// `\r` is stripped first: this repo is checked out CRLF on Windows, and a
    /// scanner that anchors on `\n` finds nothing there while staying green.
    #[test]
    fn the_window_and_the_transcript_length_come_from_one_read() {
        let src = include_str!("chat.rs").replace('\r', "");
        let start = src
            .find("pub async fn handle_history(")
            .expect("handle_history moved; this guard no longer scans it");
        // End at the function's closing brace in column 0 — the syntactic end
        // of the unit being scanned, not a line count, so an edit inside the
        // body cannot slide the window off it.
        let body_end = src[start..]
            .find("\n}\n")
            .expect("handle_history has no column-0 closing brace");
        // Comment lines are dropped before anything is matched: a scanner
        // judges CODE, and the prose inside this very function names both
        // `history_len` and `history_page` while explaining why there is now
        // one read. (Caught by this guard failing on its own first run.)
        let body: String = src[start..start + body_end]
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let body = body.as_str();

        let reads = body.matches(".history_page(").count();
        assert_eq!(
            reads, 1,
            "handle_history makes {reads} calls to `history_page`, not 1. The \
             window and the transcript's length have to come from the same \
             read: they are two answers about a session a live run is \
             appending to, and two reads answer about two different sessions."
        );

        for second_read in ["history_len", "history_total", ".get_history("] {
            assert!(
                !body.contains(second_read),
                "handle_history reads the transcript a second time via \
                 `{second_read}`. Everything it needs comes back from the one \
                 `history_page` call; a second read reintroduces the skew \
                 between the count and the window that this shape removed."
            );
        }
    }

    /// End-to-end over the handler: `chat.history` must report how long the
    /// whole transcript is, not just how long the slice it served is.
    ///
    /// Asserted through `handle_history` rather than on `history_len` alone,
    /// because the failure this guards against is not a wrong count — it is
    /// the field quietly not reaching the wire, which no store-level test can
    /// see. A client that receives no `total` falls back to guessing from the
    /// page length, and that guess cannot tell a truncated transcript from a
    /// complete one of exactly `limit` rows.
    #[tokio::test]
    async fn history_reports_the_whole_transcript_alongside_the_window_it_serves() {
        use crate::gateway::router::SessionKey;
        use crate::gateway::session_store::file_backend::{
            FileSessionStore, FileSessionStoreConfig,
        };
        use crate::gateway::session_store::types::MessageRecord;
        use crate::sync_primitives::Arc;

        let temp = tempfile::TempDir::new().unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        );
        let key = SessionKey::main("history-total");
        store.get_or_create(&key).await.unwrap();
        for ts in 1..=5i64 {
            store
                .append_message(
                    &key,
                    MessageRecord {
                        id: format!("m{ts}"),
                        role: "user".into(),
                        content: format!("message {ts}"),
                        timestamp: ts,
                        metadata: None,
                        input_tokens: 0,
                        output_tokens: 0,
                        tool_call_id: None,
                        tool_name: None,
                    },
                )
                .await
                .unwrap();
        }

        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "chat.history".into(),
            params: Some(json!({
                "session_key": key.to_key_string(),
                "limit": 2,
            })),
            id: Some(json!(1)),
        };
        let response = handle_history(request, store, None).await;
        let result = response.result.expect("history succeeds");

        assert_eq!(
            result["count"], 2,
            "the window is the limit the caller asked for"
        );
        assert_eq!(
            result["total"], 5,
            "and `total` is the session, not the window — this is the field a \
             client needs to know its transcript has a beginning it was not sent"
        );
        let messages = result["messages"].as_array().expect("messages array");
        assert_eq!(
            messages[0]["content"], "message 4",
            "the window kept is the TRAILING one, which is why the beginning \
             is the part that goes missing"
        );

        // The reconciliation the clients cannot perform on themselves.
        // `interfaces/cli` and `interfaces/tui` may not depend on `alephcore`,
        // so their copy of these keys has nothing to disagree with — this
        // crate depends on both sides and is the only place the shared
        // contract can be held against a REAL response rather than against a
        // literal written next to the assertion.
        let window: aleph_protocol::session_thread::HistoryWindow =
            serde_json::from_value(result.clone()).expect(
                "the response must deserialize into the shape every thin \
                 client reads it with; a rename here is this test going red, \
                 not a CLI column quietly describing something else",
            );
        assert_eq!(window.count, 2);
        assert_eq!(window.total, Some(5));
        assert_eq!(
            window.above(),
            Some(3),
            "three rows the caller was not sent — the number `aleph chat \
             history --limit 2` has to print instead of calling the window the \
             total"
        );
        assert_eq!(window.is_complete(), Some(false));
    }

    /// The wire contract Panel Stop depends on. An older client that sends only
    /// `run_id` must still parse (the field is optional), and a client that
    /// scopes the stop must have its session key actually arrive — this is the
    /// half that reaches `busy_queue::purge`.
    #[test]
    fn abort_params_carry_the_session_to_purge_and_stay_backward_compatible() {
        let scoped: AbortParams =
            serde_json::from_value(json!({"run_id": "run-1", "session_key": "agent:main:main"}))
                .unwrap();
        assert_eq!(scoped.session_key.as_deref(), Some("agent:main:main"));

        let legacy: AbortParams = serde_json::from_value(json!({"run_id": "run-1"})).unwrap();
        assert!(legacy.session_key.is_none());
    }

    /// Stop must empty the lane, not just cancel the run. Asserts the effect at
    /// the consumer — the tickets are marked, so `deliver_with_ticket` bails
    /// before attempting — rather than that `purge` was called.
    #[test]
    fn a_session_scoped_abort_abandons_everything_waiting_on_that_session() {
        use crate::gateway::busy_queue;
        let session = "abort-test:purges-the-lane";
        let waiting =
            busy_queue::register(session, 8, "queued-1").expect("lane has room for the first");
        let behind =
            busy_queue::register(session, 8, "queued-2").expect("lane has room for the second");

        let dropped = busy_queue::purge(session);

        assert_eq!(dropped, 2, "both waiting messages are reported to the user");
        assert!(
            waiting.is_cancelled() && behind.is_cancelled(),
            "a purged ticket must read as cancelled to its own waiter, which is \
             what stops it being delivered once the cancel frees the slot"
        );
    }

    /// Stop means the whole conversation, including the `/btw` side lane.
    ///
    /// The test above could not see this: it registers both tickets on ONE
    /// invented key, so "purges the conversation" and "purges one key" are
    /// indistinguishable to it. Here the second ticket is registered on the
    /// key `busy_queue::register_run` actually derives for a side question —
    /// the lane the addressed key cannot reach.
    ///
    /// Asserted at the consumer (the ticket reads cancelled to its own
    /// waiter), because that is what stops it being delivered when
    /// `cancel_session` frees the slot.
    #[test]
    fn an_abort_drops_the_side_question_lane_too_not_just_the_addressed_one() {
        use crate::gateway::{btw, busy_queue};

        let main =
            SessionKey::from_key_string("agent:btw-abort-test:main").expect("canonical key parses");
        let side = btw::side_session_of(&main).expect("a main key has a side lane");
        assert_ne!(
            side.to_key_string(),
            main.to_key_string(),
            "precondition: the side lane really is a different key, or this \
             test would pass for the wrong reason"
        );

        let on_main = busy_queue::register(&main.to_key_string(), 8, "queued-main")
            .expect("main lane has room");
        let on_side = busy_queue::register(&side.to_key_string(), 8, "queued-btw")
            .expect("side lane has room");

        // The production function `handle_abort` calls — NOT a restatement of
        // it. Restating the expression here would make this test pass for
        // whatever `handle_abort` happens to do; calling it makes reverting
        // the fix turn this test red.
        let dropped = super::purge_conversation_lanes(&main);

        assert_eq!(
            dropped, 2,
            "both lanes must be reported; a receipt of 1 (or 0) tells the user \
             nothing was queued while a side question is still waiting"
        );
        assert!(
            on_main.is_cancelled(),
            "the addressed lane must still be purged"
        );
        assert!(
            on_side.is_cancelled(),
            "the /btw side lane survived Stop: cancel_session will cancel the \
             running side run, and that release wakes this ticket into a full \
             turn after the user pressed Stop"
        );
    }

    /// The purge must be keyed on the parsed `SessionKey`, not on the raw
    /// string the client sent: tickets are registered under
    /// `to_key_string()`, so any accepted form that does not round-trip
    /// byte-for-byte would purge an empty lane and still answer success.
    #[test]
    fn the_abort_purge_is_keyed_on_the_canonical_form_of_the_session_key() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/gateway/handlers/chat.rs"
        ))
        .expect("this file is readable from its own test");
        let production = crate::utils::source_scan::production_prefix(&src);
        let stripped = crate::utils::source_scan::strip_comment_lines(&production);
        // Whitespace removed, because rustfmt decides where this expression
        // wraps and that decision is not the property under test. A guard
        // pinned to the incidental line breaks goes red on a reformat while
        // the behaviour it names is untouched — and a reader then learns to
        // ignore it.
        let code: String = stripped.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            code.contains("parsed_session_key.as_ref().map_or(0,purge_conversation_lanes)"),
            "handle_abort must purge from the parsed SessionKey via \
             `purge_conversation_lanes`; purging `params.session_key` directly \
             re-derives the lane key from the client's string instead of from \
             the form tickets are stored under"
        );
        assert!(
            code.contains("btw::side_session_of(session)"),
            "purge_conversation_lanes must reach the /btw side lane, the way \
             `inbound_router::command_handler::handle_stop` does"
        );
        assert!(
            !code.contains("map_or(0,crate::gateway::busy_queue::purge)"),
            "handle_abort must not purge the client's raw session_key string: \
             that is the single-lane form this replaced"
        );
    }

    #[test]
    fn test_send_params_deserialization() {
        let json = json!({
            "message": "Hello, world!",
            "session_key": "agent:main:main",
            "stream": true
        });

        let params: SendParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.message, "Hello, world!");
        assert_eq!(params.session_key, Some("agent:main:main".to_string()));
        assert!(params.stream);
        assert!(params.thinking.is_none());
    }

    #[test]
    fn test_send_params_defaults() {
        let json = json!({
            "message": "Test"
        });

        let params: SendParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.message, "Test");
        assert!(params.session_key.is_none());
        assert!(params.channel.is_none());
        assert!(params.stream); // default true
        assert!(params.thinking.is_none());
    }

    #[test]
    fn test_send_response_serialization() {
        let response = SendResponse {
            run_id: "run-123".to_string(),
            session_key: "agent:main:main".to_string(),
            streaming: true,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["run_id"], "run-123");
        assert_eq!(json["session_key"], "agent:main:main");
        assert_eq!(json["streaming"], true);
    }

    #[test]
    fn test_abort_params_deserialization() {
        let json = json!({
            "run_id": "run-456"
        });

        let params: AbortParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.run_id, "run-456");
    }

    #[test]
    fn test_history_params_deserialization() {
        let json = json!({
            "session_key": "agent:main:main",
            "limit": 50,
            "before": "2024-01-01T00:00:00Z"
        });

        let params: HistoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_key, "agent:main:main");
        assert_eq!(params.limit, Some(50));
        assert_eq!(params.before, Some("2024-01-01T00:00:00Z".to_string()));
    }

    /// Every spelling a client could plausibly send has to name the SAME
    /// instant — including a bare millisecond number, because half the rows on
    /// a real install are stamped that way and echoing one back is the obvious
    /// thing for a script to do. Read as seconds, `1609459200000` is the year
    /// 52975, i.e. a cursor that admits the entire transcript.
    #[test]
    fn every_cursor_spelling_names_the_same_instant() {
        // 2021-01-01T00:00:00Z == 1609459200 s == 1609459200000 ms.
        let expected = chrono::DateTime::from_timestamp(1_609_459_200, 0);
        assert_eq!(parse_before("1609459200"), expected);
        assert_eq!(parse_before("2021-01-01T00:00:00Z"), expected);
        assert_eq!(parse_before("1609459200000"), expected);
        // The endpoint serves RFC 3339 with an offset; echoing that back must
        // land on the same instant, not on a local-time reading of it.
        assert_eq!(parse_before("2021-01-01T08:00:00+08:00"), expected);
        // Surrounding whitespace is tolerated.
        assert_eq!(parse_before("  1609459200  "), expected);
    }

    #[test]
    fn test_parse_before_rejects_garbage_and_empty() {
        assert_eq!(parse_before(""), None);
        assert_eq!(parse_before("   "), None);
        assert_eq!(parse_before("not-a-timestamp"), None);
    }

    #[test]
    fn test_history_params_minimal() {
        let json = json!({
            "session_key": "agent:main:main"
        });

        let params: HistoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_key, "agent:main:main");
        assert!(params.limit.is_none());
        assert!(params.before.is_none());
    }

    #[test]
    fn test_clear_params_deserialization() {
        let json = json!({
            "session_key": "agent:main:main"
        });

        let params: ClearParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_key, "agent:main:main");
    }

    #[test]
    fn test_rewind_params_deserialization() {
        let json = json!({
            "session_key": "agent:main:main",
            "seq": 7
        });

        let params: RewindParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_key, "agent:main:main");
        assert_eq!(params.seq, 7);
    }

    #[test]
    fn test_chat_message_serialization() {
        let message = ChatMessage {
            role: "assistant".to_string(),
            content: "Hello!".to_string(),
            timestamp: "2024-01-01T12:00:00Z".to_string(),
            run_id: Some("run-789".to_string()),
            context_tokens: Some(42_000),
            context_window: Some(200_000),
            total_tokens: Some(55_000),
            author_user_id: None,
        };

        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["content"], "Hello!");
        assert_eq!(json["timestamp"], "2024-01-01T12:00:00Z");
        assert_eq!(json["run_id"], "run-789");
        assert_eq!(json["context_tokens"], 42_000);
        assert_eq!(json["context_window"], 200_000);
        assert_eq!(json["total_tokens"], 55_000);
    }

    #[test]
    fn test_chat_message_without_run_id() {
        let message = ChatMessage {
            role: "user".to_string(),
            content: "Hi".to_string(),
            timestamp: "2024-01-01T12:00:00Z".to_string(),
            run_id: None,
            context_tokens: None,
            context_window: None,
            total_tokens: None,
            author_user_id: None,
        };

        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["role"], "user");
        // Absent occupancy + run_id must be omitted, not serialized as null.
        let obj = json.as_object().unwrap();
        assert!(!obj.contains_key("run_id"));
        assert!(!obj.contains_key("context_tokens"));
        assert!(!obj.contains_key("context_window"));
        assert!(!obj.contains_key("total_tokens"));
    }

    #[test]
    fn history_metadata_string_occupancy_parses_into_typed_fields() {
        // Mirrors the persisted shape: occupancy stored as STRING metadata
        // (HashMap<String,String>-safe). `handle_history` parses them back.
        let meta = json!({
            "run_id": "r1",
            "context_tokens": "42000",
            "context_window": "200000",
            "total_tokens": "55000",
        });
        let field = |k: &str| meta.get(k).and_then(|v| v.as_str());
        assert_eq!(field("run_id").map(String::from), Some("r1".to_string()));
        assert_eq!(
            field("context_tokens").and_then(|s| s.parse::<u32>().ok()),
            Some(42_000)
        );
        assert_eq!(
            field("context_window").and_then(|s| s.parse::<u32>().ok()),
            Some(200_000)
        );
        assert_eq!(
            field("total_tokens").and_then(|s| s.parse::<u64>().ok()),
            Some(55_000)
        );
    }

    #[test]
    fn estimate_params_parses_session_key() {
        let v = serde_json::json!({ "session_key": "main:agentA" });
        let p: super::EstimateParams = serde_json::from_value(v).unwrap();
        assert_eq!(p.session_key, "main:agentA");
    }

    /// P1 visibility chokepoint — pinned per task-6-brief.md Step 1.
    /// `chat.history` / `chat.clear` / `chat.rewind` are not literally named
    /// in the brief's file list (only `chat.send`/`chat.abort` are), but they
    /// are session-addressed handlers in this same file, already carrying a
    /// `SessionStore`, with the identical unguarded pattern — leaving them
    /// open would make the `sessions.history` fix moot (same content,
    /// reachable through a sibling method). Guarded here as an in-scope
    /// extension of the same chokepoint, not a separate task.
    mod visibility_guards {
        use super::*;
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::gateway::protocol::RESOURCE_NOT_FOUND;
        use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
        use crate::scope::{with_scope, ScopeAttribution};

        fn store(temp: &tempfile::TempDir) -> Arc<dyn SessionStore> {
            Arc::new(
                SessionManager::new(SessionManagerConfig {
                    db_path: temp.path().join("chat_visibility.db"),
                    ..Default::default()
                })
                .unwrap(),
            )
        }

        async fn alice_session(store: &Arc<dyn SessionStore>) -> SessionKey {
            let key = SessionKey::from_key_string("agent:alicechatvis:main").unwrap();
            with_scope(
                Some(ScopeAttribution::personal("u-alice")),
                store.get_or_create(&key),
            )
            .await
            .unwrap();
            key
        }

        fn request(method: &str, params: serde_json::Value) -> JsonRpcRequest {
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: method.into(),
                params: Some(params),
                id: Some(json!(1)),
            }
        }

        /// `chat.abort` with a foreign `session_key`: denied, and the
        /// caller-controlled busy-queue purge that key would otherwise drive
        /// must not fire (bob learns nothing and nothing of alice's is
        /// touched).
        #[tokio::test]
        async fn chat_abort_denies_a_foreign_session_key() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let waiting = crate::gateway::busy_queue::register(&alice_key_str, 8, "queued-1")
                .expect("lane has room");

            let router = Arc::new(crate::gateway::router::AgentRouter::new());
            let event_bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
            let agent_registry = Arc::new(crate::gateway::agent_instance::AgentRegistry::new());
            let execution_adapter: Arc<dyn crate::gateway::ExecutionAdapter> = Arc::new(
                crate::gateway::execution_engine::SimpleExecutionEngine::new(
                    crate::gateway::execution_engine::ExecutionEngineConfig::default(),
                ),
            );
            let run_manager = Arc::new(AgentRunManager::new(
                router,
                event_bus,
                agent_registry,
                execution_adapter,
            ));

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_abort(
                        request(
                            "chat.abort",
                            json!({ "run_id": "irrelevant", "session_key": alice_key_str }),
                        ),
                        run_manager,
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
            assert!(
                !waiting.is_cancelled(),
                "a denied abort must not purge the foreign session's queue"
            );
        }

        /// End-to-end coverage for `chat.history`'s `"pending"` key: drives
        /// the real `handle_history`, not just `busy_queue::pending_for` in
        /// isolation, so a future edit swapping `&canonical` for
        /// `params.session_key` (or dropping the field) would fail here even
        /// though `pending_for` itself stayed correct.
        ///
        /// Addresses the session by a differently-cased-but-equivalent
        /// spelling, on purpose: `normalize_agent_id` folds it to the same
        /// `SessionKey`, so the session lookup still succeeds either way, but
        /// only the CANONICAL string is what `busy_queue::register` (and
        /// production's `register_run`) key the lane on — a raw,
        /// non-canonical `params.session_key` would look up a lane that was
        /// never registered under that exact spelling and silently report no
        /// one waiting, which is the bug this test exists to catch (see the
        /// call site's own comment).
        ///
        /// Uses its own session key rather than `alice_session`'s shared
        /// literal: `busy_queue` is a process-global lane keyed by that same
        /// string, and `chat_abort_denies_a_foreign_session_key` above also
        /// registers a ticket on it — tests run in parallel, so sharing the
        /// key would make this test's own lane depth depend on whether that
        /// other test's `TicketGuard` has dropped yet.
        #[tokio::test]
        async fn chat_history_reports_the_sessions_pending_lane() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let key = SessionKey::main("HistoryPendingTest");
            store.get_or_create(&key).await.unwrap();
            let canonical = key.to_key_string();
            assert_eq!(
                canonical, "agent:historypendingtest:main",
                "test premise: agent_id normalizes to lowercase"
            );

            let _waiting = crate::gateway::busy_queue::register(&canonical, 8, "queued-1")
                .expect("lane has room");

            let response = handle_history(
                request(
                    "chat.history",
                    json!({ "session_key": "agent:HistoryPendingTest:main" }),
                ),
                store.clone(),
                None,
            )
            .await;

            let pending = response
                .result
                .as_ref()
                .and_then(|r| r.get("pending"))
                .unwrap_or_else(|| {
                    panic!("chat.history response carried no \"pending\" key: {response:?}")
                });
            assert_eq!(
                pending,
                &json!([{ "run_id": "queued-1", "ahead": 0 }]),
                "the session's own waiting lane must be reported verbatim, even \
                 when addressed by a non-canonical spelling of the same session"
            );
        }

        /// Pins the invariant `handle_abort`'s comment relies on for its
        /// malformed-`session_key` fallthrough: a malformed key can only
        /// ever collide with a real `busy_queue` entry if SOME canonical
        /// `to_key_string()` output could also fail to parse back — round
        /// trip it for a representative sample of every `SessionKey`
        /// variant shape and confirm none do. This is the guard that
        /// upgrades the assumption from "a comment" to "load-bearing and
        /// checked".
        #[test]
        fn a_canonical_session_key_always_round_trips_so_a_malformed_one_cannot_collide() {
            for key in [
                SessionKey::main("roundtrip-main"),
                SessionKey::peer("roundtrip-peer", "window-1"),
                SessionKey::task("roundtrip-task", "cron", "job-1"),
            ] {
                let canonical = key.to_key_string();
                assert!(
                    SessionKey::from_key_string(&canonical).is_some(),
                    "every canonical session key string must parse back — \
                     `{canonical}` did not, which would let a malformed \
                     caller-supplied key collide with a real busy_queue entry"
                );
            }
            // And the converse half of the same invariant: a string that
            // fails to parse is exactly the shape `handle_abort` lets fall
            // through unguarded — confirm a garbage string really does fail,
            // so that fallthrough branch is provably reachable and provably
            // safe, not just assumed.
            assert!(SessionKey::from_key_string("not a real session key").is_none());
        }

        #[tokio::test]
        async fn chat_history_denies_cross_user_as_not_found() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_history(
                        request("chat.history", json!({ "session_key": alice_key_str })),
                        store.clone(),
                        None,
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
            assert!(
                as_bob.result.is_none(),
                "a denied history must not leak the live-turn pointer either — \
                 `active_run` is resolved after this gate, and a refusal that \
                 carried a result would be answering a question it just refused"
            );
        }

        /// The settings snapshot is part of this response's contract for the
        /// same reason `active_run` and `plan` are: they are one snapshot with
        /// the transcript, and a client that had to fetch them separately would
        /// spend a window rendering a conversation while describing a different
        /// one's mode, tier and token count.
        ///
        /// This is the wire that makes reopening a terminal mid-task land you
        /// back where you were. Before it, no keyed surface reported a session's
        /// own settings at all: `sessions.list` carried three of them (and not
        /// `think_level`), and nothing carried them for a client attaching by
        /// key — so a reopened client painted the install defaults over a
        /// conversation the run loop was still governing by its stored values.
        #[tokio::test]
        async fn history_carries_the_sessions_own_settings() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;

            // Arm every knob the way a real conversation would have.
            let patch = crate::gateway::session_store::types::SessionPatch {
                metadata: Some(json!({
                    "exec_tier": "ask",
                    "session_mode": "code",
                    "think_level": "high",
                    "memory_mode": "off",
                })),
                ..Default::default()
            };
            store.patch_session(&alice_key, &patch).await.unwrap();

            let resp = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_history(
                        request(
                            "chat.history",
                            json!({ "session_key": alice_key.to_key_string() }),
                        ),
                        store.clone(),
                        None,
                    ),
                )
                .await;
            let result = resp.result.expect("alice may read her own history");
            let session = result.get("session").expect(
                "the settings snapshot must always be emitted — absent reads to a client \
                         as an old core, and it then shows the install defaults",
            );

            // Parsed through the shared contract type, not poked at with string
            // keys: this is the type the TUI deserializes, so a field renamed on
            // one side has to fail here rather than in a terminal.
            let snapshot: aleph_protocol::SessionSnapshot =
                serde_json::from_value(session.clone()).expect("snapshot parses as the contract");

            assert_eq!(snapshot.session_key, alice_key.to_key_string());
            assert_eq!(snapshot.exec_tier.as_deref(), Some("ask"));
            assert_eq!(snapshot.mode.as_deref(), Some("code"));
            assert_eq!(
                snapshot.think_level.as_deref(),
                Some("high"),
                "the twin that reached no client surface until now"
            );
            assert_eq!(snapshot.memory_mode.as_deref(), Some("off"));
        }

        /// An unset knob is reported as absent, never as a value. The client
        /// renders absent as "follows the global default"; a concrete value
        /// here would be the server inventing one, and the two are
        /// indistinguishable downstream.
        #[tokio::test]
        async fn an_unconfigured_session_reports_no_overrides() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;

            let resp = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_history(
                        request(
                            "chat.history",
                            json!({ "session_key": alice_key.to_key_string() }),
                        ),
                        store.clone(),
                        None,
                    ),
                )
                .await;
            let result = resp.result.expect("alice may read her own history");
            let snapshot: aleph_protocol::SessionSnapshot =
                serde_json::from_value(result["session"].clone()).expect("snapshot parses");
            assert_eq!(snapshot.exec_tier, None);
            assert_eq!(snapshot.mode, None);
            assert_eq!(snapshot.think_level, None);
            assert_eq!(snapshot.memory_mode, None);
            assert_eq!(snapshot.model_pin, None);
        }

        /// The live-turn pointer is part of this response's contract: the Panel
        /// reads `active_run` to decide whether a session it has just opened is
        /// mid-turn and should be joined. Present-and-null is the answer for
        /// "nothing running" AND for a build with no run manager wired; both
        /// mean "no live turn to join". A missing key would be indistinguishable
        /// from an old core to the client's parser, so pin that it is emitted.
        #[tokio::test]
        async fn history_always_carries_the_live_turn_pointer() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            // The scratchpad registry is a process-global; a sibling test in
            // this module may have bound the same canonical key and never
            // cleared it (or, more subtly, the static is keyed by canonical
            // session key, which is the same across this whole `mod`). Drop
            // any pre-existing binding first so we actually exercise the
            // "never bound" arm this test pins.
            crate::builtin_tools::scratchpad_registry::clear(&alice_key.to_key_string());

            let resp = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_history(
                        request(
                            "chat.history",
                            json!({ "session_key": alice_key.to_key_string() }),
                        ),
                        store.clone(),
                        None,
                    ),
                )
                .await;
            let result = resp.result.expect("alice may read her own history");
            assert!(
                result.get("active_run").is_some(),
                "`active_run` must always be emitted — absent reads to the \
                 client as an old core, not as `nothing is running`"
            );
            assert!(
                result["active_run"].is_null(),
                "no run manager ⇒ no live turn this handler can confirm"
            );
            assert!(
                result.get("active_run_elapsed_ms").is_some(),
                "the age rides beside `active_run` and is emitted on the same \
                 terms: absent would read to a client as an old core rather \
                 than as `I could not measure it`"
            );
            assert!(
                result["active_run_elapsed_ms"].is_null(),
                "no run to measure ⇒ no age; a client reads null as `count from \
                 now`, never as `it started now`"
            );
            assert!(
                result["active_run"].is_string() || result["active_run"].is_null(),
                "`active_run` stays `string | null` on the wire — the TUI's \
                 three-way parse keys off its PRESENCE and the Panel off its \
                 value, so widening it into an object to carry the age would \
                 have broken both"
            );
            assert!(
                result.get("plan").is_some(),
                "`plan` must always be emitted for the same reason: absent reads \
                 to the client's parser as an old core, and it treats that as \
                 `say nothing about the plan` rather than `there is no plan`"
            );
            assert!(
                result["plan"].is_null(),
                "this session never bound a scratchpad"
            );
        }

        /// The reason this field exists: a client that just attached learns the
        /// durable checklist from the FILE, not by replaying a lossy trace it
        /// may not even have (mid-run, or a plan older than the fetch window).
        #[tokio::test]
        async fn history_serves_the_durable_execution_list() {
            let _home = crate::utils::paths::IsolatedAlephHome::new();
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let canonical = alice_key.to_key_string();

            // Exactly what the `scratchpad` tool does on a `set_plan`.
            let project = "hist-plan-probe";
            crate::builtin_tools::scratchpad_registry::set_active(&canonical, project);
            crate::memory::scratchpad::ScratchpadManager::new(project, &canonical)
                .set_plan(
                    Some("Ship auth"),
                    &[
                        crate::memory::scratchpad::PlanItem {
                            text: "Design".into(),
                            status: crate::memory::scratchpad::PlanItemStatus::Done,
                        },
                        crate::memory::scratchpad::PlanItem {
                            text: "Build".into(),
                            status: crate::memory::scratchpad::PlanItemStatus::InProgress,
                        },
                    ],
                )
                .await
                .unwrap();

            let resp = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_history(
                        request("chat.history", json!({ "session_key": canonical })),
                        store.clone(),
                        None,
                    ),
                )
                .await;
            let result = resp.result.expect("alice may read her own history");
            let plan = &result["plan"];
            assert_eq!(plan["objective"], "Ship auth");
            assert_eq!(plan["items"][0]["status"], "completed");
            assert_eq!(plan["items"][1]["status"], "in_progress");
            assert_eq!(plan["complete"], false);

            crate::builtin_tools::scratchpad_registry::clear(&canonical);
        }

        /// The gate the plan inherits is the transcript's own. A caller who
        /// cannot see the session must not learn its checklist either — and
        /// must not be able to tell "denied" from "no such session".
        #[tokio::test]
        async fn a_denied_history_does_not_leak_the_execution_list() {
            let _home = crate::utils::paths::IsolatedAlephHome::new();
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let canonical = alice_key.to_key_string();

            let project = "hist-plan-leak-probe";
            crate::builtin_tools::scratchpad_registry::set_active(&canonical, project);
            crate::memory::scratchpad::ScratchpadManager::new(project, &canonical)
                .set_plan(
                    Some("Alice's secret objective"),
                    &[crate::memory::scratchpad::PlanItem::pending("secret step")],
                )
                .await
                .unwrap();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_history(
                        request("chat.history", json!({ "session_key": canonical })),
                        store.clone(),
                        None,
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
            assert!(as_bob.result.is_none());
            let rendered = serde_json::to_string(&as_bob).unwrap();
            assert!(
                !rendered.contains("secret"),
                "the refusal leaked plan text: {rendered}"
            );

            crate::builtin_tools::scratchpad_registry::clear(&canonical);
        }

        #[tokio::test]
        async fn chat_clear_denies_cross_user_as_not_found() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_clear(
                        request("chat.clear", json!({ "session_key": alice_key_str })),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
        }

        #[tokio::test]
        async fn chat_rewind_denies_cross_user_as_not_found() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_rewind(
                        request(
                            "chat.rewind",
                            json!({ "session_key": alice_key_str, "seq": 1 }),
                        ),
                        store.clone(),
                        None,
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
        }

        // ── chat.context_estimate ────────────────────────────────────────

        /// A harness whose ONLY job is to prove whether the estimator ran.
        /// The default `estimate_context` returns `None`, which would make a
        /// denial and a pass-through indistinguishable — so this one returns
        /// a value no other code path can produce.
        struct CountingEstimator(std::sync::Arc<std::sync::atomic::AtomicUsize>);

        const SENTINEL_WINDOW: u32 = 424_242;

        #[async_trait::async_trait]
        impl crate::orchestrator::dispatch::HarnessRunner for CountingEstimator {
            #[allow(clippy::too_many_arguments)]
            async fn run(
                &self,
                _s: String,
                _sp: Arc<crate::orchestrator::flow_spec::FlowSpec>,
                _i: crate::orchestrator::flow_spec::FlowInput,
                _sb: Arc<dyn crate::sandbox::Sandbox>,
                _ev: tokio::sync::broadcast::Sender<crate::orchestrator::dispatch::FlowStreamEvent>,
                _c: tokio_util::sync::CancellationToken,
                _tool_service_override: Option<Arc<dyn crate::tools::service::ToolService>>,
                _trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
                _interaction_manifest: Option<crate::thinker::InteractionManifest>,
                _workspace_override: Option<std::path::PathBuf>,
                _max_iterations_override: Option<u32>,
                _transient_context: Option<String>,
                _think_level: Option<crate::agents::thinking::ThinkLevel>,
                _envelope: crate::thinker::TurnEnvelope,
                _turn_model: Option<crate::providers::session_model_handle::SessionModelPref>,
            ) -> Result<
                crate::orchestrator::dispatch::FlowOutcome,
                crate::orchestrator::errors::FlowError,
            > {
                unimplemented!("context_estimate tests never dispatch a run")
            }

            async fn estimate_context(
                &self,
                _session_key: &str,
            ) -> Option<crate::orchestrator::harness_bridge::context_estimate::ContextEstimate>
            {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(
                    crate::orchestrator::harness_bridge::context_estimate::ContextEstimate {
                        used_tokens: 1_234,
                        window_tokens: SENTINEL_WINDOW,
                    },
                )
            }
        }

        fn counting_estimator() -> (
            Arc<dyn crate::orchestrator::dispatch::HarnessRunner>,
            std::sync::Arc<std::sync::atomic::AtomicUsize>,
        ) {
            let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            (Arc::new(CountingEstimator(calls.clone())), calls)
        }

        /// `chat.context_estimate` discloses the addressed session's occupancy
        /// AND the model it is pinned to (`window_tokens`). The denial must
        /// stop the estimator from ever running — asserting only on the
        /// response would still pass if the estimate were computed and then
        /// thrown away, which is exactly the leak on the timing/cache side.
        #[tokio::test]
        async fn context_estimate_denies_a_foreign_session_without_running_the_estimator() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let (harness, calls) = counting_estimator();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_context_estimate(
                        request(
                            "chat.context_estimate",
                            json!({ "session_key": alice_key.to_key_string() }),
                        ),
                        harness.clone(),
                        store.clone(),
                    ),
                )
                .await;
            assert!(as_bob.error.is_none(), "the denial is a null, not an error");
            assert_eq!(as_bob.result, Some(serde_json::Value::Null));
            assert_eq!(
                calls.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "a denied estimate must not read the foreign session at all"
            );

            // The owner still gets the real numbers — this is what proves the
            // gate is a gate and not a blanket disable.
            let as_alice = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_context_estimate(
                        request(
                            "chat.context_estimate",
                            json!({ "session_key": alice_key.to_key_string() }),
                        ),
                        harness.clone(),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_alice
                    .result
                    .as_ref()
                    .and_then(|r| r["window_tokens"].as_u64()),
                Some(u64::from(SENTINEL_WINDOW)),
                "the owner must still see her own pinned model's window"
            );
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }

        /// The half `existing_session_is_visible` exists for: a session that
        /// has never been created is the ordinary fresh-composer case, not a
        /// denial. Turning it into one would blank the gauge for every new
        /// conversation — a regression that looks like the fix working.
        #[tokio::test]
        async fn context_estimate_still_answers_for_a_not_yet_created_session() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let (harness, calls) = counting_estimator();

            let fresh = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_context_estimate(
                        request(
                            "chat.context_estimate",
                            json!({ "session_key": "agent:neverwascreated:main" }),
                        ),
                        harness,
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                fresh
                    .result
                    .as_ref()
                    .and_then(|r| r["window_tokens"].as_u64()),
                Some(u64::from(SENTINEL_WINDOW))
            );
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }

        /// The denial shape is the one this method already had. A malformed
        /// key and a foreign key must serialize identically, so `null` cannot
        /// be read as "that key exists and is not yours".
        #[tokio::test]
        async fn a_denied_estimate_is_byte_identical_to_an_unresolvable_key() {
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let (harness, _calls) = counting_estimator();

            let denied = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_context_estimate(
                        request(
                            "chat.context_estimate",
                            json!({ "session_key": alice_key.to_key_string() }),
                        ),
                        harness.clone(),
                        store.clone(),
                    ),
                )
                .await;
            let unparseable = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_context_estimate(
                        request(
                            "chat.context_estimate",
                            json!({ "session_key": "not a real session key" }),
                        ),
                        harness,
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                serde_json::to_string(&denied).unwrap(),
                serde_json::to_string(&unparseable).unwrap(),
            );
        }
    }
}
