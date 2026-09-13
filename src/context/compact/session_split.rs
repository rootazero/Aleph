//! Compaction-driven session-split.
//!
//! When in-place compaction can no longer hold context pressure down, the
//! harness ends the current session and continues in a fresh child session
//! (`epoch + 1`) seeded with a summary of the pre-tail history plus the
//! verbatim fresh tail. The parent session's log is frozen — never re-read
//! by the loop — so per-turn cost resets to bounded.
//!
//! The split is two `emit_batch` transactions (parent closer, then the whole
//! child seed) followed by the routing write; the order and its torn states
//! are documented at the call sites in [`perform_session_split`], and the
//! boot-side half is `ProjectionReconciler::heal_split_epochs`.

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
    /// The parent log holds no open `RunStarted` to inherit from — either no
    /// marker at all, or the last one is already closed. The child's opener
    /// is a clone of the parent's, so with nothing to clone the split is
    /// refused before any batch is written rather than seeded with an empty
    /// envelope that a later crash would resume unsnapshotted.
    NoOpenRun,
    /// The parent log could not be reduced, summarization failed, or event
    /// emission failed. Epoch registration is NOT fatal (see step 5 in
    /// [`perform_session_split`]): once both batches are committed the split
    /// has happened, and a refused routing write is logged and healed at the
    /// next boot rather than reported here.
    Failed(anyhow::Error),
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSplittable => write!(f, "session key kind is not splittable"),
            Self::NoOpenRun => write!(
                f,
                "parent log has no open RunStarted for the child to inherit"
            ),
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

    // The run the child continues. Its opener below is a CLONE of the
    // parent's open `RunStarted` — the envelope a resume replays and the
    // project root it resumes in — because a crash after the split is
    // detected against the child, and a child opened with no envelope
    // resumes unsnapshotted: every knob replaced by today's value, the skill
    // scope gone. `reduce_run` is the one derivation of "which run is open"
    // (the last `RunStarted` with no `RunFinished` after it); an `Err` here
    // means the log cannot be reasoned about, which is a failure, and no
    // open run means nothing to inherit, which is a refusal — both before
    // the summarizer is paid for and before either batch is written.
    let open_run = crate::session::reduction::reduce_run(events)
        .map_err(|c| SplitError::Failed(anyhow::anyhow!("parent log contradiction: {c}")))?
        .open_run
        .ok_or(SplitError::NoOpenRun)?;

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

    // Two batches and one routing write, in a DECIDED order. The order is the
    // crash contract: the boot heal (`ProjectionReconciler::heal_split_epochs`)
    // reads "a `SessionForked` at seq 1 of a child" as "the parent is already
    // closed", so the parent's closer must be durable before the child exists.
    //
    //   3. parent  [RunFinished{Completed}]                 ← one transaction
    //   4. child   [SessionForked, summary, tail…, RunStarted] ← one transaction
    //   5. routing  register_epoch(child)                    ← other connection
    //
    // Torn states, parent-first: (i) died after 3 — the parent reads `Clean`,
    // no child exists, routing still names the parent; the next turn simply
    // re-splits, nothing runs twice. (ii) died after 4 — the log says split,
    // routing says parent; the heal registers the child at boot, BEFORE the
    // resume pass, so the one interrupted run resume finds is the child's.
    // Child-first would have a window where BOTH sessions reduce to
    // `Interrupted` and both get resumed (double execution), and no boot
    // pass could tell that apart from two independent crashes.
    //
    // The epoch cannot join either transaction: the routing table is the
    // gateway `SessionStore`'s own connection, not the event log's.
    let at = crate::session::events::now_ms();
    let split_run_id = uuid::Uuid::new_v4().to_string();

    // 3. Close the parent FIRST. The harness bridge emitted `RunStarted` on
    //    the parent at run start; without this closer the frozen parent's log
    //    would end on a dangling `RunStarted` and `ResumeCoordinator` would
    //    resume it. The run_id need only correlate the closer with the child's
    //    opener — the resume scan is positional.
    session
        .emit_batch(
            parent_session_id,
            vec![SessionEvent::RunFinished {
                run_id: split_run_id.clone(),
                outcome: crate::session::events::RunOutcome::Completed,
                at,
            }],
            None,
        )
        .await
        .map_err(|e| SplitError::Failed(anyhow::anyhow!("emit parent RunFinished: {e}")))?;

    // 4. Seed the child in ONE transaction: fork marker, summary, verbatim
    //    tail, and the open run — so a crash *after* the split is detected
    //    against the child, the live epoch the run actually continues on.
    let tail = &events[tail_start..];
    let mut child_batch = Vec::with_capacity(tail.len() + 3);
    child_batch.push(SessionEvent::SessionForked {
        parent_session_id: parent_session_id.to_key_string(),
        at,
    });
    child_batch.push(build_summary_event(summary_text, at));
    child_batch.extend(tail.iter().map(|record| record.event.clone()));
    child_batch.push(SessionEvent::RunStarted {
        run_id: split_run_id,
        at,
        // The parent's open run, verbatim. The in-memory `RunRequest` does
        // carry the same facts for the LIVE continuation — but a resume
        // after a crash has no `RunRequest`; it has only this marker, and
        // reads its project root and envelope from nowhere else.
        project_root: open_run.project_root,
        envelope: open_run.envelope,
    });
    session
        .emit_batch(&child, child_batch, None)
        .await
        .map_err(|e| SplitError::Failed(anyhow::anyhow!("seed child: {e}")))?;

    // 5. Routing LAST. From here the log is authoritative: the parent's run is
    //    closed and the child's is open, so a refused registration is NOT an
    //    `Err` — returning one would send the caller back to compact-to-fit
    //    on a parent whose run is already closed, while a child with an open
    //    `RunStarted` waits to be resumed as well. Log it; the boot heal
    //    registers the child (log leads, routing follows). Until then inbound
    //    routing still resolves to the parent.
    //
    //    Retire what belonged to the superseded epoch only AFTER a successful
    //    registration: registration is what makes the parent superseded, and
    //    the `/btw` side session keyed to the parent's exact epoch is still
    //    live and reachable behind a registration that did not happen.
    //    Nobody asked for this split, so without the retire the orphan would
    //    be created by the system and reported to no one.
    match epoch_registrar.register_epoch(&child).await {
        Ok(()) => epoch_registrar.retire_superseded(parent_session_id).await,
        Err(e) => tracing::error!(
            target: "context_budget",
            parent = ?parent_session_id,
            child = ?child,
            error = %e,
            "session split committed but epoch registration failed; \
             routing resolves to the parent until the boot heal",
        ),
    }

    Ok(SplitOutcome {
        child_session_id: child,
    })
}

