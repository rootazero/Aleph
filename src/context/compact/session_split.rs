//! Compaction-driven session-split.
//!
//! When in-place compaction can no longer hold context pressure down, the
//! harness ends the current session and continues in a fresh child session
//! (`epoch + 1`) seeded with a summary of the pre-tail history plus the
//! verbatim fresh tail. The parent session's log is frozen — never re-read
//! by the loop — so per-turn cost resets to bounded.

use crate::context::compact::compactor::ContextCompactor;
use crate::context::compact::preserve::SUMMARY_MARKER;
use crate::context::compact::summary_utils::{
    cap_summary_lines, clamp_start_to_budget, MAX_SUMMARY_LINES,
};
use crate::providers::message::UnifiedMessage;
use crate::session::epoch_registrar::SessionEpochRegistrar;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::session::service::{SessionId, SessionService};

/// Outcome of a successful split.
#[derive(Debug, Clone)]
pub struct SplitOutcome {
    pub child_session_id: SessionId,
}

/// Why a split could not be performed.
#[derive(Debug)]
pub enum SplitError {
    /// The session key kind has no epoch — `with_next_epoch()` returned the
    /// key unchanged (Group/Task/Subagent/Ephemeral).
    NotSplittable,
    /// Summarization, epoch registration, or event emission failed.
    Failed(anyhow::Error),
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSplittable => write!(f, "session key kind is not splittable"),
            Self::Failed(e) => write!(f, "session split failed: {e}"),
        }
    }
}
impl std::error::Error for SplitError {}

