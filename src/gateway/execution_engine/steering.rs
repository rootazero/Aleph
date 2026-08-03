//! Mid-loop steering — inject a user message into a live agent run.
//!
//! # Why this exists (codex parity)
//!
//! When a user sends a message while an agent is already mid-run, the legacy
//! behaviour was to reject with [`ExecutionError::AgentBusy`] and let the
//! inbound router retry with back-off until the agent went idle. For a long
//! autonomous loop (dozens of Think→Act turns) that meant the user could not
//! course-correct for minutes — the steering message only ran *after* the
//! original task finished.
//!
//! codex solves this with an input mailbox (`get_pending_input` /
//! `take_queued_response_items_for_next_turn`): a message that arrives mid-turn
//! is injected into the running conversation and consumed at the next turn
//! boundary. Aleph's harness was already *designed* for this — the prompt
//! builder (G2) wraps any non-synthetic `UserMessage` found in the live tail in
//! `<system-reminder>` so the model treats it as a genuine interjection, and the
//! Think loop re-reads the full event log every turn via `get_events`. The only
//! missing wire was a delivery path that appends such an event to a *running*
//! session. This module is that wire.
//!
//! # R10 compliance
//!
//! This is pure scaffolding (an I/O delivery seam), not cognition: it makes no
//! routing, intent, or completion judgement. The harness is untouched; a
//! stronger model needs this exactly as much as a weaker one (Future-Proof
//! Test ✓). The decision of *whether* to inject is a mechanical same-session
//! check, kept as a pure, unit-tested function.

use std::collections::HashMap;
use std::sync::OnceLock;

use tokio::sync::RwLock;

use super::{ActiveRun, RunRequest, RunState};
use crate::orchestrator::Orchestrator;
use crate::routing::session_key::SessionKey;
use crate::session::events::{now_ms, MessageContent, SessionEvent, SessionEventRecord};
use crate::session::service::SessionId;
use crate::sync_primitives::Arc;

/// Find the run id of *another* `Running` sibling on `session_key`, if any.
///
/// Pure and synchronous so the same-session predicate can be unit-tested without
/// spinning up an orchestrator. `new_run_id` is the just-reserved run that lost
/// the `try_start_run` race; it is excluded so a run never counts itself as its
/// own steering target. The id is what the `Interrupt` busy-input mode feeds to
/// [`super::ExecutionEngine::cancel`]; `Steer` only needs the boolean
/// ([`find_steering_target`]).
pub(super) fn find_steering_target_id(
    runs: &HashMap<String, ActiveRun>,
    new_run_id: &str,
    session_key: &SessionKey,
) -> Option<String> {
    runs.iter().find_map(|(id, run)| {
        (id != new_run_id
            && matches!(run.state, RunState::Running)
            && &run.request.session_key == session_key)
            .then(|| id.clone())
    })
}

/// Decide whether `session_key` already has *another* `Running` sibling run.
/// Thin boolean wrapper over [`find_steering_target_id`] for the `Steer` path,
/// which does not need the id.
/// Test-only: the boolean shape of [`find_steering_target_id`]. Production now
/// needs the id itself (to read the target run's model override), so this stays
/// as the readable spelling the target-selection tests are written against.
#[cfg(test)]
pub(super) fn find_steering_target(
    runs: &HashMap<String, ActiveRun>,
    new_run_id: &str,
    session_key: &SessionKey,
) -> bool {
    find_steering_target_id(runs, new_run_id, session_key).is_some()
}

