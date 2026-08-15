//! User-driven `/compact` — summarize-and-retire over the session event log.
//!
//! # Why this module exists
//!
//! `/compact` (and its `/compress` alias, the `session_compact` tool, and the
//! `session.compact` RPC) used to call `SessionStore::compact(KeepLastN{50})`,
//! which deletes rows from the **`messages` table**. That table is a read
//! projection for the Panel (`gateway::session_projector`: "`session_events` is
//! the single source of truth … the `messages` table is an *eventually
//! consistent* read projection"), while the agent's prompt is rebuilt every
//! turn from `session_events` (`SessionService::get_events` →
//! `harness::agent::prompt::build_prompt`). The two never met: `/compact`
//! permanently deleted the user's visible scrollback and reduced the next
//! prompt by exactly zero tokens, while reporting a fabricated
//! `deleted × 50` token saving.
//!
//! This module compacts the thing the prompt is actually built from.
//!
//! # How it works (pi `CompactionEntry` / codex replacement-history parity)
//!
//! 1. Walk the live event log newest-first, accumulating per-event token
//!    estimates until `keep_tokens` is filled — a **token** boundary, not a
//!    message count. Fifty huge tool results still overflow the window; fifty
//!    one-line turns throw away context for nothing.
//! 2. Snap the cut so it never orphans a `tool_result` from its `tool_use`, and
//!    never lands inside an open run's `RunStarted … RunFinished` span.
//! 3. Summarize the dropped span through the same [`ContextCompactor`] the
//!    automatic path uses (LLM, deterministic-truncation fallback), optionally
//!    steered by the user's own `/compact <instructions>` directive.
//! 4. Append the summary as a `SystemMessage`, then a `CompactionPerformed`
//!    checkpoint — finally giving that long-declared, never-produced event a
//!    producer.
//! 5. **Soft-retire** the compacted prefix via
//!    [`SessionEventStore::retire_through`]. The rows survive (the log stays
//!    append-only, `/undo` and export are unaffected) and the BM25 mirror is
//!    deliberately kept, so `recall_events` can still surface a detail the
//!    summary abstracted away.
//!
//! Nothing is deleted, anywhere. The Panel keeps the full scrollback and gains
//! a visible summary row; the agent's next prompt starts from the summary.
//!
//! # One at a time, over a span that is still there
//!
//! The sequence runs inside a per-session [`CompactionBracket`], and the span it
//! summarized is re-read before anything is written. Without the first, two
//! `/compact`s racing from different surfaces each append a summary the other
//! never retires; without the second, a `chat.clear` landing during the
//! summarize await is undone by a summary of the turns it just erased.
//!
//! # Where the summary sits
//!
//! The log is append-only, so the summary lands **after** the kept tail rather
//! than before it. This is deliberate rather than a compromise waiting to be
//! fixed: the alternative — retiring everything and re-emitting the tail — would
//! duplicate every kept turn into the `messages` projection, i.e. show the user
//! their last ten turns twice. The summary therefore carries an explicit framing
//! line saying what it covers, and rides at the position with the most attention.

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::compact::compactor::{deterministic_truncation, ContextCompactor};
use crate::context::compact::preserve::SUMMARY_MARKER;
use crate::context::compact::session_split::event_to_message;
use crate::context::compact::summary_utils::{
    cap_summary_lines, clamp_start_to_budget, latest_user_task, MAX_SUMMARY_LINES,
    SUMMARIZER_INPUT_TOKEN_BUDGET,
};
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::session::service::{SessionId, SessionService};
use crate::session::store::SessionEventStore;
use crate::sync_primitives::{Arc, Mutex};

/// Default token budget for the tail kept verbatim by a manual compaction —
/// pi's `keepRecentTokens` (its default is 20 000 too).
///
/// A **token** budget, not a message count: the old `KeepLastN{50}` was wrong
/// in both directions — fifty `file_read` results can be 200k tokens, and fifty
/// one-line turns are a few hundred.
pub const DEFAULT_KEEP_TOKENS: usize = 20_000;

/// Floor on the keep budget. Below this a compaction would drop the turn that
/// asked for it, so a caller passing an absurdly small budget is clamped rather
/// than obeyed.
const MIN_KEEP_TOKENS: usize = 2_000;

/// Ceiling on the keep budget. Above this `/compact` is doing nothing useful
/// (the whole point is to shrink the prompt), so an absurd value is clamped
/// instead of silently turning the command into a no-op.
const MAX_KEEP_TOKENS: usize = 400_000;

/// Framing line under the marker. The log is append-only so this summary sits
/// *after* the turns it does not cover; saying so plainly costs one line and
/// removes the only real ambiguity of the tail placement.
const SUMMARY_FRAMING: &str = "(Compacted at the user's request. This condenses the earlier turns \
     of this conversation, which are no longer replayed in full. The messages above are the most \
     recent turns, kept verbatim.)";

/// Options for one manual compaction.
#[derive(Debug, Clone, Default)]
pub struct ManualCompactOptions {
    /// The user's `/compact <instructions>` directive, if any. Steers what the
    /// summary keeps (codex / pi / kimi-cli parity).
    pub instructions: Option<String>,
    /// Verbatim tail budget in tokens. `None` — every production caller — takes
    /// the operator setting (`[context_budget] manual_compact_keep_tokens`, else
    /// [`DEFAULT_KEEP_TOKENS`]). Present so the boundary logic can be driven at
    /// its edges without installing the process-wide handle, which is a
    /// `OnceLock` and therefore un-resettable per test.
    pub keep_tokens: Option<usize>,
}

/// What a manual compaction did.
#[derive(Debug, Clone)]
pub struct ManualCompactOutcome {
    /// False when the conversation was already short enough to leave alone.
    pub compacted: bool,
    /// Conversational events folded into the summary.
    pub events_compacted: usize,
    /// Events left in the live conversation (excludes the appended summary).
    pub events_kept: usize,
    /// Estimated prompt tokens the compacted span was costing every turn.
    pub tokens_before: usize,
    /// Estimated prompt tokens the summary costs in its place.
    pub tokens_after: usize,
    /// The summary body (without the marker line), for surfacing to the user.
    pub summary: String,
    /// Why nothing happened, when `compacted` is false.
    pub skipped_reason: Option<String>,
}

