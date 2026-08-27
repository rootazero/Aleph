//! Chat API — wraps chat.send / chat.abort / chat.history / chat.clear RPC methods.

use crate::context::DashboardState;
use aleph_protocol::queue::PendingRun;
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

/// One `chat.history` response: the persisted transcript, plus the two facts
/// about the session's *current* state that the transcript cannot express.
///
/// `active_run` exists so a client that opens a conversation **mid-turn** can
/// join it. Nothing else can tell it: the `stream.*` frames of a run in
/// progress carry a `run_id` this client never saw accepted, so they route
/// nowhere, and the sidebar's re-hydrate is suppressed for as long as the
/// session is running. Without this the second terminal on a shared thread sat
/// in front of a frozen transcript for the whole turn.
///
/// `plan` is the durable execution list, read server-side from the scratchpad
/// file the model itself works. It exists for the same reason one field over:
/// the Todo strip was fed only by live frames and by replaying the *lossy*
/// trace mirror, so a fresh attach — refresh, second tab, second device, next
/// morning — showed nothing for a checklist the model is still being held to.
/// `None` means the session has no list, which is different from "we did not
/// look": a core that predates the field also sends nothing, and that reads the
/// same, which is why the caller applies it only when present.
///
/// `pending` is the wait lane's authoritative half — the same fact
/// `StreamEvent::RunQueued` carries, for a client that attached after those
/// frames fired. A live client learns it is queued from the stream; a client
/// that attaches mid-wait — a refresh, a second tab, a second device, a room
/// teammate — never received those frames, so this snapshot is the only place
/// the fact survives for it. Absent (a core that predates the field) and
/// empty (nothing waiting) both mean "no queue to show".
#[derive(Debug, Clone)]
pub struct SessionHistory {
    pub messages: Vec<ChatMessage>,
    pub active_run: Option<String>,
    pub plan: Option<aleph_protocol::plan::PlanSnapshot>,
    pub pending: Vec<PendingRun>,
}

/// A file attachment to send with a chat message.
#[derive(Debug, Clone)]
pub struct ChatAttachment {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,
    pub size: u64,
}

/// Read the durable execution list off a `chat.history` response.
///
/// Free function so the skew and malformed cases are testable without a live
/// gateway. Three inputs collapse to `None`, on purpose:
///
/// * field absent — a core older than the field;
/// * `null` — this session has no execution list;
/// * present but unparseable — a shape change on the core side.
///
/// The last one is the interesting choice: the transcript is what this call is
/// for, and dropping it because a checklist did not decode would be a worse
/// failure than the one the field exists to fix. The caller treats `None` as
/// "say nothing about the plan" rather than "there is no plan", so all three
/// degrade to the pre-existing behaviour instead of blanking a live strip.
fn parse_history_plan(result: &Value) -> Option<aleph_protocol::plan::PlanSnapshot> {
    let raw = result.get("plan").filter(|v| !v.is_null())?;
    serde_json::from_value(raw.clone()).ok()
}