/// Wrap a summary string in a `SessionEvent::SystemMessage` stamped `at` —
/// the one instant the whole child batch carries.
fn build_summary_event(summary: String, at: crate::session::events::Timestamp) -> SessionEvent {
    SessionEvent::SystemMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: format!("{SUMMARY_MARKER}\n{summary}"),
        at,
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
    use crate::routing::session_key::SessionKey;
    use crate::session::events::{
        now_ms, EventSeq, MessageContent, Retire, RunEnvelopeSnapshot, RunOutcome,
        SessionEventRecord,
    };
    use crate::session::service::{SessionError, SessionHandle, SessionService};
    use crate::sync_primitives::Arc as AlephArc;

    /// One trace shared by every fake in a test, so the ORDER in which the
    /// split touches the session service and the registrar is a single
    /// vector — the thing the batch-order test asserts.
    type Trace = Arc<Mutex<Vec<String>>>;

    /// One recorded `emit_batch` call: target session, the rows in batch
    /// order, and the retire the batch carried.
    type RecordedBatch = (SessionId, Vec<SessionEvent>, Option<Retire>);

    // -------------------------------------------------------------------------
    // Fake SessionService
    // -------------------------------------------------------------------------

    struct RecordingSessionService {
        batches: Mutex<Vec<RecordedBatch>>,
        next_seq: Mutex<EventSeq>,
        trace: Trace,
        /// When `Some(n)`, the n-th `emit_batch` call (0-based) fails with
        /// `SessionError::Storage` — the in-process failure the split's
        /// caller falls back from.
        fail_batch_at: Option<usize>,
    }

    impl RecordingSessionService {
        fn new() -> Arc<Self> {
            Self::with_trace(Arc::new(Mutex::new(vec![])))
        }

        fn with_trace(trace: Trace) -> Arc<Self> {
            Arc::new(Self {
                batches: Mutex::new(vec![]),
                next_seq: Mutex::new(1),
                trace,
                fail_batch_at: None,
            })
        }

        fn failing_batch_at(n: usize) -> Arc<Self> {
            Arc::new(Self {
                batches: Mutex::new(vec![]),
                next_seq: Mutex::new(1),
                trace: Arc::new(Mutex::new(vec![])),
                fail_batch_at: Some(n),
            })
        }

        async fn batches(&self) -> Vec<RecordedBatch> {
            self.batches.lock().await.clone()
        }

        /// Every row that reached the service, flattened in commit order.
        async fn emitted(&self) -> Vec<(SessionId, SessionEvent)> {
            self.batches
                .lock()
                .await
                .iter()
                .flat_map(|(id, events, _)| events.iter().map(|e| (id.clone(), e.clone())))
                .collect()
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

        /// A batch of one, as the production service does — so a split that
        /// regressed to per-row `emit_event` would still show up in
        /// `batches()` as N batches of one, not vanish from the record.
        async fn emit_event(
            &self,
            id: &SessionId,
            event: SessionEvent,
        ) -> Result<EventSeq, SessionError> {
            let seqs = self.emit_batch(id, vec![event], None).await?;
            seqs.first()
                .copied()
                .ok_or_else(|| SessionError::Other("one event in, no seq out".into()))
        }

        async fn emit_batch(
            &self,
            id: &SessionId,
            events: Vec<SessionEvent>,
            retire: Option<Retire>,
        ) -> Result<Vec<EventSeq>, SessionError> {
            let call_index = self.batches.lock().await.len();
            if self.fail_batch_at == Some(call_index) {
                self.trace
                    .lock()
                    .await
                    .push(format!("batch_failed:{}", id.to_key_string()));
                return Err(SessionError::Storage("injected batch failure".into()));
            }
            let mut seq = self.next_seq.lock().await;
            let seqs: Vec<EventSeq> = (0..events.len() as EventSeq).map(|i| *seq + i).collect();
            *seq += events.len() as EventSeq;
            self.trace
                .lock()
                .await
                .push(format!("batch:{}:{}", id.to_key_string(), events.len()));
            self.batches.lock().await.push((id.clone(), events, retire));
            Ok(seqs)
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
        retired: Mutex<Vec<SessionId>>,
        trace: Trace,
        /// When true, `register_epoch` refuses — the routing-table write that
        /// the split must survive (the log leads; the boot heal follows).
        refuse: bool,
    }

    impl RecordingRegistrar {
        fn new() -> Arc<Self> {
            Self::with_trace(Arc::new(Mutex::new(vec![])))
        }

        fn with_trace(trace: Trace) -> Arc<Self> {
            Arc::new(Self {
                registered: Mutex::new(vec![]),
                retired: Mutex::new(vec![]),
                trace,
                refuse: false,
            })
        }

        fn refusing() -> Arc<Self> {
            Arc::new(Self {
                registered: Mutex::new(vec![]),
                retired: Mutex::new(vec![]),
                trace: Arc::new(Mutex::new(vec![])),
                refuse: true,
            })
        }

        async fn keys(&self) -> Vec<SessionId> {
            self.registered.lock().await.clone()
        }

        async fn retired(&self) -> Vec<SessionId> {
            self.retired.lock().await.clone()
        }
    }

    #[async_trait]
    impl crate::session::epoch_registrar::SessionEpochRegistrar for RecordingRegistrar {
        async fn register_epoch(&self, key: &SessionId) -> anyhow::Result<()> {
            self.trace
                .lock()
                .await
                .push(format!("register_epoch:{}", key.to_key_string()));
            if self.refuse {
                anyhow::bail!("registrar deliberately refuses");
            }
            self.registered.lock().await.push(key.clone());
            Ok(())
        }

        async fn retire_superseded(&self, superseded: &SessionId) {
            self.trace
                .lock()
                .await
                .push(format!("retire_superseded:{}", superseded.to_key_string()));
            self.retired.lock().await.push(superseded.clone());
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

    /// The envelope the parent's open run carries in every fixture below —
    /// distinctive values so the child-opener test can tell "inherited" from
    /// "defaulted".
    fn parent_envelope() -> RunEnvelopeSnapshot {
        RunEnvelopeSnapshot {
            exec_tier: Some("ask".to_string()),
            model: Some("m-parent".to_string()),
            allowed_tools: Some(vec!["grep".to_string()]),
            ..RunEnvelopeSnapshot::default()
        }
    }

    const PARENT_ROOT: &str = "/parent/project";

    fn run_started_record(seq: EventSeq, run_id: &str) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event: SessionEvent::RunStarted {
                run_id: run_id.to_string(),
                at: now_ms(),
                project_root: Some(PARENT_ROOT.to_string()),
                envelope: Some(parent_envelope()),
            },
            created_at_ms: now_ms(),
        }
    }

    fn run_finished_record(seq: EventSeq, run_id: &str) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event: SessionEvent::RunFinished {
                run_id: run_id.to_string(),
                outcome: RunOutcome::Completed,
                at: now_ms(),
            },
            created_at_ms: now_ms(),
        }
    }

    /// A parent log the harness bridge would have produced: the run's opener
    /// first, then the user messages. Every fixture that expects the split to
    /// SUCCEED starts from this — the child inherits the opener's envelope,
    /// so a parent without one is refused, not seeded blind.
    fn parent_log(messages: &[&str]) -> Vec<SessionEventRecord> {
        std::iter::once(run_started_record(1, "run-parent"))
            .chain(
                messages
                    .iter()
                    .enumerate()
                    .map(|(i, text)| user_record(i as EventSeq + 2, text)),
            )
            .collect()
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
    // Test 2: the split is two batches in a decided order, routing last
    // -------------------------------------------------------------------------

    /// Parent `[RunFinished{Completed}]` FIRST, child `[SessionForked,
    /// SystemMessage, tail…, RunStarted]` SECOND, `register_epoch` THIRD.
    ///
    /// The order is the crash contract, not a style choice: the boot heal
    /// (`ProjectionReconciler::heal_split_epochs`) reads "a `SessionForked`
    /// on the child" as "the parent is already closed". Child-first would
    /// open a window where BOTH sessions reduce to `Interrupted` and both
    /// get resumed; epoch-first would let routing point at a child whose log
    /// does not exist yet.
    #[tokio::test]
    async fn split_commits_parent_then_child_then_registers_the_epoch() {
        let parent = SessionKey::Main {
            agent_id: "agent-a".into(),
            main_key: "main".into(),
            epoch: 0,
        };
        let child = parent.with_next_epoch();
        let events = parent_log(&["pre-tail 1", "pre-tail 2", "fresh tail"]);
        let trace = Arc::new(Mutex::new(vec![]));
        let session = RecordingSessionService::with_trace(trace.clone());
        let registrar = RecordingRegistrar::with_trace(trace.clone());
        let compactor = ContextCompactor::new(
            AlephArc::new(MockProvider::new("S")),
            CompactorConfig::default(),
        );

        perform_session_split(
            session.as_ref(),
            registrar.as_ref(),
            &compactor,
            &parent,
            &events,
            3,
        )
        .await
        .unwrap();

        assert_eq!(
            *trace.lock().await,
            vec![
                format!("batch:{}:1", parent.to_key_string()),
                format!("batch:{}:4", child.to_key_string()),
                format!("register_epoch:{}", child.to_key_string()),
                format!("retire_superseded:{}", parent.to_key_string()),
            ]
        );
        let batches = session.batches().await;
        assert!(matches!(
            batches[0].1.as_slice(),
            [SessionEvent::RunFinished {
                outcome: RunOutcome::Completed,
                ..
            }]
        ));
        assert!(matches!(
            batches[1].1.as_slice(),
            [
                SessionEvent::SessionForked { .. },
                SessionEvent::SystemMessage { .. },
                SessionEvent::UserMessage { .. },
                SessionEvent::RunStarted { .. },
            ]
        ));
        assert!(batches.iter().all(|(_, _, retire)| retire.is_none()));
    }

    // -------------------------------------------------------------------------
    // Test 3: the child batch's content — fork marker, summary, verbatim tail
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn split_seeds_child_with_forked_summary_and_fresh_tail() {
        // Parent: Main session at epoch 0.
        let parent = SessionKey::Main {
            agent_id: "agent-a".to_string(),
            main_key: "main".to_string(),
            epoch: 0,
        };
        let expected_child = parent.with_next_epoch();

        // Build events: the opener + 2 pre-tail + 1 fresh tail.
        let events = parent_log(&[
            "pre-tail message 1",
            "pre-tail message 2",
            "fresh tail message",
        ]);
        let tail_start = 3; // events[..3] summarized; events[3..] copied verbatim

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

        // 2. Registrar received the child key, and the parent was retired.
        assert_eq!(registrar.keys().await, vec![expected_child.clone()]);
        assert_eq!(registrar.retired().await, vec![parent.clone()]);

        // 3. Exactly two batches: the parent's closer, then the child's seed.
        let batches = session.batches().await;
        assert_eq!(batches.len(), 2, "one parent batch + one child batch");
        let (parent_target, parent_rows, _) = &batches[0];
        let (child_target, child_rows, _) = &batches[1];
        assert_eq!(parent_target, &parent);
        assert_eq!(child_target, &expected_child);

        // Parent batch: the one closer, `Completed` — the frozen parent must
        // never read as an interrupted run.
        match parent_rows.as_slice() {
            [SessionEvent::RunFinished { outcome, .. }] => {
                assert_eq!(*outcome, RunOutcome::Completed);
            }
            other => panic!("expected [RunFinished] on the parent, got {other:?}"),
        }

        // Child batch, in order:
        //    [0] SessionForked        (names the parent)
        //    [1] SystemMessage        (the summary)
        //    [2] verbatim fresh tail
        //    [3] RunStarted           (a crash after the split is detected
        //                              against the live epoch)
        assert_eq!(
            child_rows.len(),
            4,
            "forked + summary + 1 tail + RunStarted, got {child_rows:?}"
        );
        match &child_rows[0] {
            SessionEvent::SessionForked {
                parent_session_id, ..
            } => {
                assert_eq!(parent_session_id, &parent.to_key_string());
            }
            other => panic!("expected SessionForked, got {other:?}"),
        }
        match &child_rows[1] {
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
        match &child_rows[2] {
            SessionEvent::UserMessage { content, .. } => {
                assert_eq!(content.text, "fresh tail message");
            }
            other => panic!("expected UserMessage (fresh tail), got {other:?}"),
        }
        match &child_rows[3] {
            SessionEvent::RunStarted { run_id, .. } => {
                // The closer and the opener correlate by run_id.
                let SessionEvent::RunFinished {
                    run_id: closer_id, ..
                } = &parent_rows[0]
                else {
                    unreachable!("asserted above");
                };
                assert_eq!(
                    run_id, closer_id,
                    "split run_id must correlate both markers"
                );
            }
            other => panic!("expected RunStarted on child, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------------
    // The child's opener inherits the parent's run envelope — the crash path
    // -------------------------------------------------------------------------

    /// A crash after a split is resumed against the CHILD, and a resume
    /// replays whatever the child's opener froze. An opener written with
    /// `envelope: None` resumes unsnapshotted — every knob replaced by
    /// today's value, the skill scope gone — so the child's `RunStarted`
    /// must carry the parent's open run's envelope and project root, read
    /// through the service the batch was handed to.
    #[tokio::test]
    async fn the_child_opener_carries_the_parents_envelope_and_project_root() {
        let parent = SessionKey::Main {
            agent_id: "agent-a".into(),
            main_key: "main".into(),
            epoch: 0,
        };
        let events = parent_log(&["pre-tail", "fresh tail"]);
        let session = RecordingSessionService::new();
        let registrar = RecordingRegistrar::new();
        let compactor = ContextCompactor::new(
            AlephArc::new(MockProvider::new("S")),
            CompactorConfig::default(),
        );

        perform_session_split(
            session.as_ref(),
            registrar.as_ref(),
            &compactor,
            &parent,
            &events,
            2,
        )
        .await
        .expect("split should succeed");

        let child = parent.with_next_epoch();
        let openers: Vec<(Option<String>, Option<RunEnvelopeSnapshot>)> = session
            .emitted()
            .await
            .into_iter()
            .filter(|(target, _)| *target == child)
            .filter_map(|(_, event)| match event {
                SessionEvent::RunStarted {
                    project_root,
                    envelope,
                    ..
                } => Some((project_root, envelope)),
                _ => None,
            })
            .collect();
        assert_eq!(
            openers,
            vec![(Some(PARENT_ROOT.to_string()), Some(parent_envelope()))],
            "the child's opener must be a clone of the parent's open run's \
             envelope and project root, not a blank marker"
        );
    }

    /// No open `RunStarted` in the parent log ⇒ nothing to inherit ⇒ the split
    /// is REFUSED before either batch is written: the parent keeps its log,
    /// routing never learns a child, and the caller falls back to
    /// compact-to-fit. Two shapes of "no open run": no marker at all, and a
    /// run the log already closed.
    #[tokio::test]
    async fn a_parent_without_an_open_run_is_refused_before_any_batch() {
        let parent = SessionKey::Main {
            agent_id: "agent-a".into(),
            main_key: "main".into(),
            epoch: 0,
        };
        let no_marker = vec![user_record(1, "pre-tail"), user_record(2, "fresh tail")];
        let closed_run = vec![
            run_started_record(1, "run-parent"),
            user_record(2, "pre-tail"),
            run_finished_record(3, "run-parent"),
            user_record(4, "fresh tail"),
        ];
        for (label, events, tail_start) in [
            ("no marker at all", no_marker, 1),
            ("a run the log already closed", closed_run, 3),
        ] {
            let session = RecordingSessionService::new();
            let registrar = RecordingRegistrar::new();
            let compactor = ContextCompactor::new(
                AlephArc::new(MockProvider::new("S")),
                CompactorConfig::default(),
            );

            let result = perform_session_split(
                session.as_ref(),
                registrar.as_ref(),
                &compactor,
                &parent,
                &events,
                tail_start,
            )
            .await;

            assert!(
                matches!(result, Err(SplitError::NoOpenRun)),
                "{label}: expected NoOpenRun, got {result:?}"
            );
            assert!(
                session.batches().await.is_empty(),
                "{label}: a refused split must write nothing — not even the parent closer"
            );
            assert!(
                registrar.keys().await.is_empty(),
                "{label}: routing must never learn a child"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Test 4: routing is not fatal — the log leads, the boot heal follows
    // -------------------------------------------------------------------------

    /// Once both batches are committed the split HAS happened: the parent's
    /// run is closed and the child's is open. Failing the split here would
    /// send the caller back to a parent whose run is already closed while a
    /// child with an open `RunStarted` waits to be resumed twice. So a refused
    /// `register_epoch` is logged and healed at boot, not returned.
    #[tokio::test]
    async fn a_refused_epoch_registration_is_not_a_failed_split() {
        let parent = SessionKey::Main {
            agent_id: "agent-a".into(),
            main_key: "main".into(),
            epoch: 0,
        };
        let events = parent_log(&["pre-tail", "fresh tail"]);
        let session = RecordingSessionService::new();
        let registrar = RecordingRegistrar::refusing();
        let compactor = ContextCompactor::new(
            AlephArc::new(MockProvider::new("S")),
            CompactorConfig::default(),
        );

        let outcome = perform_session_split(
            session.as_ref(),
            registrar.as_ref(),
            &compactor,
            &parent,
            &events,
            2,
        )
        .await
        .expect("both batches committed: the split succeeded even though routing refused");

        assert_eq!(outcome.child_session_id, parent.with_next_epoch());
        assert_eq!(
            session.batches().await.len(),
            2,
            "both batches still landed"
        );
        assert!(
            registrar.keys().await.is_empty(),
            "the refusal really refused"
        );
        assert!(
            registrar.retired().await.is_empty(),
            "nothing is retired behind a registration that did not happen — \
             the parent's side session is still live and still reachable"
        );
    }

    // -------------------------------------------------------------------------
    // Test 5: an in-process batch failure is an Err, and nothing later runs
    // -------------------------------------------------------------------------

    /// The parent batch refused ⇒ `Err`, no child rows, no registration: the
    /// caller falls back to compact-to-fit on a parent whose run is still
    /// open, exactly as if the split had never been attempted.
    #[tokio::test]
    async fn a_refused_parent_batch_fails_the_split_before_anything_else() {
        let parent = SessionKey::Main {
            agent_id: "agent-a".into(),
            main_key: "main".into(),
            epoch: 0,
        };
        let events = parent_log(&["pre-tail", "fresh tail"]);
        let session = RecordingSessionService::failing_batch_at(0);
        let registrar = RecordingRegistrar::new();
        let compactor = ContextCompactor::new(
            AlephArc::new(MockProvider::new("S")),
            CompactorConfig::default(),
        );

        let result = perform_session_split(
            session.as_ref(),
            registrar.as_ref(),
            &compactor,
            &parent,
            &events,
            2,
        )
        .await;

        assert!(
            matches!(result, Err(SplitError::Failed(_))),
            "expected Failed, got {result:?}"
        );
        assert!(session.batches().await.is_empty(), "nothing was committed");
        assert!(
            registrar.keys().await.is_empty(),
            "routing never learned a child"
        );
    }

    /// The child batch refused ⇒ `Err` too (the caller's fallback contract),
    /// and routing is NOT registered: a child the log does not hold must never
    /// become the routing target. The parent closer that already landed is the
    /// accepted torn state — `FinishWithoutStart` when the run's own closer
    /// follows it, REPORT class, readable and counted.
    #[tokio::test]
    async fn a_refused_child_batch_fails_the_split_without_registering() {
        let parent = SessionKey::Main {
            agent_id: "agent-a".into(),
            main_key: "main".into(),
            epoch: 0,
        };
        let events = parent_log(&["pre-tail", "fresh tail"]);
        let session = RecordingSessionService::failing_batch_at(1);
        let registrar = RecordingRegistrar::new();
        let compactor = ContextCompactor::new(
            AlephArc::new(MockProvider::new("S")),
            CompactorConfig::default(),
        );

        let result = perform_session_split(
            session.as_ref(),
            registrar.as_ref(),
            &compactor,
            &parent,
            &events,
            2,
        )
        .await;

        assert!(
            matches!(result, Err(SplitError::Failed(_))),
            "expected Failed, got {result:?}"
        );
        let batches = session.batches().await;
        assert_eq!(batches.len(), 1, "only the parent closer landed");
        assert_eq!(batches[0].0, parent);
        assert!(
            registrar.keys().await.is_empty(),
            "routing never learned a child"
        );
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
        let events = parent_log(&["only message"]);

        let session = RecordingSessionService::new();
        let registrar = RecordingRegistrar::new();
        let provider = AlephArc::new(MockProvider::new("summary"));
        let compactor = ContextCompactor::new(provider, CompactorConfig::default());

        // tail_start = 5 ≫ events.len() = 2.
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
