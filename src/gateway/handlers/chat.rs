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
    /// True when this message is an ASR-transcribed spoken utterance (the
    /// Panel voice loop). Forwarded to [`AgentRunParams::voice_input`] so the
    /// session gets the voice-mode prompt layer and the `[voice]` model pin.
    #[serde(default)]
    pub voice_input: bool,
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

/// Parse the `chat.history` `before` cursor into Unix seconds.
///
/// Accepts a bare integer (Unix seconds) or an RFC 3339 timestamp. Returns
/// `None` for empty / unparseable input so a malformed cursor degrades to an
/// un-paginated (most-recent) fetch rather than erroring the request.
fn parse_before(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(unix) = trimmed.parse::<i64>() {
        return Some(unix);
    }
    chrono::DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|dt| dt.timestamp())
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
        voice_input: params.voice_input,
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
    // entry either, so there is nothing to authorize.
    if let Some(ref key_str) = params.session_key {
        if let Some(session_key) = SessionKey::from_key_string(key_str) {
            let meta = match session_store.get_metadata(&session_key).await {
                Ok(Some(m)) => m,
                Ok(None) => return visibility::not_found_response(request.id),
                Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
            };
            if !visibility::session_visible(&meta) {
                return visibility::not_found_response(request.id); // same error as missing (GC 4)
            }
        }
    }

    // Drop the backlog before cancelling, never after: cancelling releases the
    // session slot, which wakes the lane's front waiter, which can be admitted
    // (and so leave the lane) before a later purge could mark it. Same ordering
    // rule as `/stop` in `inbound_router::command_handler::handle_stop`.
    let dropped = params
        .session_key
        .as_deref()
        .map_or(0, crate::gateway::busy_queue::purge);

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
/// Returns the chat history for a session.
pub async fn handle_history(
    request: JsonRpcRequest,
    session_manager: Arc<dyn SessionStore>,
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

    // Get history from session manager, honoring the `before` cursor.
    match session_manager
        .get_history_before(&session_key, params.limit, before_ts)
        .await
    {
        Ok(messages) => {
            let chat_messages: Vec<ChatMessage> = messages
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
                    }
                })
                .collect();

            let count = chat_messages.len();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": params.session_key,
                    "messages": chat_messages,
                    "count": count,
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
pub async fn handle_context_estimate(
    request: JsonRpcRequest,
    harness: Arc<dyn crate::orchestrator::dispatch::HarnessRunner>,
) -> JsonRpcResponse {
    let params: EstimateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match harness.estimate_context(&params.session_key).await {
        Some(est) => JsonRpcResponse::success(
            request.id,
            json!({
                "used_tokens": est.used_tokens,
                "window_tokens": est.window_tokens,
            }),
        ),
        None => JsonRpcResponse::success(request.id, serde_json::Value::Null),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    #[test]
    fn test_parse_before_unix_and_rfc3339_agree() {
        // 2021-01-01T00:00:00Z == 1609459200 Unix seconds.
        assert_eq!(parse_before("1609459200"), Some(1609459200));
        assert_eq!(parse_before("2021-01-01T00:00:00Z"), Some(1609459200));
        // Surrounding whitespace is tolerated.
        assert_eq!(parse_before("  1609459200  "), Some(1609459200));
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
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
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
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
        }
    }
}