impl ManualCompactOutcome {
    fn skipped(events_kept: usize, reason: impl Into<String>) -> Self {
        Self {
            compacted: false,
            events_compacted: 0,
            events_kept,
            tokens_before: 0,
            tokens_after: 0,
            summary: String::new(),
            skipped_reason: Some(reason.into()),
        }
    }

    /// Tokens this compaction removed from every future prompt. Saturating:
    /// a summary longer than what it replaced (possible on a tiny window)
    /// reports zero saved rather than underflowing.
    #[must_use]
    pub const fn tokens_saved(&self) -> usize {
        self.tokens_before.saturating_sub(self.tokens_after)
    }
}

// ---------------------------------------------------------------------------
// Boot-time wiring
// ---------------------------------------------------------------------------

/// Process-wide wiring for user-driven compaction, installed once at daemon
/// boot (`orchestrator_init`).
///
/// Two surfaces reach `/compact` by different routes — the `session_compact`
/// tool (Panel / channel slash commands, and the model itself under R8) and the
/// `session.compact` RPC (TUI `/compress`, CLI `aleph session compact`) — and
/// neither carries a provider. Publishing the summarizer once here is what makes
/// them behave identically (R6: one core, many channels); the alternative was
/// two code paths that would drift the first time either was touched.
///
/// Same shape as the neighbouring `set_global_session_service` /
/// `set_global_session_event_store` handles this module already depends on.
pub struct ManualCompactWiring {
    /// Provider for the side-channel summarization call. `None` degrades to
    /// deterministic truncation — still a real compaction, just a blunt summary.
    pub summarizer: Option<Arc<dyn AiProvider>>,
    /// Operator default for the verbatim tail budget
    /// (`[context_budget] manual_compact_keep_tokens`).
    pub keep_tokens: usize,
}

static MANUAL_WIRING: std::sync::OnceLock<ManualCompactWiring> = std::sync::OnceLock::new();

/// Install the process-wide manual-compaction wiring. Idempotent: a second call
/// is ignored (mirrors `set_global_session_service`).
pub fn install_manual_compaction(wiring: ManualCompactWiring) {
    let _ = MANUAL_WIRING.set(wiring);
}

/// The summarizer provider installed at boot, if any.
#[must_use]
pub fn manual_summarizer() -> Option<Arc<dyn AiProvider>> {
    MANUAL_WIRING.get().and_then(|w| w.summarizer.clone())
}

/// The operator-configured verbatim tail budget, or [`DEFAULT_KEEP_TOKENS`].
#[must_use]
pub fn manual_keep_tokens() -> usize {
    MANUAL_WIRING
        .get()
        .map_or(DEFAULT_KEEP_TOKENS, |w| w.keep_tokens)
}

// ---------------------------------------------------------------------------
// Operation bracket
// ---------------------------------------------------------------------------

/// Session keys with a manual compaction in flight, process-wide.
///
/// A `BTreeSet` rather than a `HashSet` only because `BTreeSet::new` is `const`,
/// which keeps this a plain `static` with no lazy initialisation. It holds one
/// entry per concurrently compacting session, so it is empty almost always.
static IN_FLIGHT: Mutex<std::collections::BTreeSet<String>> =
    Mutex::new(std::collections::BTreeSet::new());

/// RAII claim on one session's manual compaction, released on drop — including
/// on every early return and `?` propagation in [`compact_session`].
///
/// Nothing else serializes this operation. Of the surfaces that reach
/// `/compact`, only the two that run inside a lane-admitted run (the Panel slash
/// fast path and the model's own `session_compact` call) are serialized against
/// each other by the session's busy queue; the `session.compact` RPC — TUI
/// `/compress`, `aleph session compact`, any WS client — rides the Mutate lane,
/// which is a concurrency *semaphore*, not a per-session mutex.
///
/// Two overlapping compactions are not merely wasteful. Both read the same
/// un-retired log, both pay the summarizer, and both append their summary
/// *above* the other's `retire_through` boundary — so neither retires the
/// other's, and every future prompt of that session carries two near-identical
/// `[Context Summary]` blocks until some later compaction sweeps them up.
struct CompactionBracket {
    key: String,
}

impl CompactionBracket {
    /// Claim `session_id`, or `None` when a compaction of it is already running.
    fn try_acquire(session_id: &SessionId) -> Option<Self> {
        let key = session_id.to_key_string();
        let claimed = IN_FLIGHT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone());
        claimed.then_some(Self { key })
    }
}