/// Whether `request` is asking for anything the steering seam cannot carry.
///
/// A steering event is a plain-text `SessionEvent::UserMessage`
/// (`blocks: Vec::new()`), appended to the live log for the running loop to read
/// at its next turn. Everything else a `RunRequest` can carry is resolved
/// **after** the admission gate in `execute()`, so `GateOutcome::HandledInline`
/// skips all of it — the extra intent is silently discarded and the request
/// returns success having done only half of what was asked.
///
/// Two things are known to ride requests that reach this path:
///
/// * **Attachments.** The injection path has no media-processor seam, so a file
///   would degrade to a text marker the model cannot see (the harness replays
///   media only from `content.blocks`). The busy queue redelivers it as a fresh
///   run that processes media normally (`inner.rs` media_processor →
///   `FlowInput::Multimodal`).
/// * **Slash commands.** The L0 fast path and the skill/allowed-tools overlay
///   both read `SLASH_COMMAND_MODE_KEY` in `execute()`, *after* the gate. A
///   steered `/moa on` therefore never executes: it lands in the transcript as
///   the literal string `/moa on`, the running loop reads it as a mid-task
///   interjection, and the client gets zero events and no error. This module
///   already knew the metadata was load-bearing in the other direction —
///   [`build_steering_rescue_request`] strips it so a rescue cannot re-enter the
///   fast path — but nothing checked the reverse. Reachable only since the lane
///   stopped holding the running message's ticket (`mark_admitted`): before
///   that, a follow-up command parked and ran as a fresh run, which is exactly
///   the behaviour deferring restores.
///
/// Scoped to injection, NOT to [`has_steering_content`], so the `Interrupt`
/// branch still cancels a sibling for an attachment-only or command-only
/// message.
fn carries_more_than_text(request: &RunRequest) -> bool {
    !request.attachments.is_empty()
        || request
            .metadata
            .contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY)
}

/// Whether `request` carries actual user steering content (non-blank text or at
/// least one attachment).
///
/// A content-less request — a resume-style loop continuation, or a synthetic run
/// that merely lost the `try_start_run` race — has nothing to contribute to a
/// running sibling. Both busy-input branches consult this single predicate so
/// they stay symmetric: `Steer` skips injecting a blank `UserMessage`, and
/// `Interrupt` skips cancelling a healthy sibling it cannot improve on (Hermes
/// `internal`-event protection parity — a content-less event never tears down
/// in-flight work). Intrinsic guard at the decision point, so it no longer
/// depends on every caller remembering to strip an inherited `Interrupt` key
/// (cf. [`build_steering_rescue_request`]). Pure for unit testing.
pub(super) fn has_steering_content(request: &RunRequest) -> bool {
    !request.input.trim().is_empty() || !request.attachments.is_empty()
}

/// Render the user-visible session text for a steering interjection: the raw
/// input, plus a text marker for any attachment.
///
/// This is NOT byte-identical to how `execute()` stores a first message — that
/// path stores the *raw* input as text and carries attachments as real media
/// `ContentBlock`s (`FlowInput::Multimodal`). A steering event has empty
/// `blocks`, so a marker is the only way to represent an attachment in text.
/// In practice `try_inject_steering` now defers attachment-bearing steers to the
/// busy queue, so the sole production caller reaches here with plain text; the
/// markers remain a fallback for any direct caller. Do NOT "restore parity" by
/// copying these markers into `execute()` — the normal path deliberately keeps
/// the stored message and the derived session title equal to the raw input.
pub(super) fn render_user_session_text(request: &RunRequest) -> String {
    let mut text = request.input.clone();
    for att in &request.attachments {
        let label = att.filename.as_deref().unwrap_or("file");
        if att.mime_type.starts_with("image/") {
            text.push_str(&format!("\n[Image attached: {}]", att.mime_type));
        } else if att.mime_type.starts_with("audio/") {
            text.push_str(&format!("\n[Audio attached: {}]", att.mime_type));
        } else {
            text.push_str(&format!("\n[Attachment: {} ({})]", label, att.mime_type));
        }
    }
    text
}

/// Prepended to a steering message when the target session has an active
/// scratchpad execution list, so the model reconciles its task list before
/// continuing. The model decides append / insert / reprioritize (R7 — the
/// harness never splices `scratchpad.md` for the user).
const RECONCILE_PREAMBLE: &str =
    "[user added mid-task] The user sent new input while you are executing a \
     task list. Reconcile your scratchpad first — call the scratchpad tool to \
     append, insert, or reprioritize steps as you judge appropriate — then \
     continue.\n\nNew input: ";