/// Read the wait lane off a `chat.history` response.
///
/// Free function so the skew and malformed cases are testable without a live
/// socket — same shape as `parse_history_plan` next door. Absent reads as
/// empty: a core that predates the field and an idle session are
/// indistinguishable here, and both mean "no queue to show".
///
/// Each item is deserialized into the shared protocol type rather than
/// hand-read key by key. That is the whole point of the type being shared: a
/// field renamed on the server changes this side's parse at the same time,
/// instead of leaving a client that reads an empty queue and says nothing.
/// An item that fails to deserialize is dropped, not fatal — one malformed
/// entry must not hide the rest of the lane.
#[must_use]
pub fn parse_history_pending(result: &Value) -> Vec<PendingRun> {
    let Some(items) = result.get("pending").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| serde_json::from_value::<PendingRun>(v.clone()).ok())
        .collect()
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
    /// `dials` — the per-session knobs this send carries, already reduced by
    /// `session_dials_for_send` to the ones that should ride *this* message.
    /// One argument rather than four because a caller that forgets one does not
    /// fail: the turn simply runs under the install default while the pill that
    /// set it keeps showing the user's choice.
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
        dials: &shared_ui_logic::state::SendDials,
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
            "exec_tier": dials.exec_tier,
            // Same first-message carriage for the other three dials. Their wire
            // names are the server's (`thinking`, `memory`), not the storage
            // keys — see `AgentRunParams`.
            "mode": dials.mode,
            "thinking": dials.thinking,
            "memory": dials.memory,
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
            plan: parse_history_plan(&result),
            pending: parse_history_pending(&result),
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

    /// The durable list arrives with every status intact — this is the field
    /// that lets a refreshed page, a second tab, or a second device see a
    /// checklist that until now only existed in whoever's browser had watched
    /// it being built.
    #[test]
    fn a_served_plan_decodes_with_its_statuses() {
        let resp = serde_json::json!({
            "messages": [],
            "plan": {
                "objective": "Ship auth",
                "complete": false,
                "items": [
                    {"text": "Design", "status": "completed"},
                    {"text": "Build", "status": "in_progress"},
                    {"text": "Test", "status": "pending"}
                ]
            }
        });
        let plan = parse_history_plan(&resp).expect("a served plan decodes");
        assert_eq!(plan.objective.as_deref(), Some("Ship auth"));
        assert_eq!(plan.done_count(), 1);
        assert_eq!(plan.total(), 3);
        assert_eq!(plan.current_step(), Some("Build"));
    }

    /// All three "we were told nothing usable" shapes collapse to `None`, and
    /// the caller leaves the strip alone rather than clearing it. Clearing on
    /// ambiguity would take the Todo panel away from exactly the clients this
    /// field was added to serve.
    #[test]
    fn absent_null_and_malformed_plans_all_read_as_no_answer() {
        assert!(parse_history_plan(&serde_json::json!({"messages": []})).is_none());
        assert!(parse_history_plan(&serde_json::json!({"plan": null})).is_none());
        assert!(parse_history_plan(&serde_json::json!({"plan": {"items": "nope"}})).is_none());
    }

    /// A client that attaches mid-wait never received the `RunQueued` frames —
    /// they fired before its socket existed. Without the snapshot it paints
    /// "thinking" over a queue it cannot see.
    #[test]
    fn pending_is_read_off_the_history_response() {
        let raw = serde_json::json!({
            "messages": [],
            "active_run": null,
            "pending": [
                {"run_id": "run-a", "ahead": 0},
                {"run_id": "run-b", "ahead": 1},
            ]
        });
        let pending = parse_history_pending(&raw);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[1].run_id, "run-b");
        assert_eq!(pending[1].ahead, 1);
    }

    /// The shape is `aleph_protocol::queue::PendingRun`, the same type the
    /// server serializes — not a Panel-local copy of its field names. That is
    /// what makes a server-side rename a compile error here instead of a
    /// client that silently reads an empty queue. Asserting it by round-trip
    /// keeps the check on the type rather than on a literal key list.
    #[test]
    fn the_shape_is_the_shared_protocol_type() {
        let one = aleph_protocol::queue::PendingRun {
            run_id: "run-a".to_string(),
            ahead: 2,
        };
        let raw = serde_json::json!({ "pending": [serde_json::to_value(&one).unwrap()] });
        assert_eq!(parse_history_pending(&raw), vec![one]);
    }

    /// Absent against a core that predates the field, and absent when nothing
    /// is waiting. Both mean "no queue to show", so neither may error.
    #[test]
    fn a_missing_or_malformed_pending_array_reads_as_empty() {
        assert!(parse_history_pending(&serde_json::json!({"messages": []})).is_empty());
        assert!(parse_history_pending(&serde_json::json!({"pending": "nonsense"})).is_empty());
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

    /// Every send path resolves the session dials through the shared rule.
    ///
    /// The compiler already forces a caller to pass *something* for `dials`;
    /// what it cannot see is a caller passing `SendDials::default()`, or
    /// re-deriving the first-send-only rule inline. Both existed: the two voice
    /// paths each carried their own copy of that rule, written when there were
    /// two dials, and so kept carrying two of four once there were four. The
    /// pills stayed on screen and kept showing the user's pick, which is
    /// exactly what makes the failure invisible.
    ///
    /// Scans by file rather than by call site because the rule is resolved once
    /// per send *path* and used by every `ChatApi::send` in it (a queue flush
    /// sends in a loop).
    #[test]
    fn every_send_path_resolves_the_dials_through_the_shared_rule() {
        let sources = crate::disposed_reads::rust_sources(&crate::disposed_reads::src_dir());
        assert!(sources.len() > 50, "the walk is broken, not the code");

        let mut senders = 0_usize;
        let mut offenders = Vec::new();
        for path in sources {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !src.contains("ChatApi::send(") {
                continue;
            }
            senders += 1;
            if !src.contains("session_dials_for_send(") {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            senders >= 4,
            "found {senders} send paths — the scan stopped seeing them, which              would make this test pass by finding nothing"
        );
        assert!(
            offenders.is_empty(),
            "these send paths build their dials some other way: {offenders:?}"
        );
    }

    /// Every send path binds the run it started to a conversation.
    ///
    /// `bind_run` is what installs the `run_id -> ConvId` route, and that route
    /// is the FIRST of `resolve_target`'s three steps — without it none of the
    /// run's later frames (reasoning, tool rows, streamed text, the final
    /// answer) can be placed, and the handler returns before touching anything.
    /// The failure is completely silent: the send succeeds, the server streams
    /// the whole turn, and the surface renders nothing.
    ///
    /// The phone composer shipped exactly like that for as long as it has
    /// existed — it was the only send path in the crate with no `bind_run`, and
    /// no test asked. Scanned by file, like its sibling above, because one send
    /// *path* may contain several `ChatApi::send` calls (a queue flush sends in
    /// a loop and binds only the first, since the rest steer into that run).
    ///
    /// Production halves only: this file and the wide composer both name
    /// `ChatApi::send(` inside their own test fixtures.
    #[test]
    fn every_send_path_binds_its_run() {
        let sources = crate::disposed_reads::rust_sources(&crate::disposed_reads::src_dir());
        assert!(sources.len() > 50, "the walk is broken, not the code");

        let mut senders = 0_usize;
        let mut offenders = Vec::new();
        for path in sources {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Routed through `production_lines` (the same gated-ITEM walk the
            // rest of the panel uses — a hand-rolled `split("#[cfg(test)]")`
            // here is exactly what the `i18n_census` "no hand-rolled cut" guard
            // says no to, and a previous form of this cut truncated at the
            // first `#[cfg(test)]` line, under-scanning every gated `use` /
            // helper `fn` / `mod` above the trailing test module and reporting
            // a clean pass for whatever it could not see).
            let production: String = crate::i18n_census::production_lines(&src)
                .into_iter()
                .map(|(_, line)| line)
                .collect::<Vec<_>>()
                .join("\n");
            if !production.contains("ChatApi::send(") {
                continue;
            }
            senders += 1;
            if !production.contains("bind_run(") {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            senders >= 4,
            "found {senders} send paths — the scan stopped seeing them, which \
             would make this test pass by finding nothing"
        );
        assert!(
            offenders.is_empty(),
            "these send paths start a run nothing can route: {offenders:?}"
        );
    }
}