impl Drop for CompactionBracket {
    fn drop(&mut self) {
        IN_FLIGHT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

// ---------------------------------------------------------------------------
// The compaction itself
// ---------------------------------------------------------------------------

/// Compact `session_id`'s live conversation: summarize everything older than
/// the verbatim tail, record the summary and its checkpoint, and retire the
/// compacted prefix.
///
/// `summarizer` is optional so a deployment with no provider wired still gets a
/// real (deterministic) compaction instead of a silent failure.
///
/// Ordering is fail-safe by construction: the summary and checkpoint are
/// appended **before** the retire. If the retire fails, the log carries a
/// summary and nothing was lost; the reverse order could strip the prefix and
/// then fail to record what it said.
pub async fn compact_session(
    service: &dyn SessionService,
    store: &dyn SessionEventStore,
    summarizer: Option<&ContextCompactor>,
    session_id: &SessionId,
    opts: &ManualCompactOptions,
) -> anyhow::Result<ManualCompactOutcome> {
    // Claim the session before the log is read: a snapshot taken outside the
    // bracket can already describe a log another compaction is about to retire.
    let Some(_bracket) = CompactionBracket::try_acquire(session_id) else {
        // Every other skip reports the live log it left alone. This is the one
        // that never took a snapshot to count, so it reads the log once rather
        // than reporting a fabricated zero.
        let live = service.get_events(session_id, None, None).await?;
        return Ok(ManualCompactOutcome::skipped(
            live.len(),
            "a compaction of this conversation is already in progress",
        ));
    };

    let events = service.get_events(session_id, None, None).await?;
    let keep_tokens = opts
        .keep_tokens
        .unwrap_or_else(manual_keep_tokens)
        .clamp(MIN_KEEP_TOKENS, MAX_KEEP_TOKENS);

    let Some(cut) = select_cut(&events, keep_tokens) else {
        return Ok(ManualCompactOutcome::skipped(
            events.len(),
            "conversation already fits the verbatim tail budget",
        ));
    };

    // Only conversational events carry summarizable content; lifecycle markers
    // ride along in the retired span but contribute nothing to the summary.
    let dropped: Vec<UnifiedMessage> = events[..cut]
        .iter()
        .filter_map(|r| event_to_message(&r.event))
        .collect();
    if dropped.is_empty() {
        return Ok(ManualCompactOutcome::skipped(
            events.len(),
            "nothing summarizable before the verbatim tail",
        ));
    }

    // What the compacted span was actually costing the prompt each turn —
    // measured on the real message bodies, not on the capped summarizer input.
    let tokens_before: usize = dropped
        .iter()
        .map(|m| estimate_tokens_smart(&m.text_content()))
        .sum();

    // Bound the summarizer's own input: keep the NEWEST budget-worth so a huge
    // history cannot overflow the (possibly flash-tier) summarizer's window.
    let elided = clamp_start_to_budget(&dropped, SUMMARIZER_INPUT_TOKEN_BUDGET);
    let summarizer_input = &dropped[elided..];

    // Anchor on the live task from the KEPT tail, exactly like every other
    // drain site. The user's explicit directive outranks it inside the prompt.
    let tail: Vec<UnifiedMessage> = events[cut..]
        .iter()
        .filter_map(|r| event_to_message(&r.event))
        .collect();
    let focus = task_focus(&tail);
    let instructions = opts
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let body = match summarizer {
        Some(compactor) => {
            compactor
                .summarize_slice(summarizer_input, focus.as_deref(), instructions)
                .await?
        }
        // No provider wired: a blunt summary still beats replaying the raw
        // span forever, and it is exactly what the LLM path falls back to.
        None => deterministic_truncation(summarizer_input),
    };
    let body = cap_summary_lines(body, MAX_SUMMARY_LINES);
    let body = if elided > 0 {
        format!("[{elided} earlier events elided from this summary]\n{body}")
    } else {
        body
    };
    if body.trim().is_empty() {
        return Ok(ManualCompactOutcome::skipped(
            events.len(),
            "summarizer produced no usable summary; conversation left intact",
        ));
    }

    let tokens_after = estimate_tokens_smart(&body);
    // A "summary" that is not smaller than what it replaces is pure loss: the
    // structure goes, the token cost stays. Reachable in practice on the
    // deterministic fallback, whose first-line-per-message rendering barely
    // shrinks a span of single-line turns. Refusing here is what makes
    // `tokens_saved() > 0` an invariant of every successful compaction, so the
    // number reported to the user is never zero-or-negative dressed up as a win.
    if tokens_after >= tokens_before {
        return Ok(ManualCompactOutcome::skipped(
            events.len(),
            "the summary would not be smaller than the turns it replaces",
        ));
    }

    let cut_seq = events[cut - 1].seq;
    let from_seq = events[0].seq;

    // The summarize above is a network call measured in seconds, and the bracket
    // only excludes another compaction — `chat.clear` and `chat.rewind` retire
    // live rows the whole time it runs. They go through `retire_from`, which
    // deletes the BM25 mirror precisely so erased turns can never be recalled;
    // appending a summary *of* that span now would hand the model the very turns
    // the user just erased, at the head of every future prompt. So re-read the
    // log and compact only if the summarized span is still exactly there.
    // Growth past the cut is expected and fine — the run that asked for the
    // compaction keeps appending while the summary is being written.
    let live = service.get_events(session_id, None, None).await?;
    let span_intact = live.len() >= cut
        && live[..cut]
            .iter()
            .zip(&events[..cut])
            .all(|(now, then)| now.seq == then.seq);
    if !span_intact {
        return Ok(ManualCompactOutcome::skipped(
            live.len(),
            "the conversation changed while the summary was being written",
        ));
    }

    let summary_seq = service
        .emit_event(
            session_id,
            SessionEvent::SystemMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: format!("{SUMMARY_MARKER}\n{SUMMARY_FRAMING}\n\n{body}"),
                at: crate::session::events::now_ms(),
            },
        )
        .await?;

    service
        .emit_event(
            session_id,
            SessionEvent::CompactionPerformed {
                from_seq,
                to_seq: cut_seq,
                // The seq of the `SystemMessage` carrying the summary — the one
                // reference that survives every later compaction of this log.
                summary_ref: summary_seq.to_string(),
                at: crate::session::events::now_ms(),
            },
        )
        .await?;

    let retired = match store.retire_through(session_id, cut_seq).await {
        Ok(n) => n,
        Err(e) => {
            // The summary and its checkpoint are already durable. Say so, so a
            // reader of the log does not mistake the extra `[Context Summary]`
            // for a compaction that happened: nothing was lost, but nothing was
            // freed either.
            tracing::warn!(
                ?session_id,
                error = %e,
                "manual compaction: summary recorded but the prefix could not be retired; \
                 context is unchanged",
            );
            return Err(e.into());
        }
    };
    // Tell the prompt-cache watchdog this break was deliberate.
    //
    // Retiring the prefix guarantees the next turns are cache-cold — that is
    // the entire point of the operation. Without this the watchdog counts them
    // as evidence and warns "the stable prefix may have changed" about a
    // change the user just asked for. That is the same false-positive class
    // the armed gate and per-prefix keying were introduced to eliminate, and an
    // alarm that cries wolf after every `/compact` is one operators learn to
    // ignore — it is the only early signal this domain has. Scoped to THIS
    // conversation, never the process-wide reset: only this session's prefix was
    // retired, so silencing the agent's other sessions would hide a real break.
    crate::thinker::prompt_builder::cache_monitor::global_cache_monitor().notify_compaction(Some(
        &crate::thinker::prompt_builder::cache_monitor::cache_scope(
            session_id.agent_id(),
            Some(&session_id.to_key_string()),
        ),
    ));

    tracing::info!(
        ?session_id,
        retired,
        events_compacted = cut,
        tokens_before,
        tokens_after,
        instructed = instructions.is_some(),
        "manual compaction: summarized and retired the conversation prefix",
    );

    Ok(ManualCompactOutcome {
        compacted: true,
        events_compacted: cut,
        events_kept: events.len() - cut,
        tokens_before,
        tokens_after,
        summary: body,
        skipped_reason: None,
    })
}

// ---------------------------------------------------------------------------
// Cut selection
// ---------------------------------------------------------------------------

/// Choose the exclusive end of the compacted prefix, or `None` when there is
/// nothing worth compacting.
///
/// Walks newest-first accumulating per-event token estimates until
/// `keep_tokens` is filled, then applies two safety snaps. Always leaves at
/// least one event on each side of the cut.
fn select_cut(events: &[SessionEventRecord], keep_tokens: usize) -> Option<usize> {
    if events.len() < 2 {
        return None;
    }
    let mut acc = 0usize;
    let mut cut = events.len();
    while cut > 0 {
        let cost = event_to_message(&events[cut - 1].event)
            .map_or(0, |m| estimate_tokens_smart(&m.text_content()));
        // Stop *before* consuming the event that would overflow, but never
        // before keeping at least one: a single oversized final turn must not
        // make the whole conversation compactable.
        if cut < events.len() && acc.saturating_add(cost) > keep_tokens {
            break;
        }
        acc = acc.saturating_add(cost);
        cut -= 1;
    }
    if cut == 0 {
        return None;
    }

    let cut = snap_past_tool_results(events, cut);
    let cut = snap_out_of_open_run(events, cut);
    (cut > 0 && cut < events.len()).then_some(cut)
}

/// Advance `cut` past any contiguous run of `ToolResult` / `ToolError` so the
/// kept region never *begins* on a result whose `tool_use` was just retired.
///
/// `build_prompt` already downgrades an orphan result to plain text rather than
/// letting the provider reject it (Anthropic: `tool_result` without a preceding
/// `tool_use`), but producing the orphan in the first place turns a structured
/// result into prose for the rest of the session. Event-level mirror of the
/// compactor's `snap_boundary_forward`, identical to the guard
/// `perform_session_split` applies to its fresh-tail boundary.
fn snap_past_tool_results(events: &[SessionEventRecord], cut: usize) -> usize {
    let mut c = cut;
    while c < events.len()
        && matches!(
            events[c].event,
            SessionEvent::ToolResult { .. } | SessionEvent::ToolError { .. }
        )
    {
        c += 1;
    }
    c
}

/// Pull `cut` back so it never falls between a `RunStarted` and the
/// `RunFinished` that closes it.
///
/// `load_run_markers` skips retired events, so retiring a `RunStarted` while
/// keeping its `RunFinished` leaves a dangling close that `ResumeCoordinator`
/// reads as a run it never saw start. Both markers must land on the same side
/// of the cut. Pulling back (rather than pushing forward) is the safe
/// direction: it compacts less, never more.
fn snap_out_of_open_run(events: &[SessionEventRecord], cut: usize) -> usize {
    // One pass over the kept tail collects the runs it closes; the prefix scan
    // is then a set lookup rather than a nested search.
    let closed_after_cut: std::collections::HashSet<&str> = events[cut..]
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::RunFinished { run_id, .. } => Some(run_id.as_str()),
            _ => None,
        })
        .collect();
    if closed_after_cut.is_empty() {
        return cut;
    }
    events[..cut]
        .iter()
        .position(|r| {
            matches!(&r.event, SessionEvent::RunStarted { run_id, .. }
                if closed_after_cut.contains(run_id.as_str()))
        })
        .unwrap_or(cut)
}