/// Perform a compaction-driven session split.
///
/// `tail_start` is the index into `events` where the fresh tail begins.
/// `events[..tail_start]` is summarized; `events[tail_start..]` is copied
/// verbatim into the child session.
pub async fn perform_session_split(
    session: &dyn SessionService,
    epoch_registrar: &dyn SessionEpochRegistrar,
    compactor: &ContextCompactor,
    parent_session_id: &SessionId,
    events: &[SessionEventRecord],
    tail_start: usize,
) -> Result<SplitOutcome, SplitError> {
    // 1. Mint child; non-epoch kinds are not splittable.
    let child = parent_session_id.with_next_epoch();
    if &child == parent_session_id {
        return Err(SplitError::NotSplittable);
    }

    // Clamp the tail boundary to the event count (P7 defensive design): a
    // `tail_start` past the end would panic the verbatim-copy slice below
    // (`&events[tail_start..]`). `summarize_pretail` already clamps the same
    // way; doing it once here keeps both slices consistent and degrades a
    // bad index to "no fresh tail" (everything summarized) instead of a panic.
    let tail_start = tail_start.min(events.len());

    // Tool-pair snap (P7 + Anthropic API safety): advance the fresh-tail
    // boundary forward past any leading ToolResult/ToolError run so the child
    // session is never seeded with a verbatim tool result whose originating
    // tool_use was summarized away into the SystemMessage. A tool_result with no
    // preceding tool_use is rejected by Anthropic-compatible backends (HTTP 400)
    // on the child's first turn. Single source:
    // `event_snap::snap_past_tool_results` — the event-level twin of the
    // in-place compactor's message-level `snap_boundary_forward`, shared with
    // the manual `/compact` cut selection.
    //
    // Deliberately NOT snapped here: `snap_out_of_open_run`. The split's
    // `tail_start` falls inside the currently-open run by construction (that
    // run has no `RunFinished` in the log yet), so the guard could only fire
    // on a historical run whose close was already lost — and pulling the
    // boundary back over it would re-seed the child with a run the parent
    // already finished. See the guard's doc in `event_snap`.
    let tail_start = super::event_snap::snap_past_tool_results(events, tail_start);

    // The split path used to be the one compaction surface with zero
    // telemetry: the breaker escalated to it, and the only trace of what
    // happened next was the parent's RunFinished appearing in the log. Emit
    // the shape of the split (how much is summarized away vs. carried
    // verbatim) so an operator can tell a healthy escalation from a split
    // storm without replaying the event log.
    tracing::info!(
        target: "context_budget",
        parent = ?parent_session_id,
        child = ?child,
        total_events = events.len(),
        summarized_events = tail_start,
        carried_tail_events = events.len() - tail_start,
        "session split: summarizing pre-tail into a new epoch",
    );

    // 2. Summarize events[..tail_start].
    let summary_text = summarize_pretail(compactor, events, tail_start)
        .await
        .map_err(SplitError::Failed)?;
    tracing::info!(
        target: "context_budget",
        parent = ?parent_session_id,
        summary_chars = summary_text.len(),
        "session split: pre-tail summarized; seeding child epoch",
    );

    // 3. Register the new epoch so gateway routing sees it.
    epoch_registrar
        .register_epoch(&child)
        .await
        .map_err(SplitError::Failed)?;

    // 3b. …and retire what belonged to the epoch that was just superseded.
    // Routing now resolves this conversation to `child`, so anything keyed to
    // the parent's exact epoch — the `/btw` side session, whose key is derived
    // from it — stops being derivable by any surface. Unlike `/new` or a
    // delete, nobody asked for this: the split happens on its own, so without
    // this call the orphan is created by the system and reported to no one.
    // After the registration, deliberately: registration is what makes the
    // parent superseded, and a retirement before a failed registration would
    // have thrown away a side session that is still live.
    epoch_registrar.retire_superseded(parent_session_id).await;

    // 4. Seed the child: SessionForked -> SystemMessage(summary) -> fresh tail.
    let parent_str = parent_session_id.to_key_string();
    session
        .emit_event(
            &child,
            SessionEvent::SessionForked {
                parent_session_id: parent_str,
                at: crate::session::events::now_ms(),
            },
        )
        .await
        .map_err(|e| SplitError::Failed(anyhow::anyhow!("emit SessionForked: {e}")))?;

    session
        .emit_event(&child, build_summary_event(summary_text))
        .await
        .map_err(|e| SplitError::Failed(anyhow::anyhow!("emit summary: {e}")))?;

    for record in &events[tail_start..] {
        session
            .emit_event(&child, record.event.clone())
            .await
            .map_err(|e| SplitError::Failed(anyhow::anyhow!("copy fresh-tail event: {e}")))?;
    }

    // 5. Balance the run markers across the split (Cycle 5 × Cycle 6
    //    integration). The harness bridge emitted `RunStarted` on the parent
    //    at run start; without this the parent's log would end on a dangling
    //    `RunStarted` and `ResumeCoordinator` would mis-detect the frozen
    //    parent as an interrupted run. Close the parent's run, and open one on
    //    the child so a crash *after* the split is detected against the child
    //    (the live epoch the run actually continues on). The run_id need only
    //    correlate within each session's log — the resume scan is positional.
    let split_run_id = uuid::Uuid::new_v4().to_string();
    session
        .emit_event(
            parent_session_id,
            SessionEvent::RunFinished {
                run_id: split_run_id.clone(),
                outcome: crate::session::events::RunOutcome::Completed,
                at: crate::session::events::now_ms(),
            },
        )
        .await
        .map_err(|e| SplitError::Failed(anyhow::anyhow!("emit parent RunFinished: {e}")))?;
    session
        .emit_event(
            &child,
            SessionEvent::RunStarted {
                run_id: split_run_id,
                at: crate::session::events::now_ms(),
                // Session-split forks emit a marker on the child session;
                // the child inherits whatever project context the parent
                // was running under via the in-memory RunRequest, so the
                // resume path does not need a duplicate persisted value
                // here.
                project_root: None,
                envelope: None,
            },
        )
        .await
        .map_err(|e| SplitError::Failed(anyhow::anyhow!("emit child RunStarted: {e}")))?;

    Ok(SplitOutcome {
        child_session_id: child,
    })
}

/// Wrap a summary string in a `SessionEvent::SystemMessage`.
fn build_summary_event(summary: String) -> SessionEvent {
    SessionEvent::SystemMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: format!("{SUMMARY_MARKER}\n{summary}"),
        at: crate::session::events::now_ms(),
    }
}

