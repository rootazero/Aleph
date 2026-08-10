//! Chat API — wraps chat.send / chat.abort / chat.history / chat.clear RPC methods.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::VecDeque;

/// How many of this Panel's own run ids to remember.
///
/// The only reader is the `run.session_updated` re-hydrate decision, which asks
/// about the run that *just* touched the open session — so an id ageing off the
/// back cannot produce a wrong lasting answer, only one extra (idempotent)
/// history reload. Sized well past any plausible number of runs between two
/// consecutive updates of one session.
const OWN_RUN_MEMORY: usize = 64;

thread_local! {
    /// Run ids this Panel started, oldest first.
    ///
    /// Written by exactly ONE place — [`ChatApi::send`]'s success arm, i.e. the
    /// instant the server hands this process a run id. That is deliberate and
    /// is the property the whole mechanism rests on: "did I start this run?"
    /// must not depend on four send sites all remembering to register, because
    /// a fifth one will not. `record_own_run` is private to this module, so the
    /// compiler forbids a second recorder; `chat_send_is_the_only_way_to_start_a_run`
    /// covers the other half — a new module issuing the RPC itself.
    ///
    /// A `thread_local` rather than a Leptos signal because the fact is about
    /// the *process*, not about any component's lifetime: it must outlive the
    /// conversation tab that sent (`SessionMap::route` is cleared on
    /// `settle_run`, and the update that matters most arrives right after the
    /// run completes) and it is never rendered, so nothing should re-run when
    /// it changes.
    static OWN_RUNS: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) };
}

/// Remember a run id this Panel started. Idempotent — a steered send returns a
/// run id the client already holds, and re-recording it must not evict a
/// younger one.
fn record_own_run(run_id: &str) {
    if run_id.is_empty() {
        return;
    }
    OWN_RUNS.with_borrow_mut(|runs| {
        if runs.iter().any(|r| r == run_id) {
            return;
        }
        if runs.len() >= OWN_RUN_MEMORY {
            runs.pop_front();
        }
        runs.push_back(run_id.to_string());
    });
}

/// Did THIS Panel start this run?
///
/// The question `origin_channel` cannot answer: every Panel connection sends
/// the literal `"gui:chat"`, so a channel comparison says "mine" for a second
/// tab of the same user and for every other member of a project room.
#[must_use]
pub fn is_own_run(run_id: &str) -> bool {
    OWN_RUNS.with_borrow(|runs| runs.iter().any(|r| r == run_id))
}

/// A single chat message (from history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user" | "assistant" | "system"
    pub content: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
    /// Last-turn context-window occupancy persisted on assistant turns, so the
    /// gauge re-projects when a session is reloaded from history.
    #[serde(default)]
    pub context_tokens: Option<u32>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    /// The human principal who typed this turn, when it was sent inside a
    /// project room (P2 Task 6). `None` for a single-user session, a
    /// pre-P2 core, or an assistant/system row. Mirrors
    /// `gateway::handlers::chat::ChatMessage::author_user_id` on the wire.
    #[serde(default)]
    pub author_user_id: Option<String>,
}

/// Response from chat.send
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSendResponse {
    pub run_id: String,
    pub session_key: String,
    pub streaming: bool,
}

/// Response from chat.context_estimate — a pre-run occupancy estimate for a
/// session that never ran an LLM turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEstimateResponse {
    pub used_tokens: u32,
    pub window_tokens: u32,
}

/// One `chat.history` response: the persisted transcript, plus the run that is
/// in flight on this session *right now* (`None` = nothing running).
///
/// `active_run` exists so a client that opens a conversation **mid-turn** can
/// join it. Nothing else can tell it: the `stream.*` frames of a run in
/// progress carry a `run_id` this client never saw accepted, so they route
/// nowhere, and the sidebar's re-hydrate is suppressed for as long as the
/// session is running. Without this the second terminal on a shared thread sat
/// in front of a frozen transcript for the whole turn.
#[derive(Debug, Clone)]
pub struct SessionHistory {
    pub messages: Vec<ChatMessage>,
    pub active_run: Option<String>,
}

/// A file attachment to send with a chat message.
#[derive(Debug, Clone)]
pub struct ChatAttachment {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,
    pub size: u64,
}

pub struct ChatApi;