/// The live-task anchor for a manual compaction, with the compaction command
/// itself filtered out.
///
/// `/compact` arrives through the slash fast path *and* as a model tool call,
/// and in the latter case the user's literal `"/compact …"` turn is the newest
/// user message in the kept tail — so the unfiltered anchor would tell the
/// summarizer that the user's active task is "compact this conversation".
/// The user's actual directive, when they gave one, rides the far stronger
/// `instructions` block instead.
fn task_focus(tail: &[UnifiedMessage]) -> Option<String> {
    latest_user_task(tail).filter(|task| {
        let head = task.trim_start().to_ascii_lowercase();
        !(head.starts_with("/compact") || head.starts_with("/compress"))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, EventSeq, MessageContent, RunOutcome, ToolOutput};

    fn rec(seq: EventSeq, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: now_ms(),
        }
    }

    fn user(seq: EventSeq, text: &str) -> SessionEventRecord {
        rec(
            seq,
            SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: text.to_string(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                synthetic: false,
                author_user_id: None,
                at: now_ms(),
            },
        )
    }

    fn assistant(seq: EventSeq, text: &str) -> SessionEventRecord {
        rec(
            seq,
            SessionEvent::AssistantMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: text.to_string(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                usage: None,
                at: now_ms(),
            },
        )
    }

    fn tool_result(seq: EventSeq, call_id: &str) -> SessionEventRecord {
        rec(
            seq,
            SessionEvent::ToolResult {
                turn_id: uuid::Uuid::new_v4(),
                call_id: call_id.to_string(),
                output: ToolOutput {
                    value: serde_json::json!({"ok": true}),
                    metadata: Default::default(),
                },
                at: now_ms(),
            },
        )
    }

    /// ~1 token per 4 chars, so ~400 chars ≈ 100 tokens. Multi-line on purpose:
    /// real assistant turns and tool results are, and the deterministic
    /// summarizer keeps the first line of each message — a single-line corpus
    /// would make its "summary" the same size as the original.
    fn bulky(seq: EventSeq, tag: &str) -> SessionEventRecord {
        let body: String = (0..10)
            .map(|i| format!("{tag} detail line {i} {}\n", "x".repeat(30)))
            .collect();
        assistant(seq, &body)
    }

    #[test]
    fn short_conversation_is_left_alone() {
        let events = vec![user(1, "hi"), assistant(2, "hello")];
        assert_eq!(select_cut(&events, DEFAULT_KEEP_TOKENS), None);
        assert_eq!(select_cut(&[], DEFAULT_KEEP_TOKENS), None);
    }

    /// Estimated tokens of the kept tail `events[cut..]`.
    fn tail_tokens(events: &[SessionEventRecord], cut: usize) -> usize {
        events[cut..]
            .iter()
            .filter_map(|r| event_to_message(&r.event))
            .map(|m| estimate_tokens_smart(&m.text_content()))
            .sum()
    }

    #[test]
    fn cut_is_token_based_not_message_count() {
        // The property, not a hand-tuned index: a message-count rule keeps a
        // fixed number of events regardless of their size, a token rule keeps
        // what fits. Asserting the budget rather than "expect cut==7" also
        // means the test cannot be silently invalidated by a change to the
        // estimator or the fixture's size.
        let events: Vec<_> = (1..=10).map(|i| bulky(i, "turn")).collect();

        for budget in [200usize, 400, 800] {
            let cut = select_cut(&events, budget).expect("a long conversation must compact");
            assert!(cut > 0 && cut < events.len(), "budget={budget} cut={cut}");
            // The kept tail respects the budget, except that the newest event
            // is always kept even when it alone exceeds it.
            let kept_beyond_newest = tail_tokens(&events, cut + 1);
            assert!(
                kept_beyond_newest <= budget,
                "budget={budget} cut={cut}: kept tail beyond the newest event is \
                 {kept_beyond_newest} tokens"
            );
            // Taking one more event would have blown the budget — i.e. the cut
            // is maximal, not merely legal.
            assert!(
                tail_tokens(&events, cut - 1) > budget,
                "budget={budget} cut={cut}: the cut kept less than it could"
            );
        }

        // Monotonic: a bigger budget keeps more.
        let tight = select_cut(&events, 200).unwrap();
        let loose = select_cut(&events, 800).unwrap();
        assert!(loose < tight, "tight={tight} loose={loose}");
    }

    #[test]
    fn a_single_oversized_final_turn_still_keeps_itself() {
        // The last event alone blows the budget. It must still be kept — the
        // alternative is compacting the entire conversation including the turn
        // the user is looking at.
        let events = vec![user(1, "small"), assistant(2, "b"), bulky(3, "huge")];
        let cut = select_cut(&events, 10).expect("must still compact the older prefix");
        assert_eq!(cut, 2, "the oversized tail event is kept verbatim");
    }

    #[test]
    fn cut_never_orphans_a_tool_result_from_its_call() {
        // Budget lands the raw cut right before a tool-result run; the snap must
        // push past it so the kept region does not open on an orphan.
        let events = vec![
            user(1, "go"),
            bulky(2, "old"),
            assistant(3, "calling"),
            tool_result(4, "c1"),
            tool_result(5, "c2"),
            user(6, "next"),
        ];
        let cut = select_cut(&events, 1).expect("must compact");
        assert!(
            !matches!(
                events[cut].event,
                SessionEvent::ToolResult { .. } | SessionEvent::ToolError { .. }
            ),
            "kept region must not begin on a tool result, cut={cut}"
        );
    }

    #[test]
    fn cut_never_splits_an_open_run_span() {
        // The run spans indices 2..=5 and straddles where the token budget
        // would otherwise cut. Retiring `RunStarted` while keeping its
        // `RunFinished` leaves `load_run_markers` (which skips retired events)
        // reading a run that never started — `ResumeCoordinator`'s input.
        let run = "run-1".to_string();
        let events = vec![
            user(1, "older"),
            bulky(2, "old work"),
            rec(
                3,
                SessionEvent::RunStarted {
                    run_id: run.clone(),
                    at: now_ms(),
                    project_root: None,
                },
            ),
            bulky(4, "in-run A"),
            bulky(5, "in-run B"),
            rec(
                6,
                SessionEvent::RunFinished {
                    run_id: run,
                    outcome: RunOutcome::Completed,
                    at: now_ms(),
                },
            ),
        ];
        // Without the snap the budget-driven cut lands inside the span.
        let raw = {
            let mut acc = 0usize;
            let mut c = events.len();
            while c > 0 {
                let cost = event_to_message(&events[c - 1].event)
                    .map_or(0, |m| estimate_tokens_smart(&m.text_content()));
                if c < events.len() && acc.saturating_add(cost) > 1 {
                    break;
                }
                acc = acc.saturating_add(cost);
                c -= 1;
            }
            c
        };
        assert!(
            raw > 2,
            "precondition: the raw cut splits the span, raw={raw}"
        );

        let cut = select_cut(&events, 1).expect("must compact");
        assert_invariant_run_markers_balanced(&events, cut);
        assert!(cut <= 2, "cut must be pulled out of the span, cut={cut}");
    }

    /// Every `RunStarted` in the retired prefix must have its `RunFinished`
    /// there too — the property `snap_out_of_open_run` exists to hold, stated
    /// without depending on exact indices.
    fn assert_invariant_run_markers_balanced(events: &[SessionEventRecord], cut: usize) {
        let closed_in_tail: Vec<&str> = events[cut..]
            .iter()
            .filter_map(|r| match &r.event {
                SessionEvent::RunFinished { run_id, .. } => Some(run_id.as_str()),
                _ => None,
            })
            .collect();
        for record in &events[..cut] {
            if let SessionEvent::RunStarted { run_id, .. } = &record.event {
                assert!(
                    !closed_in_tail.contains(&run_id.as_str()),
                    "run {run_id} would be retired while its close stays live"
                );
            }
        }
    }

    #[test]
    fn a_run_fully_inside_the_prefix_needs_no_snap() {
        // The guard must not over-fire: when BOTH markers land in the retired
        // prefix they are balanced, so the cut stays where the budget put it.
        let run = "run-1".to_string();
        let events = vec![
            rec(
                1,
                SessionEvent::RunStarted {
                    run_id: run.clone(),
                    at: now_ms(),
                    project_root: None,
                },
            ),
            bulky(2, "work"),
            rec(
                3,
                SessionEvent::RunFinished {
                    run_id: run,
                    outcome: RunOutcome::Completed,
                    at: now_ms(),
                },
            ),
            bulky(4, "after"),
            user(5, "next"),
        ];
        let cut = select_cut(&events, 1).expect("must compact");
        assert!(
            cut >= 3,
            "a balanced run must not pull the cut back, cut={cut}"
        );
        assert_invariant_run_markers_balanced(&events, cut);
    }

    #[test]
    fn skipped_outcome_reports_zero_saved() {
        let out = ManualCompactOutcome::skipped(3, "nothing to do");
        assert!(!out.compacted);
        assert_eq!(out.tokens_saved(), 0);
        assert!(out.skipped_reason.is_some());
    }

    #[test]
    fn tokens_saved_never_underflows() {
        let out = ManualCompactOutcome {
            compacted: true,
            events_compacted: 2,
            events_kept: 1,
            tokens_before: 10,
            tokens_after: 40,
            summary: "verbose".into(),
            skipped_reason: None,
        };
        assert_eq!(out.tokens_saved(), 0);
    }

    #[test]
    fn keep_budget_is_clamped_to_a_sane_band() {
        // Directly exercised through the constants the caller clamps with, so a
        // hostile `keep_tokens: 0` cannot compact away the current turn and a
        // `usize::MAX` cannot silently disable the command.
        assert_eq!(
            0usize.clamp(MIN_KEEP_TOKENS, MAX_KEEP_TOKENS),
            MIN_KEEP_TOKENS
        );
        assert_eq!(
            usize::MAX.clamp(MIN_KEEP_TOKENS, MAX_KEEP_TOKENS),
            MAX_KEEP_TOKENS
        );
    }

    // -----------------------------------------------------------------------
    // End-to-end: the wire this whole module exists to connect
    // -----------------------------------------------------------------------

    /// A `SessionService` backed by a real `SqliteEventStore`, so `get_events`
    /// observes retirement exactly as production does. The old `/compact`
    /// looked correct against a mock precisely because nothing checked that the
    /// thing the prompt reads had actually changed.
    struct StoreBackedService {
        store: std::sync::Arc<crate::session::store::SqliteEventStore>,
        next_seq: tokio::sync::Mutex<EventSeq>,
    }

    #[async_trait::async_trait]
    impl SessionService for StoreBackedService {
        async fn attach(
            &self,
            id: SessionId,
        ) -> Result<crate::session::service::SessionHandle, crate::session::service::SessionError>
        {
            Ok(crate::session::service::SessionHandle { id, head_seq: 0 })
        }
        async fn get_events(
            &self,
            id: &SessionId,
            from: Option<EventSeq>,
            to: Option<EventSeq>,
        ) -> Result<Vec<SessionEventRecord>, crate::session::service::SessionError> {
            self.store.load_events_range(id, from, to).await
        }
        async fn emit_event(
            &self,
            id: &SessionId,
            event: SessionEvent,
        ) -> Result<EventSeq, crate::session::service::SessionError> {
            let mut guard = self.next_seq.lock().await;
            let seq = *guard;
            *guard += 1;
            self.store.append(id, seq, &event, now_ms()).await?;
            Ok(seq)
        }
        async fn subscribe(
            &self,
            _id: &SessionId,
        ) -> Result<
            tokio::sync::broadcast::Receiver<SessionEventRecord>,
            crate::session::service::SessionError,
        > {
            let (_tx, rx) = tokio::sync::broadcast::channel(1);
            Ok(rx)
        }
        async fn wake(
            &self,
            id: &SessionId,
        ) -> Result<crate::session::service::SessionHandle, crate::session::service::SessionError>
        {
            self.attach(id.clone()).await
        }
        async fn detach(
            &self,
            _id: &SessionId,
        ) -> Result<(), crate::session::service::SessionError> {
            Ok(())
        }
    }

    async fn seeded_session(
        turns: usize,
    ) -> (
        StoreBackedService,
        std::sync::Arc<crate::session::store::SqliteEventStore>,
        SessionId,
    ) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::session::store::migrate_add_session_events(&conn).unwrap();
        let store = std::sync::Arc::new(crate::session::store::SqliteEventStore::new(conn));
        let service = StoreBackedService {
            store: std::sync::Arc::clone(&store),
            next_seq: tokio::sync::Mutex::new(1),
        };
        let sid: SessionId = crate::routing::session_key::SessionKey::ephemeral("manual-compact");
        for i in 0..turns {
            service
                .emit_event(
                    &sid,
                    user(
                        0,
                        &format!("request {i} about the migration {}", "q".repeat(400)),
                    )
                    .event,
                )
                .await
                .unwrap();
            service
                .emit_event(&sid, bulky(0, &format!("answer {i}")).event)
                .await
                .unwrap();
        }
        (service, store, sid)
    }

    #[tokio::test]
    async fn compaction_actually_shrinks_what_the_prompt_is_rebuilt_from() {
        // THE regression test for this whole module: the old `/compact` deleted
        // rows from the `messages` read projection while the prompt was rebuilt
        // from `session_events`, so it freed nothing. Assert against
        // `get_events` — the exact call `harness::agent::think` makes.
        let (service, store, sid) = seeded_session(40).await;
        let before = service.get_events(&sid, None, None).await.unwrap();
        assert_eq!(before.len(), 80);

        // No summarizer wired: the deterministic fallback still produces a real
        // compaction, which is the floor this feature must never drop below.
        let outcome = compact_session(
            &service,
            store.as_ref(),
            None,
            &sid,
            &ManualCompactOptions {
                instructions: None,
                keep_tokens: Some(MIN_KEEP_TOKENS),
            },
        )
        .await
        .unwrap();
        assert!(outcome.compacted, "{:?}", outcome.skipped_reason);
        assert!(outcome.tokens_saved() > 0);

        let after = service.get_events(&sid, None, None).await.unwrap();
        assert!(
            after.len() < before.len(),
            "the live event log must shrink: {} -> {}",
            before.len(),
            after.len()
        );
        // The summary and its checkpoint survive (they were appended after the
        // retired prefix), and the checkpoint points at the summary's seq.
        let summary_seq = after
            .iter()
            .find_map(|r| match &r.event {
                SessionEvent::SystemMessage { content, .. }
                    if content.starts_with(SUMMARY_MARKER) =>
                {
                    Some(r.seq)
                }
                _ => None,
            })
            .expect("the summary must be live after compaction");
        let checkpoint = after
            .iter()
            .find_map(|r| match &r.event {
                SessionEvent::CompactionPerformed {
                    to_seq,
                    summary_ref,
                    ..
                } => Some((*to_seq, summary_ref.clone())),
                _ => None,
            })
            .expect("CompactionPerformed must be emitted — it had no producer before this module");
        assert_eq!(checkpoint.1, summary_seq.to_string());
        assert!(
            after.iter().all(|r| r.seq > checkpoint.0
                || r.seq == summary_seq
                || matches!(r.event, SessionEvent::CompactionPerformed { .. })),
            "nothing at or below the checkpoint boundary may still be live"
        );
    }

    #[tokio::test]
    async fn compacted_turns_stay_searchable() {
        // The "not a net loss" contract: unlike `chat.clear`, compaction keeps
        // the BM25 mirror so `recall_events` can still surface a detail the
        // summary abstracted away. If this ever flips, the claim in the tool
        // description becomes false.
        let (service, store, sid) = seeded_session(40).await;
        compact_session(
            &service,
            store.as_ref(),
            None,
            &sid,
            &ManualCompactOptions {
                instructions: None,
                keep_tokens: Some(MIN_KEEP_TOKENS),
            },
        )
        .await
        .unwrap();

        let hits = store.search_events(&sid, "migration", 20).await.unwrap();
        assert!(
            !hits.is_empty(),
            "compacted turns must remain recallable via the event index"
        );
    }

    #[tokio::test]
    async fn a_summary_that_would_not_shrink_the_prompt_is_refused() {
        // Single-line turns: the deterministic summarizer keeps the first line
        // of each message, so its "summary" is the corpus. Compacting there
        // would trade structure for nothing — the guard must decline and say so
        // rather than report a zero-token "success".
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::session::store::migrate_add_session_events(&conn).unwrap();
        let store = std::sync::Arc::new(crate::session::store::SqliteEventStore::new(conn));
        let service = StoreBackedService {
            store: std::sync::Arc::clone(&store),
            next_seq: tokio::sync::Mutex::new(1),
        };
        let sid: SessionId = crate::routing::session_key::SessionKey::ephemeral("no-shrink");
        for i in 0..40 {
            service
                .emit_event(
                    &sid,
                    assistant(0, &format!("turn {i} {}", "z".repeat(400))).event,
                )
                .await
                .unwrap();
        }

        let outcome = compact_session(
            &service,
            store.as_ref(),
            None,
            &sid,
            &ManualCompactOptions {
                instructions: None,
                keep_tokens: Some(MIN_KEEP_TOKENS),
            },
        )
        .await
        .unwrap();
        assert!(!outcome.compacted);
        assert!(outcome
            .skipped_reason
            .as_deref()
            .is_some_and(|r| r.contains("not be smaller")));
        assert_eq!(
            service.get_events(&sid, None, None).await.unwrap().len(),
            40,
            "a refused compaction must leave the log untouched"
        );
    }

    #[tokio::test]
    async fn compacting_twice_is_idempotent_once_the_tail_fits() {
        let (service, store, sid) = seeded_session(40).await;
        let opts = ManualCompactOptions {
            instructions: None,
            keep_tokens: Some(MAX_KEEP_TOKENS),
        };
        // A huge keep budget leaves nothing to compact — an honest no-op with a
        // reason, not a fabricated success.
        let out = compact_session(&service, store.as_ref(), None, &sid, &opts)
            .await
            .unwrap();
        assert!(!out.compacted);
        assert!(out.skipped_reason.is_some());
        assert_eq!(out.tokens_saved(), 0);
        assert_eq!(
            service.get_events(&sid, None, None).await.unwrap().len(),
            80,
            "a skipped compaction must not touch the log"
        );
    }

    // -----------------------------------------------------------------------
    // The operation bracket
    // -----------------------------------------------------------------------

    /// A summarizer whose LLM call parks until the test releases it. Both
    /// failures the bracket exists for live inside that await and nowhere else,
    /// so holding a compaction open there is what makes them reproducible.
    struct ParkingSummarizer {
        /// Notified once a compaction is parked in the summarize await.
        entered: tokio::sync::Notify,
        /// Awaited there until the test lets the compaction finish.
        release: tokio::sync::Notify,
    }

    impl ParkingSummarizer {
        fn new() -> Self {
            Self {
                entered: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            }
        }
    }

    impl crate::providers::AiProvider for ParkingSummarizer {
        fn process<'a>(
            &'a self,
            _payload: crate::providers::adapter::RequestPayload<'a>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(crate::providers::adapter::ProviderResponse::text_only(
                    "<summary>The earlier turns, condensed.</summary>".to_string(),
                ))
            })
        }

        fn name(&self) -> &str {
            "parking-summarizer"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    fn parked_compactor(parked: &std::sync::Arc<ParkingSummarizer>) -> ContextCompactor {
        ContextCompactor::new(
            std::sync::Arc::clone(parked) as std::sync::Arc<dyn crate::providers::AiProvider>,
            crate::context::compact::compactor::CompactorConfig::default(),
        )
    }

    fn tail_budget_opts() -> ManualCompactOptions {
        ManualCompactOptions {
            instructions: None,
            keep_tokens: Some(MIN_KEEP_TOKENS),
        }
    }

    /// Live summaries in the log — the count that must stay at one per
    /// `/compact` no matter how the surfaces overlap.
    fn live_summaries(events: &[SessionEventRecord]) -> usize {
        events
            .iter()
            .filter(|r| {
                matches!(&r.event, SessionEvent::SystemMessage { content, .. }
                    if content.starts_with(SUMMARY_MARKER))
            })
            .count()
    }

    #[tokio::test]
    async fn a_second_compaction_is_refused_while_one_is_in_flight() {
        // Nothing upstream serializes this: the `session.compact` RPC rides a
        // concurrency semaphore, not a per-session mutex, so a TUI `/compress`
        // and the model's own `session_compact` genuinely overlap. Both would
        // read the same un-retired log and append a summary above the other's
        // retire boundary — two summaries in every future prompt.
        let (service, store, sid) = seeded_session(40).await;
        let parked = std::sync::Arc::new(ParkingSummarizer::new());
        let compactor = parked_compactor(&parked);
        let opts = tail_budget_opts();

        // The second entry deliberately takes the deterministic path: a
        // regression must fail this test's assertions, not deadlock waiting on
        // a summarizer only the first entry can release.
        let (first, second) = tokio::join!(
            compact_session(&service, store.as_ref(), Some(&compactor), &sid, &opts),
            async {
                parked.entered.notified().await;
                let out = compact_session(&service, store.as_ref(), None, &sid, &opts).await;
                parked.release.notify_one();
                out
            }
        );

        let first = first.unwrap();
        let second = second.unwrap();
        assert!(first.compacted, "{:?}", first.skipped_reason);
        assert!(
            !second.compacted,
            "the overlapping compaction must be refused, not run"
        );
        assert!(
            second
                .skipped_reason
                .as_deref()
                .is_some_and(|r| r.contains("already in progress")),
            "the refusal must say why: {:?}",
            second.skipped_reason
        );

        let after = service.get_events(&sid, None, None).await.unwrap();
        assert_eq!(
            live_summaries(&after),
            1,
            "one compaction, one live summary"
        );
    }

    #[tokio::test]
    async fn a_clear_during_the_summarize_does_not_resurrect_the_erased_turns() {
        // `chat.clear` retires every live event and deletes the BM25 mirror
        // precisely so the erased turns can never be handed back to the model.
        // A compaction whose summarize was in flight must not then append a
        // summary OF that span — it would put the erased content at the head of
        // every future prompt, which is the one thing clear promises it cannot.
        let (service, store, sid) = seeded_session(40).await;
        let parked = std::sync::Arc::new(ParkingSummarizer::new());
        let compactor = parked_compactor(&parked);
        let opts = tail_budget_opts();

        let (outcome, ()) = tokio::join!(
            compact_session(&service, store.as_ref(), Some(&compactor), &sid, &opts),
            async {
                parked.entered.notified().await;
                store.retire_from(&sid, 1).await.unwrap();
                parked.release.notify_one();
            }
        );

        let outcome = outcome.unwrap();
        assert!(
            !outcome.compacted,
            "a span the user erased mid-summarize must not be compacted"
        );
        assert!(
            outcome
                .skipped_reason
                .as_deref()
                .is_some_and(|r| r.contains("changed while the summary")),
            "the skip must name the race: {:?}",
            outcome.skipped_reason
        );

        let after = service.get_events(&sid, None, None).await.unwrap();
        assert!(
            after.is_empty(),
            "a cleared conversation must stay cleared; {} event(s) came back",
            after.len()
        );
    }

    #[tokio::test]
    async fn the_bracket_is_released_when_the_compaction_returns() {
        // A leaked claim is silent and permanent: every later `/compact` on
        // that session answers "already in progress" for the life of the
        // process, and the conversation just stops being compactable.
        let (service, store, sid) = seeded_session(40).await;
        let opts = tail_budget_opts();

        let first = compact_session(&service, store.as_ref(), None, &sid, &opts)
            .await
            .unwrap();
        assert!(first.compacted, "{:?}", first.skipped_reason);

        let second = compact_session(&service, store.as_ref(), None, &sid, &opts)
            .await
            .unwrap();
        assert!(
            !second
                .skipped_reason
                .as_deref()
                .unwrap_or_default()
                .contains("in progress"),
            "the bracket outlived its compaction: {:?}",
            second.skipped_reason
        );
    }

    #[test]
    fn a_bracket_is_per_session_not_per_process() {
        // Two different conversations compacting at once is normal traffic —
        // the claim must key on the session, or one user's `/compact` refuses
        // everybody else's.
        let a: SessionId = crate::routing::session_key::SessionKey::ephemeral("bracket-a");
        let b: SessionId = crate::routing::session_key::SessionKey::ephemeral("bracket-b");

        let held_a = CompactionBracket::try_acquire(&a).expect("first claim on A");
        let held_b = CompactionBracket::try_acquire(&b).expect("a different session is unaffected");
        assert!(
            CompactionBracket::try_acquire(&a).is_none(),
            "A is already claimed"
        );

        drop(held_a);
        assert!(
            CompactionBracket::try_acquire(&a).is_some(),
            "dropping the claim frees the session"
        );
        drop(held_b);
    }

    #[test]
    fn summary_marker_is_recognised_by_the_shared_predicate() {
        // The appended summary must be skipped by verbatim-user-turn
        // preservation and by the live-task anchor, both of which key off the
        // canonical marker. A drifted marker here would make every later
        // compaction re-attach this summary as if it were user intent.
        let content = format!("{SUMMARY_MARKER}\n{SUMMARY_FRAMING}\n\nbody");
        assert_eq!(
            latest_user_task(&[UnifiedMessage::user(content)]),
            None,
            "the compaction summary must never be mistaken for the user's task"
        );
    }
}