/// Build a `Vec<UnifiedMessage>` from the pre-tail event slice and summarize
/// it via the compactor's side-channel LLM call.
///
/// Events that do not map to a conversational turn (lifecycle markers, tool
/// calls, etc.) are silently skipped — the summary captures only the dialogue
/// content visible to the user.
///
/// The summary is anchored to the live task: the most recent user request in
/// the verbatim fresh tail (`events[tail_start..]`) is passed as focus so the
/// pre-tail summary preserves detail relevant to the work the child session
/// resumes on. This is the heavy-compaction path where the task thread is most
/// at risk of being abstracted away.
async fn summarize_pretail(
    compactor: &ContextCompactor,
    events: &[SessionEventRecord],
    tail_start: usize,
) -> anyhow::Result<String> {
    let tail_start = tail_start.min(events.len());
    let pretail = &events[..tail_start];
    let messages: Vec<UnifiedMessage> = pretail
        .iter()
        .filter_map(|r| event_to_message(&r.event))
        .collect();

    // Bound the summarizer input: keep the NEWEST budget-worth of pre-tail
    // messages so the side-channel call cannot overflow, and note the elision
    // honestly (the elided span was already covered by earlier in-place
    // compaction summaries where they exist).
    // Bound the summarizer's input by the budget derived from the summarizer
    // model's own window (carried on the compactor), not the historical
    // constant — a narrow-window cheap/aux model cannot hold 48k of transcript.
    let elided = clamp_start_to_budget(&messages, compactor.summarizer_input_budget());
    let kept = &messages[elided..];

    // Live-task anchor: scan the kept tail back-to-front for the latest user
    // turn. Mapped through the same `event_to_message` filter so non-dialogue
    // events are skipped consistently with the summarized body.
    let tail_messages: Vec<UnifiedMessage> = events[tail_start..]
        .iter()
        .filter_map(|r| event_to_message(&r.event))
        .collect();
    let focus = super::summary_utils::latest_user_task(&tail_messages);

    // No user directive on this path: a split is an automatic escalation, not
    // something the user asked for with `/compact <instructions>`.
    let summary = compactor
        .summarize_slice(kept, focus.as_deref(), None)
        .await?;
    // Cap the seeded summary's size regardless of origin: the compactor's
    // deterministic fallback is one line per message, which even the clamped
    // slice can blow into thousands of lines when the events are tiny.
    let summary = cap_summary_lines(summary, MAX_SUMMARY_LINES);
    if elided > 0 {
        return Ok(format!(
            "[{elided} earlier events elided from this summary]\n{summary}"
        ));
    }
    Ok(summary)
}