/// Prepend [`RECONCILE_PREAMBLE`] to `text` iff the session has an active
/// scratchpad. Pure so the policy is unit-tested without a registry global,
/// mirroring [`find_steering_target`].
pub(super) fn apply_reconcile_preamble(text: String, has_active_scratchpad: bool) -> String {
    if !has_active_scratchpad {
        return text;
    }
    format!("{RECONCILE_PREAMBLE}{text}")
}

/// Default upper bound on un-consumed steering messages a single run may
/// accumulate before the gateway applies backpressure (`OpenSquilla`
/// `max_pending_per_session` / `OpenClaw` FIFO-cap parity). A flooding channel
/// that keeps sending while the agent is mid-loop would otherwise append
/// unbounded `UserMessage` events to the live log, bloating the very next
/// prompt. Past the cap the injection is rejected so the busy wait lane
/// redelivers the message once the burst drains — backpressure, never a drop.
///
/// This constant is the single source for the `[execution] max_pending_steering`
/// default (see `ExecutionEngineConfig::max_pending_steering`); the effective
/// bound is the configured one.
pub(super) const MAX_PENDING_STEERING: usize = 16;

/// Count the non-synthetic user messages sitting *after* the last assistant
/// message in `events` — the steering burst already injected into this run that
/// the model has not yet answered.
///
/// Mirrors the harness follow-up predicate
/// ([`crate::harness::agent::AgentHarness::has_unanswered_user_message`]) but
/// from the gateway side, which has no access to the harness's prompt-boundary
/// watermark and so uses the last assistant turn as the boundary. Returns `0`
/// before any assistant turn has run: the loop is still processing its opening
/// prompt, so there is no prior preamble-bearing steering injection to coalesce
/// against, and the original task prompt must not be miscounted as a steering
/// message. Pure and positional (R10-safe): no intent or relevance judgement,
/// only the log's shape.
pub(super) fn count_pending_steering(events: &[SessionEventRecord]) -> usize {
    let Some(last_assistant) = events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
    else {
        return 0;
    };
    events[last_assistant + 1..]
        .iter()
        .filter(|r| matches!(&r.event, SessionEvent::UserMessage { synthetic, .. } if !*synthetic))
        .count()
}

/// How far back [`read_steering_events`] reads before falling back to the whole
/// log.
///
/// [`count_pending_steering`] only looks back to the last assistant message, so
/// a trailing window answers it exactly in every realistic session — a burst
/// deeper than this is already an order of magnitude past any sane
/// `max_pending_steering`.
pub(super) const STEERING_TAIL_EVENTS: u64 = 256;