impl ChatApi {
    /// Send a message and start an agent run.
    ///
    /// `agent_id` — explicit target agent (bypasses channel binding resolution).
    /// Extracted from the current `session_key` when available.
    ///
    /// `project_root` — absolute path of the active project folder when
    /// the user has entered project mode via "enter project workspace". Forwarded as
    /// `RunRequest.workspace_override` so the agent's tool calls run
    /// inside that folder instead of `~/.aleph/workspaces/{agent_id}`.
    ///
    /// `project_id` — the project ROOM this turn belongs to (P2), distinct
    /// from `project_root`. The session's stored scope outranks this after
    /// the first turn, so resending it every turn is safe and idempotent.
    /// CRITICAL: a project-room send must pass `project_root: None` — an
    /// explicit `project_root` outranks the room's workspace binding and is
    /// REFUSED server-side for remote chat-tier callers. Callers must not
    /// set both.
    ///
    /// `voice_input` — true when `message` is an ASR-transcribed spoken
    /// utterance (voice loop / dictation). Core then arms the session's
    /// voice-mode prompt layer and the `[voice]` low-TTFT model pin.
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        state: &DashboardState,
        message: &str,
        session_key: Option<&str>,
        attachments: Vec<ChatAttachment>,
        agent_id: Option<&str>,
        project_root: Option<&str>,
        project_id: Option<&str>,
        model_override: Option<&crate::api::providers::ModelOverride>,
        exec_tier: Option<&str>,
        mode: Option<&str>,
        voice_input: bool,
    ) -> Result<ChatSendResponse, String> {
        let attachments_json: Vec<Value> = attachments
            .iter()
            .map(|a| {
                serde_json::json!({
                    "name": a.name,
                    "mime_type": a.mime_type,
                    "data": a.data_base64,
                })
            })
            .collect();

        let params = serde_json::json!({
            "message": message,
            "session_key": session_key,
            "channel": "gui:chat",
            "stream": true,
            "attachments": attachments_json,
            "agent_id": agent_id,
            "project_root": project_root,
            "project_id": project_id,
            "model_override": model_override,
            // The tier rides on the message because a brand-new conversation
            // has no session to write it to yet — and the first turn is exactly
            // the one the picker was armed for. The server stamps it onto the
            // session, so later turns need not resend it.
            "exec_tier": exec_tier,
            // Same first-message carriage for the usage mode (mode pill).
            "mode": mode,
            "voice_input": voice_input,
        });
        let result = state.rpc_call("chat.send", params).await?;
        let resp: ChatSendResponse = serde_json::from_value(result).map_err(|e| e.to_string())?;
        // The single producer of "runs this Panel started" (see `OWN_RUNS`).
        // Recorded here rather than at the send sites so a new one cannot be
        // born already broken: the server telling US a run id IS the fact.
        record_own_run(&resp.run_id);
        Ok(resp)
    }

    /// Abort a running agent, and abandon whatever that session still has
    /// waiting in the gateway's busy lane.
    ///
    /// `session_key` is what makes Stop mean "I do not want this work" rather
    /// than "stop this one run": without it the cancel frees the session slot
    /// and the server-side backlog starts firing a fresh agent run per queued
    /// message. Pass `None` only when there is no session to scope to (a run
    /// aborted before its first `chat.send` returned).
    pub async fn abort(
        state: &DashboardState,
        run_id: &str,
        session_key: Option<&str>,
    ) -> Result<(), String> {
        let params = serde_json::json!({ "run_id": run_id, "session_key": session_key });
        state.rpc_call("chat.abort", params).await?;
        Ok(())
    }

    /// Get chat history for a session.
    /// Load a session's transcript **and** whether a turn is in flight on it.
    ///
    /// The two travel together because they are one snapshot of one session:
    /// fetching them separately would leave a window in which the caller holds
    /// the transcript but not the live run (so the rest of that turn renders
    /// nowhere) or the reverse (so a placeholder bubble opens above a
    /// transcript that has not loaded).
    pub async fn history(
        state: &DashboardState,
        session_key: &str,
        limit: Option<usize>,
    ) -> Result<SessionHistory, String> {
        let params = serde_json::json!({
            "session_key": session_key,
            "limit": limit,
        });
        let result = state.rpc_call("chat.history", params).await?;
        let messages = result
            .get("messages")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        Ok(SessionHistory {
            messages: serde_json::from_value(messages).map_err(|e| e.to_string())?,
            // Absent against a core that predates the field, and `null`
            // whenever nothing is running — both mean "no live turn to join",
            // so one `and_then` covers the skew and the ordinary case.
            active_run: result
                .get("active_run")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        })
    }

    /// Clear chat history for a session.
    pub async fn clear(state: &DashboardState, session_key: &str) -> Result<(), String> {
        let params = serde_json::json!({ "session_key": session_key });
        state.rpc_call("chat.clear", params).await?;
        Ok(())
    }

    /// Estimate a session's next-prompt occupancy (sessions with no real
    /// occupancy recorded). `Ok(None)` when core returns null (unresolvable
    /// session/model) → caller keeps the gauge hidden.
    pub async fn context_estimate(
        state: &DashboardState,
        session_key: &str,
    ) -> Result<Option<ContextEstimateResponse>, String> {
        let params = serde_json::json!({ "session_key": session_key });
        let result = state.rpc_call("chat.context_estimate", params).await?;
        if result.is_null() {
            return Ok(None);
        }
        serde_json::from_value(result)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    /// Create a new session by closing the current one and incrementing the epoch.
    /// Returns the new session key.
    pub async fn new_session(
        state: &DashboardState,
        current_session_key: &str,
        topic: Option<&str>,
    ) -> Result<String, String> {
        let params = serde_json::json!({
            "session_key": current_session_key,
            "topic": topic,
        });
        let result = state.rpc_call("sessions.new", params).await?;
        result
            .get("new_session_key")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .ok_or_else(|| "Missing new_session_key in response".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OWN_RUNS` is a `thread_local`, and libtest runs the whole file on ONE
    /// thread under `--test-threads=1`. Tests that assert absolute lengths must
    /// therefore start from a known state rather than from whatever ran before.
    fn clear_ledger() {
        OWN_RUNS.with_borrow_mut(VecDeque::clear);
    }

    #[test]
    fn estimate_response_round_trips() {
        let v = serde_json::json!({ "used_tokens": 12_000, "window_tokens": 200_000 });
        let r: ContextEstimateResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.used_tokens, 12_000);
        assert_eq!(r.window_tokens, 200_000);
    }

    /// A recorded run is ours; a run id this Panel never saw from its own
    /// `chat.send` is not. That second half is the whole point — it is what a
    /// second tab of the same user, and every other member of a project room,
    /// answers for the run that just updated the session.
    #[test]
    fn a_recorded_run_is_ours_and_an_unseen_one_is_not() {
        clear_ledger();
        record_own_run("run-mine-1");
        assert!(is_own_run("run-mine-1"));
        assert!(
            !is_own_run("run-from-another-panel"),
            "a run this Panel never started must not read as its own"
        );
    }

    /// The ledger is bounded, and the bound evicts the OLDEST — evicting the
    /// newest would make the mechanism fail exactly on the run whose update is
    /// still in flight.
    #[test]
    fn the_ledger_is_bounded_and_evicts_oldest_first() {
        clear_ledger();
        record_own_run("run-eviction-canary");
        for i in 0..OWN_RUN_MEMORY {
            record_own_run(&format!("run-filler-{i}"));
        }
        assert!(
            !is_own_run("run-eviction-canary"),
            "the first id must have aged off once the bound is exceeded"
        );
        assert!(
            is_own_run(&format!("run-filler-{}", OWN_RUN_MEMORY - 1)),
            "the newest id must survive"
        );
        assert_eq!(OWN_RUNS.with_borrow(VecDeque::len), OWN_RUN_MEMORY);
    }

    /// Re-recording a run id must not consume a slot: a queue flush that steers
    /// into a live run gets the SAME run id back from every `chat.send` in the
    /// batch, so a naive push would evict `OWN_RUN_MEMORY` real ids per flush.
    #[test]
    fn re_recording_the_same_run_does_not_consume_a_slot() {
        clear_ledger();
        let before = OWN_RUNS.with_borrow(VecDeque::len);
        record_own_run("run-steered");
        let after_first = OWN_RUNS.with_borrow(VecDeque::len);
        record_own_run("run-steered");
        record_own_run("run-steered");
        assert_eq!(OWN_RUNS.with_borrow(VecDeque::len), after_first);
        assert_eq!(after_first, before + 1);
    }

    /// The ledger's single producer is `ChatApi::send`, which is only sound
    /// while `ChatApi::send` is the crate's only way to start a run. Recording
    /// at the four *call sites* of `send` was the obvious alternative and was
    /// rejected: that shape is born one site short every time a fifth send path
    /// appears, and the symptom is silent — the sender re-hydrates over its own
    /// transcript, which reads as a rendering glitch, not a wiring bug.
    ///
    /// `record_own_run` itself is private to this module, so the compiler
    /// already forbids a second recorder elsewhere. What the compiler cannot
    /// see is a new module issuing the RPC directly and never being recorded at
    /// all — which is what this scans for.
    #[test]
    fn chat_send_is_the_only_way_to_start_a_run() {
        let sources = crate::disposed_reads::rust_sources(&crate::disposed_reads::src_dir());
        assert!(
            sources.len() > 50,
            "found {} sources — the walk is broken, not the code",
            sources.len()
        );
        let needle = "\"chat.send\"";
        let mut callers = Vec::new();
        for path in sources {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            if src.contains(needle) {
                callers.push(path.display().to_string());
            }
        }
        assert_eq!(
            callers.len(),
            1,
            "`chat.send` must be issued from api/chat.rs alone — every other \
             caller starts a run this Panel then fails to recognise as its own. \
             Callers: {callers:?}"
        );
        // Compare on path components, not on a "/"-joined suffix: `Display` for
        // a `Path` emits the platform separator, so a hardcoded `api/chat.rs`
        // never matches on Windows and this pin fails for the one reason it is
        // not about.
        let tail: Vec<_> = std::path::Path::new(&callers[0])
            .components()
            .rev()
            .take(2)
            .collect();
        assert!(
            tail.iter()
                .rev()
                .map(|c| c.as_os_str())
                .eq(["api", "chat.rs"]),
            "the one caller moved out of api/chat.rs: {}",
            callers[0]
        );
    }
}