/// Convert a single `SessionEvent` to a `UnifiedMessage` for summarization.
///
/// Only conversational events (user/assistant/system messages and tool results)
/// are mapped; bookkeeping events are dropped.
///
/// Shared with `manual::compact_session`: both walk a session event log and
/// need the SAME notion of "what counts as a conversational turn". A second
/// private copy would let the manual path summarize a slightly different
/// conversation than the split path does.
pub(crate) fn event_to_message(event: &SessionEvent) -> Option<UnifiedMessage> {
    match event {
        SessionEvent::UserMessage { content, .. } => {
            Some(UnifiedMessage::user(content.text.clone()))
        }
        SessionEvent::AssistantMessage { content, .. } => {
            // A pure tool-call assistant turn carries empty `text` with the
            // action in `content.blocks` (tool_use). Synthesize a deterministic
            // line naming the tools invoked so the session-split seed summary
            // preserves *what the agent did*, not just what the tools returned —
            // this is the heavy-compaction path where the action thread is most
            // at risk of being abstracted away. Deterministic rendering of
            // already-decided calls only; no LLM judgement (R7/R10-safe).
            let mut text = content.text.clone();
            if text.is_empty() {
                let names: Vec<&str> = content
                    .blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                    .filter_map(|b| b.get("name").and_then(|n| n.as_str()))
                    .collect();
                if !names.is_empty() {
                    text = format!("[called tools: {}]", names.join(", "));
                }
            }
            Some(UnifiedMessage::assistant(text))
        }
        SessionEvent::SystemMessage { content, .. } => {
            // Treat prior system messages (e.g., earlier summaries) as user
            // context so the summarizer sees them.
            Some(UnifiedMessage::user(content.clone()))
        }
        SessionEvent::ToolResult {
            call_id, output, ..
        } => {
            let text = match output.value.as_str() {
                Some(s) => s.to_string(),
                None => output.value.to_string(),
            };
            Some(UnifiedMessage::tool_result(
                call_id.clone(),
                "",
                text,
                false,
            ))
        }
        // Lifecycle, budget, fork, and error events are dropped.
        _ => None,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::budget::pressure::estimate_tokens_smart;
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::{broadcast, Mutex};

    use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
    use crate::context::compact::summary_utils::SUMMARIZER_INPUT_TOKEN_BUDGET;
    use crate::providers::mock::MockProvider;
    use crate::session::events::{now_ms, EventSeq, MessageContent, SessionEventRecord};
    use crate::session::service::{SessionError, SessionHandle, SessionService};
    use crate::sync_primitives::Arc as AlephArc;

    // -------------------------------------------------------------------------
    // Fake SessionService
    // -------------------------------------------------------------------------

    #[derive(Default)]
    struct RecordingSessionService {
        log: Mutex<Vec<(SessionId, SessionEvent)>>,
        next_seq: Mutex<EventSeq>,
    }

    impl RecordingSessionService {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        async fn emitted(&self) -> Vec<(SessionId, SessionEvent)> {
            self.log.lock().await.clone()
        }
    }

    #[async_trait]
    impl SessionService for RecordingSessionService {
        async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError> {
            Ok(SessionHandle { id, head_seq: 0 })
        }

        async fn get_events(
            &self,
            _id: &SessionId,
            _from: Option<EventSeq>,
            _to: Option<EventSeq>,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            Ok(vec![])
        }

        async fn emit_event(
            &self,
            id: &SessionId,
            event: SessionEvent,
        ) -> Result<EventSeq, SessionError> {
            let mut seq = self.next_seq.lock().await;
            let s = *seq;
            *seq += 1;
            self.log.lock().await.push((id.clone(), event));
            Ok(s)
        }

        async fn subscribe(
            &self,
            _id: &SessionId,
        ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError> {
            let (_tx, rx) = broadcast::channel(1);
            Ok(rx)
        }

        async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError> {
            self.attach(id.clone()).await
        }

        async fn detach(&self, _id: &SessionId) -> Result<(), SessionError> {
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // Fake SessionEpochRegistrar
    // -------------------------------------------------------------------------

    struct RecordingRegistrar {
        registered: Mutex<Vec<SessionId>>,
    }

    impl RecordingRegistrar {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                registered: Mutex::new(vec![]),
            })
        }

        async fn keys(&self) -> Vec<SessionId> {
            self.registered.lock().await.clone()
        }
    }

    #[async_trait]
    impl crate::session::epoch_registrar::SessionEpochRegistrar for RecordingRegistrar {
        async fn register_epoch(&self, key: &SessionId) -> anyhow::Result<()> {
            self.registered.lock().await.push(key.clone());
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // Helper: make a SessionEventRecord
    // -------------------------------------------------------------------------

    fn user_record(seq: EventSeq, text: &str) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event: SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: text.to_string(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
                author_user_id: None,
            },
            created_at_ms: now_ms(),
        }
    }

    // -------------------------------------------------------------------------
    // Test 1: non-epoch key is not splittable
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn non_epoch_key_kind_is_not_splittable() {
        let parent = crate::routing::session_key::SessionKey::Ephemeral {
            agent_id: "test-agent".to_string(),
            ephemeral_id: "x".to_string(),
        };

        // The early-return check fires before any service is touched.
        // We still need to pass *something* — use the recording fakes as they
        // are cheap and implement the right traits.
        let session = RecordingSessionService::new();
        let registrar = RecordingRegistrar::new();
        let provider = AlephArc::new(MockProvider::new("ignored"));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        let result = perform_session_split(
            session.as_ref(),
            registrar.as_ref(),
            &compactor,
            &parent,
            &[],
            0,
        )
        .await;

        assert!(
            matches!(result, Err(SplitError::NotSplittable)),
            "expected NotSplittable, got {result:?}"
        );
    }

    // -------------------------------------------------------------------------
    // Test 2: successful split seeds child in correct order
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn split_seeds_child_with_forked_summary_and_fresh_tail() {
        // Parent: Main session at epoch 0.
        let parent = crate::routing::session_key::SessionKey::Main {
            agent_id: "agent-a".to_string(),
            main_key: "main".to_string(),
            epoch: 0,
        };
        let expected_child = parent.with_next_epoch();

        // Build events: 2 pre-tail + 1 fresh tail.
        let events = vec![
            user_record(1, "pre-tail message 1"),
            user_record(2, "pre-tail message 2"),
            user_record(3, "fresh tail message"),
        ];
        let tail_start = 2; // events[..2] summarized; events[2..] copied verbatim

        let session = RecordingSessionService::new();
        let registrar = RecordingRegistrar::new();
        let fixed_summary = "This is the LLM summary.";
        let provider = AlephArc::new(MockProvider::new(fixed_summary));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        let outcome = perform_session_split(
            session.as_ref(),
            registrar.as_ref(),
            &compactor,
            &parent,
            &events,
            tail_start,
        )
        .await
        .expect("split should succeed");

        // 1. Returned child matches epoch+1.
        assert_eq!(outcome.child_session_id, expected_child);

        // 2. Registrar received the child key.
        let reg_keys = registrar.keys().await;
        assert_eq!(reg_keys, vec![expected_child.clone()]);

        // 3. Events emitted, in order:
        //    [0] SessionForked        -> child
        //    [1] SystemMessage        -> child
        //    [2] verbatim fresh tail  -> child
        //    [3] RunFinished{Completed} -> parent (run-marker balancing)
        //    [4] RunStarted           -> child  (run-marker balancing)
        let emitted = session.emitted().await;
        assert_eq!(
            emitted.len(),
            5,
            "expected 5 emitted events (forked + summary + 1 tail + 2 run markers), got {}",
            emitted.len()
        );

        // Seeding + the child RunStarted go to the child; only the parent
        // RunFinished targets the parent.
        for (i, (id, _)) in emitted.iter().enumerate() {
            let expected = if i == 3 { &parent } else { &expected_child };
            assert_eq!(id, expected, "event {i} emitted to wrong session");
        }

        // [0] SessionForked with the parent's key string.
        match &emitted[0].1 {
            SessionEvent::SessionForked {
                parent_session_id, ..
            } => {
                assert_eq!(parent_session_id, &parent.to_key_string());
            }
            other => panic!("expected SessionForked, got {other:?}"),
        }

        // [1] SystemMessage containing the summary.
        match &emitted[1].1 {
            SessionEvent::SystemMessage { content, .. } => {
                assert!(
                    content.contains("[Context Summary]"),
                    "summary event should contain [Context Summary], got: {content}"
                );
                assert!(
                    content.contains(fixed_summary),
                    "summary event should contain the LLM output, got: {content}"
                );
            }
            other => panic!("expected SystemMessage, got {other:?}"),
        }

        // [2] Verbatim fresh-tail event (UserMessage with "fresh tail message").
        match &emitted[2].1 {
            SessionEvent::UserMessage { content, .. } => {
                assert_eq!(content.text, "fresh tail message");
            }
            other => panic!("expected UserMessage (fresh tail), got {other:?}"),
        }

        // [3] RunFinished{Completed} closes the parent's run so the frozen
        //     parent is not later mis-detected as an interrupted run.
        match &emitted[3].1 {
            SessionEvent::RunFinished { outcome, .. } => {
                assert_eq!(*outcome, crate::session::events::RunOutcome::Completed);
            }
            other => panic!("expected RunFinished on parent, got {other:?}"),
        }

        // [4] RunStarted opens a run on the child so a crash after the split
        //     is detected against the live (child) epoch.
        match &emitted[4].1 {
            SessionEvent::RunStarted { .. } => {}
            other => panic!("expected RunStarted on child, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------------
    // Pre-tail summarization is bounded (CTX: splits fire when the log is huge)
    // -------------------------------------------------------------------------

    #[test]
    fn clamp_start_keeps_newest_budget_worth() {
        // 2000 messages × ~50 tokens each ≫ the 48k budget: the oldest must be
        // elided and the kept slice must fit the summarizer budget.
        let messages: Vec<UnifiedMessage> = (0..2000)
            .map(|i| UnifiedMessage::user(format!("event {i} {}", "payload ".repeat(25))))
            .collect();
        let start = clamp_start_to_budget(&messages, SUMMARIZER_INPUT_TOKEN_BUDGET);
        assert!(start > 0, "a pre-tail this large must elide older events");
        let kept_tokens: usize = messages[start..]
            .iter()
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum();
        assert!(
            kept_tokens <= SUMMARIZER_INPUT_TOKEN_BUDGET,
            "kept slice must fit the summarizer budget, got {kept_tokens}"
        );
        // The newest message always survives.
        assert!(messages[start..]
            .iter()
            .any(|m| m.text_content().contains("event 1999")));
        // A small pre-tail is untouched.
        let small: Vec<UnifiedMessage> = (0..3)
            .map(|i| UnifiedMessage::user(format!("msg {i}")))
            .collect();
        assert_eq!(
            clamp_start_to_budget(&small, SUMMARIZER_INPUT_TOKEN_BUDGET),
            0
        );
    }

    #[tokio::test]
    async fn huge_pretail_fallback_is_capped_and_notes_elision() {
        // Thousands of pre-tail events with a failing summarizer LLM: the
        // compactor falls back to its one-line-per-message deterministic dump.
        // The seeded summary must stay bounded and honest — never a
        // many-thousand-line dump copied into the child session.
        let events: Vec<SessionEventRecord> = (0..3000)
            .map(|i| user_record(i as u64, &format!("event {i} {}", "payload ".repeat(25))))
            .collect();
        let provider = AlephArc::new(
            MockProvider::new("ignored").with_error(crate::providers::mock::MockError::Timeout),
        );
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        let summary = summarize_pretail(&compactor, &events, events.len())
            .await
            .expect("fallback path must still produce a summary");

        assert!(
            summary.starts_with('[') && summary.contains("elided from this summary"),
            "expected an honest elision note, got: {}",
            summary.lines().next().unwrap_or("")
        );
        let lines = summary.lines().count();
        assert!(
            lines <= MAX_SUMMARY_LINES + 2,
            "seed summary must be line-capped, got {lines} lines"
        );
        // Newest-first retention: the most recent event survives the caps.
        assert!(
            summary.contains("event 2999"),
            "the newest pre-tail event must survive both caps"
        );
    }

    // -------------------------------------------------------------------------
    // Test 3: tail_start past the event count is clamped, not a panic
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn tail_start_past_end_is_clamped_not_panic() {
        // A `tail_start` greater than `events.len()` must NOT panic the
        // verbatim-copy slice. It is clamped to the event count, which means
        // "no fresh tail" — every event is summarized and none is copied.
        let parent = crate::routing::session_key::SessionKey::Main {
            agent_id: "agent-a".to_string(),
            main_key: "main".to_string(),
            epoch: 0,
        };
        let events = vec![user_record(1, "only message")];

        let session = RecordingSessionService::new();
        let registrar = RecordingRegistrar::new();
        let provider = AlephArc::new(MockProvider::new("summary"));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        // tail_start = 5 ≫ events.len() = 1.
        let outcome = perform_session_split(
            session.as_ref(),
            registrar.as_ref(),
            &compactor,
            &parent,
            &events,
            5,
        )
        .await
        .expect("split should succeed with a clamped tail_start");

        assert_eq!(outcome.child_session_id, parent.with_next_epoch());

        // No verbatim fresh-tail event copied (tail clamped to len): the child
        // sees only SessionForked + SystemMessage(summary), plus the two run
        // markers — never a copied UserMessage.
        let emitted = session.emitted().await;
        assert!(
            !emitted
                .iter()
                .any(|(_, e)| matches!(e, SessionEvent::UserMessage { .. })),
            "no fresh-tail UserMessage should be copied when tail_start is clamped",
        );
    }
}