/// Read the events [`count_pending_steering`] needs, bounded.
///
/// `get_events(.., None, None)` is a full-table scan plus one `serde_json` parse
/// per event, and — now that the wait lane lets follow-ups reach the engine
/// mid-run at all — steering runs it once per follow-up message. On a long-lived
/// session that is megabytes of allocation to answer a question about the last
/// handful of events.
///
/// The window is trailing, so the answer is exact unless it happens to contain
/// no assistant message; in that case we re-read in full rather than guess (a
/// truncated view would read as "no burst pending" and quietly disable both the
/// coalescing rule and the backpressure cap). Any read error yields `None`, and
/// callers fail open exactly as they did against the single full read.
async fn read_steering_events(
    orchestrator: &Orchestrator,
    session_id: &SessionId,
) -> Option<Vec<SessionEventRecord>> {
    let head = orchestrator
        .session_service
        .attach(session_id.clone())
        .await
        .ok()?
        .head_seq;
    let from = head.saturating_sub(STEERING_TAIL_EVENTS);
    if from > 0 {
        let tail = orchestrator
            .session_service
            .get_events(session_id, Some(from), None)
            .await
            .ok()?;
        if tail
            .iter()
            .any(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        {
            return Some(tail);
        }
    }
    orchestrator
        .session_service
        .get_events(session_id, None, None)
        .await
        .ok()
}

/// Metadata key counting consecutive post-run steering rescues on a session.
/// Carried on the rescue `RunRequest` so a pathological injector that keeps
/// landing messages in the teardown window cannot chain rescues forever.
pub(super) const STEERING_RESCUE_DEPTH_KEY: &str = "steering_rescue_depth";

/// Max consecutive rescue runs (bounds the codex
/// `maybe_start_turn_for_pending_work` parity loop). Each extra rescue
/// requires a message to land in the narrow teardown window of the *previous*
/// rescue, so 2 already covers pathological timing; past the cap the burst
/// waits for the next user interaction — deferred, never dropped.
pub(super) const MAX_STEERING_RESCUE_DEPTH: usize = 2;

/// Parse the rescue depth carried in `metadata` and return the depth the next
/// rescue run should carry, or `None` once the cap is reached. Pure so the
/// bound is unit-testable without an orchestrator.
pub(super) fn next_rescue_depth(metadata: &HashMap<String, String>) -> Option<usize> {
    let depth = metadata
        .get(STEERING_RESCUE_DEPTH_KEY)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    (depth < MAX_STEERING_RESCUE_DEPTH).then_some(depth + 1)
}

/// Build the run that closes the steering teardown race (codex
/// `maybe_start_turn_for_pending_work` parity).
///
/// A steering message that lands *after* the harness's final follow-up check
/// but *before* the run's state flips out of `Running` is acknowledged into
/// the session log (the injector saw a `Running` sibling) yet has no live
/// loop left to answer it — without this, it sits unanswered until the user
/// sends a second message. Called by `execute()` after the state flip, at
/// which point further injections are impossible, so one bounded re-read of
/// the log decides race-free: an unanswered burst remains → re-drive the loop
/// over the existing log via the resume flow (`metadata["resume"]`), which
/// seeds no new user message — the orphaned steering message already in the
/// log is exactly what the rescue must answer.
///
/// Returns `None` when there is nothing to rescue, the orchestrator is not
/// wired, the log cannot be read (fail closed — the busy/retry path still
/// covers genuinely lost messages on their next delivery), or the rescue
/// depth cap is reached.
pub(super) async fn build_steering_rescue_request(
    orchestrator: &OnceLock<Arc<Orchestrator>>,
    request: &RunRequest,
) -> Option<RunRequest> {
    let Some(next_depth) = next_rescue_depth(&request.metadata) else {
        tracing::warn!(
            session = %request.session_key.to_key_string(),
            cap = MAX_STEERING_RESCUE_DEPTH,
            "post-run steering rescue: depth cap reached; deferring burst to next interaction",
        );
        return None;
    };
    let orchestrator = orchestrator.get()?;
    let session_id: SessionId = request.session_key.clone();
    let events = read_steering_events(orchestrator, &session_id).await?;
    if count_pending_steering(&events) == 0 {
        return None;
    }

    let mut metadata = request.metadata.clone();
    metadata.insert("resume".to_string(), "true".to_string());
    metadata.insert(
        STEERING_RESCUE_DEPTH_KEY.to_string(),
        next_depth.to_string(),
    );
    // Strip slash-command residue: the rescue is a plain loop continuation and
    // must never re-enter the fast path or re-apply a skill overlay.
    metadata.remove(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY);
    metadata.remove("slash_skill_instructions");
    metadata.remove("slash_skill_allowed_tools");
    // Strip the busy-input policy: if another run grabbed the freed slot
    // first (it reads the same log, so it covers the orphaned burst), an
    // inherited `Interrupt` would cancel that legitimate sibling. Absent key
    // → default `Steer`, and the empty-input guard in `try_inject_steering`
    // keeps the rescue from injecting a blank message — it just dissolves.
    metadata.remove(super::BUSY_INPUT_MODE_KEY);

    Some(RunRequest {
        run_id: uuid::Uuid::new_v4().to_string(),
        input: String::new(),
        session_key: request.session_key.clone(),
        timeout_secs: request.timeout_secs,
        metadata,
        attachments: Vec::new(),
        pending_media: Default::default(),
        sandbox_override: request.sandbox_override.clone(),
        workspace_override: request.workspace_override.clone(),
        max_iterations_override: request.max_iterations_override,
        model_override: request.model_override.clone(),
    })
}

/// Try to deliver `request` as a mid-loop steering message into the session's
/// already-running loop. Returns `true` when the message was injected (the
/// caller should treat the run as accepted and skip the `AgentBusy` path).
///
/// Returns `false` — deferring to the inbound router's FIFO busy queue — when:
/// * steering is disabled by config, or
/// * no *other* run is active on this exact session (a cross-session busy
///   agent is genuinely unavailable; the message waits its turn in the queue), or
/// * the orchestrator / session service is not yet wired, or
/// * the run's un-consumed steering burst is already at [`MAX_PENDING_STEERING`]
///   (backpressure — the queue redelivers once the burst drains), or
/// * appending the event failed.
///
/// # Coalescing
///
/// When an earlier steering message in the same un-answered burst is still
/// pending ([`count_pending_steering`] `> 0`), the scratchpad reconcile preamble
/// is suppressed: the first message of the burst already carries that directive,
/// so one copy covers the coalesced batch instead of repeating it per message.
///
/// # Boundary
///
/// If injection lands during the running loop's *final* LLM call, the message
/// is appended to the log *before* the assistant message that turn commits.
/// The harness still catches it: the outer loop's follow-up check compares the
/// log against the final turn's prompt boundary watermark
/// (`AgentHarness::last_prompt_seq`), so a message past that
/// boundary — including one wedged before the trailing assistant message —
/// re-enters the loop and is answered in the same run. Only if the run has
/// already torn down (no live loop to continue) does it defer to the next
/// interaction, when `get_events` returns it alongside the next user turn. It
/// is never dropped.
pub(super) async fn try_inject_steering(
    enabled: bool,
    max_pending: usize,
    active_runs: &RwLock<HashMap<String, ActiveRun>>,
    orchestrator: &OnceLock<Arc<Orchestrator>>,
    request: &RunRequest,
    new_run_id: &str,
) -> bool {
    if !enabled {
        return false;
    }

    // Nothing to say, nothing to inject: a request with no text and no
    // attachments (e.g. a resume-style run that lost the slot race) would
    // append a blank user message to the live log. Fall back to busy/retry.
    // Same predicate gates the `Interrupt` branch, keeping the two modes
    // symmetric.
    if !has_steering_content(request) {
        return false;
    }

    // A steering event carries TEXT AND NOTHING ELSE. Anything else the request
    // is asking for has to be deferred to the FIFO busy queue, which redelivers
    // it as a fresh run through the full pipeline. Never dropped — deferred.
    if carries_more_than_text(request) {
        return false;
    }

    let target_model = {
        let runs = active_runs.read().await;
        match find_steering_target_id(&runs, new_run_id, &request.session_key) {
            Some(id) => runs.get(&id).and_then(|r| r.request.model_override.clone()),
            None => return false,
        }
    };

    // The sibling is already committed to its model for this run; a steer cannot
    // change it. Folding in a message that asked for a different one would apply
    // the text and silently drop the directive — the composer's model pill would
    // read `opus` while the answer came from `sonnet`, with no banner and no
    // error. Defer instead, so the request gets the model it asked for.
    if request.model_override != target_model {
        return false;
    }

    let Some(orchestrator) = orchestrator.get() else {
        return false;
    };

    // `SessionId` is a type alias for `SessionKey`, so the gateway key is the
    // harness session id verbatim — no translation needed.
    let session_id: SessionId = request.session_key.clone();

    // Read the running session's tail once to make the injection both bounded
    // and coalescing-aware (Hermes / Pi / OpenSquilla parity), then decide from
    // that single snapshot. Fail open: a transient read error falls back to the
    // legacy always-inject-with-preamble path, never dropping the message.
    let pending = read_steering_events(orchestrator, &session_id)
        .await
        .as_deref()
        .map_or(0, count_pending_steering);

    // Bound: cap the un-consumed steering burst. Past the cap, reject so the
    // busy wait lane redelivers once the loop drains the burst or goes idle —
    // backpressure against a flooding channel, not a drop.
    if pending >= max_pending {
        tracing::warn!(
            session = %request.session_key.to_key_string(),
            pending,
            cap = max_pending,
            "mid-loop steering: pending burst at cap; deferring to busy-queue backpressure",
        );
        return false;
    }

    // If this session is driving a scratchpad execution list, tell the model to
    // reconcile it before continuing — but only on the *first* message of an
    // un-answered burst. A follow-up that lands while an earlier steering
    // message is still pending would otherwise repeat the identical reconcile
    // directive N times (pure noise the model re-reads each turn); one directive
    // covers the whole coalesced batch (Hermes burst-merge / Pi queue parity).
    // The model decides append / insert / reprioritize (R7 — the harness never
    // splices `scratchpad.md` for the user). Mechanical lookup, no extra I/O.
    let has_active_scratchpad = pending == 0
        && crate::builtin_tools::scratchpad_registry::active(&request.session_key.to_key_string())
            .is_some();
    let text = apply_reconcile_preamble(render_user_session_text(request), has_active_scratchpad);
    let event = SessionEvent::UserMessage {
        turn_id: uuid::Uuid::new_v4(),
        content: MessageContent {
            text,
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        at: now_ms(),
        // `false` → the prompt builder (G2) wraps this in `<system-reminder>`
        // as a real user interjection, exactly the designed steering path.
        synthetic: false,
    };

    match orchestrator
        .session_service
        .emit_event(&session_id, event)
        .await
    {
        Ok(_) => {
            // Log new_run_id so an operator can correlate a deferred steering
            // message with the target run if the sibling leaves Running in the
            // find_target→emit window (then only the sibling's Completed-only
            // teardown rescue re-drives it; a Cancelled/Failed sibling defers the
            // message to the next user turn — never dropped, but otherwise opaque).
            tracing::info!(
                session = %request.session_key.to_key_string(),
                new_run_id = %new_run_id,
                "mid-loop steering: injected user message into running loop",
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                session = %request.session_key.to_key_string(),
                error = %e,
                "mid-loop steering: failed to inject; falling back to the busy queue",
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steering event is a plain-text `UserMessage`; everything else a request
    /// asks for is resolved after the admission gate, so `HandledInline` skips
    /// it. Anything carrying more than text must be deferred to the lane, which
    /// redelivers it as a fresh run through the whole pipeline.
    #[test]
    fn a_slash_command_is_never_folded_into_a_running_sibling() {
        assert!(
            !carries_more_than_text(&run_request("s1", "keep going but be careful")),
            "ordinary text is exactly what the steering seam carries"
        );

        let mut command = run_request("s1", "/moa on");
        command.metadata.insert(
            crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY.to_string(),
            "{}".to_string(),
        );
        assert!(
            carries_more_than_text(&command),
            "the L0 fast path and the skill overlay both read this key AFTER the \
             gate, so a steered command lands in the transcript as the literal \
             string `/moa on` and never runs"
        );
        // The text alone is indistinguishable — only the metadata says it is a
        // command, which is why this cannot be a look-at-the-string check.
        assert!(!carries_more_than_text(&run_request("s1", "/moa on")));
    }

    #[test]
    fn an_attachment_bearing_steer_is_still_deferred() {
        let mut with_file = run_request("s1", "look at this");
        with_file.attachments = vec![Attachment {
            id: "att-1".to_string(),
            mime_type: "image/png".to_string(),
            filename: Some("a.png".to_string()),
            size: None,
            url: None,
            path: None,
            data: None,
        }];
        assert!(carries_more_than_text(&with_file));
    }

    use crate::gateway::channel::Attachment;
    use crate::sync_primitives::{AtomicU32, AtomicU64};

    fn run_request(session: &str, input: &str) -> RunRequest {
        RunRequest {
            run_id: "r-x".to_string(),
            input: input.to_string(),
            session_key: SessionKey::from_key_string(session).unwrap_or_else(|| {
                SessionKey::Ephemeral {
                    agent_id: "agent".to_string(),
                    ephemeral_id: session.to_string(),
                }
            }),
            timeout_secs: None,
            metadata: Default::default(),
            attachments: Vec::new(),
            pending_media: Default::default(),
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            model_override: None,
        }
    }

    fn active_run(run_id: &str, session: &str, state: RunState) -> (String, ActiveRun) {
        (
            run_id.to_string(),
            ActiveRun {
                request: run_request(session, "prior"),
                state,
                started_at: chrono::Utc::now(),
                completed_at: None,
                steps_completed: 0,
                current_tool: None,
                cancel_tx: None,
                seq_counter: AtomicU64::new(0),
                chunk_counter: AtomicU32::new(0),
            },
        )
    }

    #[test]
    fn target_found_for_same_session_running_sibling() {
        let mut runs = HashMap::new();
        let (id, run) = active_run("r-old", "s1", RunState::Running);
        runs.insert(id, run);
        assert!(find_steering_target(
            &runs,
            "r-new",
            &run_request("s1", "steer").session_key
        ));
    }

    #[test]
    fn target_id_returns_running_sibling_run_id() {
        // The Interrupt mode needs the *id* to cancel, not just a boolean.
        let mut runs = HashMap::new();
        let (id, run) = active_run("r-old", "s1", RunState::Running);
        runs.insert(id, run);
        assert_eq!(
            find_steering_target_id(&runs, "r-new", &run_request("s1", "steer").session_key),
            Some("r-old".to_string())
        );
        // No cross-session match → nothing to cancel.
        assert_eq!(
            find_steering_target_id(&runs, "r-new", &run_request("s2", "steer").session_key),
            None
        );
    }

    #[test]
    fn no_target_when_only_self_is_present() {
        let mut runs = HashMap::new();
        let (id, run) = active_run("r-new", "s1", RunState::Running);
        runs.insert(id, run);
        // Only the just-reserved run exists; it must not steer itself.
        assert!(!find_steering_target(
            &runs,
            "r-new",
            &run_request("s1", "steer").session_key
        ));
    }

    #[test]
    fn no_target_for_different_session() {
        let mut runs = HashMap::new();
        let (id, run) = active_run("r-old", "s2", RunState::Running);
        runs.insert(id, run);
        assert!(!find_steering_target(
            &runs,
            "r-new",
            &run_request("s1", "steer").session_key
        ));
    }

    #[test]
    fn no_target_when_sibling_not_running() {
        let mut runs = HashMap::new();
        let (id, run) = active_run("r-old", "s1", RunState::Completed);
        runs.insert(id, run);
        assert!(!find_steering_target(
            &runs,
            "r-new",
            &run_request("s1", "steer").session_key
        ));
    }

    #[test]
    fn preamble_added_only_when_scratchpad_active() {
        let with = apply_reconcile_preamble("do X".to_string(), true);
        assert!(with.contains("do X"));
        assert!(with.starts_with(RECONCILE_PREAMBLE));

        let without = apply_reconcile_preamble("do X".to_string(), false);
        assert_eq!(without, "do X");
    }

    #[test]
    fn has_content_true_for_text_or_attachment_false_for_empty() {
        // Plain text → content.
        assert!(has_steering_content(&run_request("s1", "do X")));
        // Whitespace-only is treated as empty (a resume placeholder).
        assert!(!has_steering_content(&run_request("s1", "   ")));
        assert!(!has_steering_content(&run_request("s1", "")));
        // No text but an attachment still counts as content (image-only steer).
        let mut req = run_request("s1", "");
        req.attachments = vec![Attachment {
            id: "att-1".to_string(),
            mime_type: "image/png".to_string(),
            filename: Some("a.png".to_string()),
            size: None,
            url: None,
            path: None,
            data: None,
        }];
        assert!(has_steering_content(&req));
    }

    #[test]
    fn render_appends_attachment_markers() {
        let mut req = run_request("s1", "hello");
        req.attachments = vec![Attachment {
            id: "att-1".to_string(),
            mime_type: "image/png".to_string(),
            filename: Some("a.png".to_string()),
            size: None,
            url: None,
            path: None,
            data: None,
        }];
        let text = render_user_session_text(&req);
        assert!(text.starts_with("hello"));
        assert!(text.contains("[Image attached: image/png]"));
    }

    // ---- count_pending_steering (coalesce / bound predicate) ----

    fn rec_user(text: &str, synthetic: bool) -> SessionEventRecord {
        SessionEventRecord {
            seq: 0,
            created_at_ms: now_ms(),
            event: SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: text.to_string(),
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic,
            },
        }
    }

    fn rec_assistant(text: &str) -> SessionEventRecord {
        SessionEventRecord {
            seq: 0,
            created_at_ms: now_ms(),
            event: SessionEvent::AssistantMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: text.to_string(),
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                usage: None,
                at: now_ms(),
            },
        }
    }

    #[test]
    fn pending_is_zero_before_any_assistant_turn() {
        // Loop still processing its opening prompt — the original prompt (and any
        // pre-first-assistant steering) must not be miscounted as pending.
        assert_eq!(count_pending_steering(&[rec_user("task", false)]), 0);
        assert_eq!(
            count_pending_steering(&[rec_user("task", false), rec_user("steer", false)]),
            0
        );
    }

    #[test]
    fn pending_counts_user_after_last_assistant() {
        let one = [
            rec_user("task", false),
            rec_assistant("working"),
            rec_user("steer-1", false),
        ];
        assert_eq!(count_pending_steering(&one), 1);

        let two = [
            rec_user("task", false),
            rec_assistant("working"),
            rec_user("steer-1", false),
            rec_user("steer-2", false),
        ];
        assert_eq!(count_pending_steering(&two), 2);
    }

    #[test]
    fn pending_ignores_synthetic_user_messages() {
        // Synthetic injections (e.g. tool-driven) are not user steering.
        let events = [
            rec_user("task", false),
            rec_assistant("working"),
            rec_user("auto", true),
        ];
        assert_eq!(count_pending_steering(&events), 0);
    }

    #[test]
    fn pending_resets_at_each_assistant_boundary() {
        // Only messages after the *last* assistant turn are still unanswered;
        // earlier bursts were already seen and answered.
        let events = [
            rec_user("task", false),
            rec_assistant("turn-1"),
            rec_user("steer-1", false),
            rec_assistant("turn-2"),
            rec_user("steer-2", false),
        ];
        assert_eq!(count_pending_steering(&events), 1);
    }

    // ---- next_rescue_depth (post-run rescue bound) ----

    #[test]
    fn rescue_depth_starts_at_one_and_caps() {
        let mut meta = HashMap::new();
        // First rescue on a normal run (no key) carries depth 1.
        assert_eq!(next_rescue_depth(&meta), Some(1));
        // A garbage value degrades to the no-key default, never a panic.
        meta.insert(STEERING_RESCUE_DEPTH_KEY.to_string(), "nope".to_string());
        assert_eq!(next_rescue_depth(&meta), Some(1));
        // Chained rescue increments…
        meta.insert(STEERING_RESCUE_DEPTH_KEY.to_string(), "1".to_string());
        assert_eq!(next_rescue_depth(&meta), Some(2));
        // …until the cap, where the burst defers to the next interaction.
        meta.insert(
            STEERING_RESCUE_DEPTH_KEY.to_string(),
            MAX_STEERING_RESCUE_DEPTH.to_string(),
        );
        assert_eq!(next_rescue_depth(&meta), None);
    }

    #[test]
    fn pending_at_cap_triggers_bound() {
        // A burst at the cap is what the gateway rejects for backpressure.
        let mut events = vec![rec_user("task", false), rec_assistant("working")];
        for i in 0..MAX_PENDING_STEERING {
            events.push(rec_user(&format!("steer-{i}"), false));
        }
        assert_eq!(count_pending_steering(&events), MAX_PENDING_STEERING);
        assert!(count_pending_steering(&events) >= MAX_PENDING_STEERING);
    }

    /// The `[execution] max_pending_steering` TOML default and the engine's own
    /// default must agree — they live in different layers (config crate vs
    /// gateway) and a silent drift would make the documented default a lie.
    #[test]
    fn config_default_matches_the_engine_default() {
        assert_eq!(
            crate::config::types::execution::ExecutionConfig::default().max_pending_steering,
            MAX_PENDING_STEERING
        );
        assert_eq!(
            super::super::ExecutionEngineConfig::default().max_pending_steering,
            MAX_PENDING_STEERING
        );
    }
}
