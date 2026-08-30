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

/// The running sibling that owns this session's slot, read out of
/// `active_runs` **once**.
///
/// Every arm of the busy-input decision needs something from the same run —
/// `for_shared_room` needs its author, `Interrupt` needs its id and its
/// admission instant, `Steer` needs its model and workspace to compare
/// against. Those used to be three separate `active_runs.read().await`
/// acquisitions inside one decision, so the mode could be chosen against one
/// snapshot and applied against another; the widest of those windows spanned
/// the whole of `try_inject_steering`'s first log read. One read, one snapshot,
/// one decision.
pub(super) struct BusySibling {
    /// The run to cancel if this turns out to be an `Interrupt`.
    pub(super) run_id: String,
    /// The sibling's request metadata — `for_shared_room` reads its author out
    /// of this to decide whether the incoming turn has authority over it.
    pub(super) metadata: HashMap<String, String>,
    /// Locked in for the sibling's whole run: a steer cannot change it, so a
    /// message asking for a different one must be deferred rather than folded.
    pub(super) model_override: Option<crate::gateway::model_override::ModelOverride>,
    /// Same argument, with file-writing consequences.
    pub(super) workspace_override: Option<std::path::PathBuf>,
    /// When the gate admitted it, on the monotonic clock. Compared against
    /// [`crate::gateway::busy_queue::waiting_since`] — see
    /// [`interrupt_targets_an_unseen_run`].
    pub(super) admitted_at: std::time::Instant,
}

/// Resolve the busy sibling for `session_key`, excluding `new_run_id` itself.
///
/// The single `active_runs` read behind the whole busy-input decision; see
/// [`BusySibling`].
pub(super) async fn find_busy_sibling(
    active_runs: &RwLock<HashMap<String, ActiveRun>>,
    new_run_id: &str,
    session_key: &SessionKey,
) -> Option<BusySibling> {
    let runs = active_runs.read().await;
    let id = find_steering_target_id(&runs, new_run_id, session_key)?;
    let run = runs.get(&id)?;
    Some(BusySibling {
        run_id: id,
        metadata: run.request.metadata.clone(),
        model_override: run.request.model_override.clone(),
        workspace_override: run.request.workspace_override.clone(),
        admitted_at: run.admitted_at,
    })
}

/// Whether an `Interrupt`-mode message would be cancelling a run that started
/// **after** the message began waiting — i.e. a run its author never saw.
///
/// `Interrupt` means "supersede the task that was running when I sent this".
/// Since [`crate::gateway::busy_queue::mark_admitted`] let followers reach the
/// engine mid-run, a *burst* of interrupt-mode messages read it instead as
/// "supersede whatever is running when my turn comes round" — and by then that
/// is the run the message immediately ahead of me just became. Each queued
/// message killed its own predecessor milliseconds after admission, so an
/// N-message burst left only the last alive and destroyed N-1 turns of work
/// that no user had asked to stop. (codex reaches the same end state from the
/// other side: a replacing task aborts the previous one exactly once, with
/// `TurnAbortReason::Replaced`, and the rest queue.)
///
/// A message with no ticket — a run that never came through a lane, or one the
/// lane already admitted — is unconstrained: `waiting_since` is `None` and a
/// genuine fresh interrupt still cancels. That is the case this predicate must
/// NOT catch, and it is why the cheaper "is the target the run my lane admitted
/// most recently?" is wrong: it also suppresses the real interrupt that arrives
/// while a healthy sibling runs, which is the entire point of the mode.
///
/// Pure, so the rule is unit-testable without an engine.
#[must_use]
pub(super) fn interrupt_targets_an_unseen_run(
    sibling_admitted_at: std::time::Instant,
    waiting_since: Option<std::time::Instant>,
) -> bool {
    waiting_since.is_some_and(|since| sibling_admitted_at > since)
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
/// Three kinds of intent are known to ride requests that reach this path:
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
/// * **Per-request execution directives** — the `RunRequest` fields that shape
///   HOW the run executes rather than what it says: `sandbox_override` (the
///   team dispatcher's isolated git worktree), `max_iterations_override` (a
///   cron job's own Think→Act cap) and `timeout_secs` (heartbeat, team member
///   runs, `sessions_send`'s wait mode). Every one of them is read inside
///   `run_loop`, i.e. after the gate, so an inline fold executes the *running*
///   sibling's sandbox / cap / deadline while reporting success for a request
///   that asked for different ones — `sessions_send` then reads back the
///   sibling's earlier reply as if it were the answer. All three producers are
///   headless dispatchers, never a composer, so deferring them costs no
///   interactive steering.
///
/// `workspace_override` is deliberately NOT on this list even though it is the
/// fourth such directive: the inbound router stamps it on ordinary channel
/// turns and the Panel stamps it on every project-room turn, so presence alone
/// would switch mid-loop steering off for exactly the sessions that use it
/// most. Its intent cannot be lost by a fold when the sibling already runs in
/// the same directory, so it is checked *comparatively* against the steer
/// target in [`try_inject_steering`] — the same shape `model_override` uses.
///
/// Scoped to injection, NOT to [`has_steering_content`], so the `Interrupt`
/// branch still cancels a sibling for an attachment-only or command-only
/// message.
fn carries_more_than_text(request: &RunRequest) -> bool {
    !request.attachments.is_empty()
        || request
            .metadata
            .contains_key(crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY)
        // A side question is a turn of its own on its own session. Folding it
        // into a running sibling would put it in the main context window.
        || request
            .metadata
            .contains_key(crate::gateway::btw::BTW_METADATA_KEY)
        || request.sandbox_override.is_some()
        || request.max_iterations_override.is_some()
        || request.timeout_secs.is_some()
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
/// "Once the burst drains" is [`wake_lane_if_burst_drained`], not the lane's
/// fallback tick: [`defer_for_backpressure`] marks the ticket on the way out so
/// the assistant turn that empties the burst can find it.
///
/// This constant is the single source for the `[execution] max_pending_steering`
/// default (see `ExecutionEngineConfig::max_pending_steering`); the effective
/// bound is the configured one.
pub(super) const MAX_PENDING_STEERING: usize = 16;

/// Count the non-synthetic user messages sitting after this run's *prompt
/// boundary* in `events` — the steering burst already injected into the running
/// loop that the model has not yet answered.
///
/// Mirrors the harness follow-up predicate
/// ([`crate::harness::agent::AgentHarness::has_unanswered_user_message`]) but
/// from the gateway side, which has no access to the harness's `last_prompt_seq`
/// watermark and so reconstructs the boundary from the log's shape. Pure and
/// positional (R10-safe): no intent or relevance judgement.
///
/// # The boundary is the LATER of two events, and the second one is load-bearing
///
/// * the last `AssistantMessage` — everything before it has been answered;
/// * the last [`SessionEvent::RunStarted`] — the marker the harness bridge
///   emits **after** `session_seed` appends the run's opening user message and
///   before the first Think.
///
/// Without the second, the message that *started the current run* is counted as
/// a steering message for the whole of the run's first provider call — seconds
/// on a fast model, a minute on a slow one, and precisely the window in which a
/// user is most likely to add "oh, and also…". Two things went wrong there:
///
/// * **coalescing inverted.** [`try_inject_steering`] suppresses the scratchpad
///   reconcile preamble when `pending > 0` ("an earlier message in this burst
///   already carries it"). With the seed counted, the *first* steer of a run
///   looked like the second and lost the preamble — the opposite of the rule.
/// * **the cap was off by one**, bounding 15 injected steers instead of the
///   configured 16.
///
/// This function's own doc has always said the opening prompt "must not be
/// miscounted as a steering message", and the bare `return 0` implemented that
/// for the first run of a session only — on every later run there *is* a
/// preceding assistant turn, so the boundary fell behind the new seed. Stating
/// the intent was not the same as covering it.
///
/// `RunStarted` cannot mislead in the other direction: per-session mutual
/// exclusion means it is only ever appended while no run is live, so it can
/// never move the boundary out from under a burst that a running loop is
/// carrying. That is also why [`drains_steering_burst`] stays assistant-only —
/// see its doc.
///
/// Falls back to `0` when the log holds neither boundary event (a surface that
/// emits no `RunStarted`, e.g. the L0 fast path): the same fail-open posture as
/// before, never a false positive.
pub(super) fn count_pending_steering(events: &[SessionEventRecord]) -> usize {
    let Some(boundary) = events.iter().rposition(|r| {
        matches!(
            r.event,
            SessionEvent::AssistantMessage { .. } | SessionEvent::RunStarted { .. }
        )
    }) else {
        return 0;
    };
    events[boundary + 1..]
        .iter()
        .filter(|r| matches!(&r.event, SessionEvent::UserMessage { synthetic, .. } if !*synthetic))
        .count()
}

/// Does appending `event` to the session log drain the un-consumed steering
/// burst — i.e. reset [`count_pending_steering`] to zero?
///
/// Derived from that function's boundary rather than restated, and narrower on
/// purpose. The count's boundary is the later of the last assistant turn and
/// the last `RunStarted`; this predicate asks a strictly smaller question —
/// *which newly appended event moves that boundary forward **while a run is
/// live**?* Per-session mutual exclusion means a `RunStarted` is only ever
/// appended when no run is live, so it can never drain a burst a running loop
/// is carrying, and a lane waiter that would care about it is already served by
/// `notify_slot_free` on the previous run's release. An assistant message is
/// therefore exactly the event that empties a live count.
///
/// Whoever moves the *live* half of that boundary has to move this with it, and
/// `the_drain_predicate_agrees_with_the_count_it_resets` fails if they drift.
fn drains_steering_burst(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::AssistantMessage { .. })
}

/// Wake `session_key`'s busy lane when `event` drained its steering burst.
///
/// The lane's other wake edges are both about the run **slot**
/// (`notify_slot_free` on release, `mark_admitted` on claim), and neither fires
/// when the running loop merely answers the burst it is already carrying. So a
/// steer refused by [`try_inject_steering`]'s `pending >= max_pending` branch —
/// which this module documents as "the queue redelivers once the burst
/// drains" — actually waited out `wake_fallback_secs` (30 s by default). That
/// tick is the missed-signal safety net, not the mechanism; this is the
/// mechanism.
///
/// Called from the gateway's one "an event was appended" seam
/// (`session_projector::MessageProjector`'s observer), so it covers every
/// producer of an assistant turn — harness run, fast path, simple engine —
/// rather than whichever one happened to be in view.
pub(crate) fn wake_lane_if_burst_drained(session_key: &str, event: &SessionEvent) {
    if drains_steering_burst(event) {
        crate::gateway::busy_queue::notify_burst_drained(session_key);
    }
}

/// Refuse a steer because the running loop's un-consumed burst is at the cap,
/// telling the lane *why* on the way out. Always returns `false` — the
/// "deferred to the busy queue" answer [`try_inject_steering`] gives its
/// caller.
///
/// Split out of that branch so the mark is reachable from a test: the branch
/// itself sits behind a live `Orchestrator` and a real session read, and a wake
/// edge whose producer is only exercised in production is how the first version
/// of this shipped without one at all.
///
/// A request that never took a ticket (loop tick, goal continuation, delegated
/// child, the OpenAI-compat surface) matches nothing in the lane and the mark
/// is a no-op — the same fail-open posture as `busy_queue::waiting_since`.
/// Complete a successful injection: wake whatever on this session is
/// deliberately asleep, then say so. Always returns `true` — the "injected"
/// answer [`try_inject_steering`] gives its caller, and the mirror image of
/// [`defer_for_backpressure`]'s `false`.
///
/// # Why the wake edge is here and not left inline
///
/// The running loop reads the log at its next turn boundary, and that boundary
/// is milliseconds away — unless the turn's Act phase is parked inside a tool
/// that waits on purpose (`subagent{action:"wait"}` up to 600 s,
/// `bash{process_action:"wait"}` up to 170 s). For that whole window the
/// message above is durably written, the caller was told `HandledInline`, and
/// nothing happens. [`crate::session::steer_signal`] is what those parks select
/// on, and this is its only production producer — see that module's doc for why
/// one producer is structural rather than lucky, and
/// `note_steer_has_exactly_one_production_call_site` for the guard.
///
/// # Why it is split out
///
/// Same reason [`defer_for_backpressure`] is: this arm sits behind a live
/// `Orchestrator` and a real session append, so anything left inline is only
/// ever exercised in production — which is exactly how the backpressure wake
/// edge shipped without one at all. Split out, it is asserted by its EFFECT
/// (`a_successful_injection_wakes_a_tool_parked_on_that_session`): a watch on
/// this session wakes, and a watch on a neighbouring session does not.
fn accept_injection(session_key: &SessionKey, new_run_id: &str) -> bool {
    crate::session::steer_signal::note_steer(session_key);
    // Log new_run_id so an operator can correlate a deferred steering message
    // with the target run if the sibling leaves Running in the
    // find_target→emit window (then only the sibling's Completed-only teardown
    // rescue re-drives it; a Cancelled/Failed sibling defers the message to the
    // next user turn — never dropped, but otherwise opaque).
    tracing::info!(
        session = %session_key.to_key_string(),
        new_run_id = %new_run_id,
        "mid-loop steering: injected user message into running loop",
    );
    true
}

fn defer_for_backpressure(session_key: &str, run_id: &str, pending: usize, cap: usize) -> bool {
    crate::gateway::busy_queue::mark_awaiting_burst_drain(session_key, run_id);
    tracing::warn!(
        session = %session_key,
        pending,
        cap,
        "mid-loop steering: pending burst at cap; deferring to busy-queue backpressure",
    );
    false
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
///
/// # Reaching a turn that is asleep
///
/// "Next turn boundary" is milliseconds away unless the Act phase is sitting
/// inside a tool that parks on purpose — up to 600 s for `subagent` `wait`,
/// 170 s for `bash` `wait`. Appending the event does not shorten that sleep,
/// so a successful injection is also announced on
/// [`crate::session::steer_signal`], which those parks select on alongside
/// their cancel token. The message is answered either way; the edge is what
/// decides whether that happens now or ten minutes from now.
/// Every reason a fold must be refused before anything is read or written —
/// the whole "may this message be folded into that run at all" question, in one
/// place and pure, so each clause can be asserted on its own.
///
/// The clauses that compare against the sibling all say the same thing in
/// different words: the sibling is already committed to a model, a working
/// directory and an execution tier for its whole run, and every one of those is
/// resolved AFTER the admission gate. Folding a message that asked for a
/// different one applies the text and silently drops the directive while
/// answering success. Deferring instead sends it back through the lane, which
/// redelivers it as a fresh run through the whole pipeline — never a drop.
fn fold_is_admissible(enabled: bool, request: &RunRequest, sibling: &BusySibling) -> bool {
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
    // is asking for has to be deferred to the FIFO busy queue.
    if carries_more_than_text(request) {
        return false;
    }

    // The composer's model pill would read `opus` while the answer came from
    // `sonnet`, with no banner and no error.
    if request.model_override != sibling.model_override {
        return false;
    }

    // Same shape for the working directory: the sibling's cwd, project-local
    // skill/AGENTS.md discovery and default shell root were all resolved from
    // ITS `workspace_override` at run start. Equal values are the common case (a
    // room stamps the same path on every turn) and lose nothing in a fold; a
    // DIFFERENT one — a channel turn landing on a session a project-room turn is
    // still running, or the reverse — would silently execute in the other
    // directory, with file-writing consequences.
    if request.workspace_override != sibling.workspace_override {
        return false;
    }

    // Same shape again, and this one is a permission boundary rather than a
    // preference: a user who flips the composer's tier pill from `auto` to
    // `plan` and sends while a run is in flight had their text executed at
    // `auto` — mutating tools running that `plan` refuses, with no approval card
    // anywhere and `persist_session_exec_tier` never reached.
    //
    // Compared, not presence-checked: `session_dials_for_send` puts `exec_tier`
    // on EVERY Panel send (`composer_dials.rs`), so listing the key in
    // `carries_more_than_text` would switch mid-loop steering off for every
    // Panel conversation — the trap `workspace_override`'s clause above already
    // records. Equal (or equally absent) values lose nothing in a fold.
    let tier_key = crate::config::types::policies::EXEC_TIER_SESSION_KEY;
    if request.metadata.get(tier_key) != sibling.metadata.get(tier_key) {
        return false;
    }

    true
}

pub(super) async fn try_inject_steering(
    enabled: bool,
    max_pending: usize,
    sibling: &BusySibling,
    orchestrator: &OnceLock<Arc<Orchestrator>>,
    request: &RunRequest,
    new_run_id: &str,
) -> bool {
    if !fold_is_admissible(enabled, request, sibling) {
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
        return defer_for_backpressure(
            &request.session_key.to_key_string(),
            new_run_id,
            pending,
            max_pending,
        );
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
    // The raw input, and nothing derived from it. `carries_more_than_text`
    // above has already deferred every attachment-bearing request to the lane,
    // so there is no second content channel left to represent here — the
    // attachment-marker renderer this line used to call had no reachable
    // production input and was cut (P6). Do NOT reintroduce markers: the normal
    // path deliberately keeps the stored message and the derived session title
    // equal to the raw input, and a marker would make a steered turn read
    // differently from the same text sent while idle.
    let text = apply_reconcile_preamble(request.input.clone(), has_active_scratchpad);
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
        // The INTERRUPTING request's scope, not the running one's: in a room
        // the whole point is that whoever steers is not necessarily whoever
        // started the run, and the transcript has to say which.
        author_user_id: crate::scope::room_author_from_metadata(&request.metadata),
    };

    match orchestrator
        .session_service
        .emit_event(&session_id, event)
        .await
    {
        Ok(_) => accept_injection(&request.session_key, new_run_id),
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

    /// Every per-request execution directive is resolved inside `run_loop`,
    /// i.e. AFTER the admission gate — so an inline fold runs the sibling's
    /// sandbox / iteration cap / deadline and returns `Ok(())` for a request
    /// that asked for different ones. Each field is asserted on its own: a
    /// registration that covers three of the four is the failure mode this
    /// guard exists for.
    #[test]
    fn a_per_request_execution_directive_is_deferred_not_folded() {
        let plain = run_request("s1", "keep going");
        assert!(!carries_more_than_text(&plain));

        let mut capped = run_request("s1", "keep going");
        capped.max_iterations_override = Some(12);
        assert!(
            carries_more_than_text(&capped),
            "a cron job's own Think→Act cap is read after the gate"
        );

        let mut bounded = run_request("s1", "keep going");
        bounded.timeout_secs = Some(30);
        assert!(
            carries_more_than_text(&bounded),
            "a wait-mode `sessions_send` reads back the sibling's earlier reply \
             when its own deadline is folded away"
        );

        let mut isolated = run_request("s1", "keep going");
        isolated.sandbox_override = Some(std::sync::Arc::new(crate::sandbox::NoopSandbox));
        assert!(
            carries_more_than_text(&isolated),
            "a team member's isolated worktree cannot be applied to a sibling \
             already running outside it"
        );

        // The one directive that is NOT presence-checked: a project room stamps
        // the same path on every turn, so presence alone would disable mid-loop
        // steering for rooms. Its check is comparative, in `try_inject_steering`.
        let mut in_project = run_request("s1", "keep going");
        in_project.workspace_override = Some(std::path::PathBuf::from("/tmp/proj"));
        assert!(
            !carries_more_than_text(&in_project),
            "workspace_override is compared against the steer target, not presence-checked"
        );
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

    #[test]
    fn a_btw_request_is_never_folded_into_a_running_sibling() {
        let mut request = run_request("s1", "/btw why?");
        request.metadata.insert(
            crate::gateway::btw::BTW_METADATA_KEY.to_string(),
            "why?".to_string(),
        );
        assert!(
            carries_more_than_text(&request),
            "a btw turn folded as steering text lands in the main context window"
        );
    }

    use crate::gateway::channel::Attachment;
    use crate::sync_primitives::{AtomicU32, AtomicU64};

    fn sibling_matching(request: &RunRequest) -> BusySibling {
        BusySibling {
            run_id: "r-old".to_string(),
            metadata: request.metadata.clone(),
            model_override: request.model_override.clone(),
            workspace_override: request.workspace_override.clone(),
            admitted_at: std::time::Instant::now(),
        }
    }

    /// A steer that asks for a DIFFERENT execution tier must be deferred, not
    /// folded — the tier is a permission boundary, and it is resolved after the
    /// gate, so a fold runs the message under the sibling's tier.
    ///
    /// The pill rides every Panel send, so the same predicate must NOT defer on
    /// mere presence: both halves are asserted here because a fix that defers on
    /// presence would turn mid-loop steering off for every Panel conversation
    /// and still pass the interesting half.
    #[test]
    fn a_steer_that_changes_the_execution_tier_is_deferred_not_folded() {
        let tier_key = crate::config::types::policies::EXEC_TIER_SESSION_KEY;

        // No tier anywhere (a channel turn): admissible.
        let plain = run_request("s1", "keep going");
        assert!(
            fold_is_admissible(true, &plain, &sibling_matching(&plain)),
            "an ordinary text steer with no dials must still fold"
        );

        // The Panel case: the pill rides every send and nothing changed.
        let mut same = run_request("s1", "keep going");
        same.metadata
            .insert(tier_key.to_string(), "auto".to_string());
        assert!(
            fold_is_admissible(true, &same, &sibling_matching(&same)),
            "exec_tier rides EVERY Panel send, so an unchanged pill must not \
             disable mid-loop steering"
        );

        // The pill was flipped mid-run.
        let mut flipped = run_request("s1", "keep going");
        flipped
            .metadata
            .insert(tier_key.to_string(), "plan".to_string());
        assert!(
            !fold_is_admissible(true, &flipped, &sibling_matching(&same)),
            "a message asking for `plan` folded into an `auto` sibling executes \
             at `auto`: mutating tools run without the approval `plan` requires, \
             and the pick is never stamped onto the session"
        );

        // And the reverse direction — a request with no tier folded into a
        // sibling that has one is the same divergence seen from the other side.
        assert!(
            !fold_is_admissible(true, &plain, &sibling_matching(&same)),
            "an untiered request must not inherit the sibling's tier by folding"
        );
    }

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
                admitted_at: std::time::Instant::now(),
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

    // ---- interrupt_targets_an_unseen_run (burst self-annihilation) ----

    #[test]
    fn a_fresh_interrupt_supersedes_the_run_that_was_already_going() {
        let admitted = std::time::Instant::now();
        let arrived_later = admitted + std::time::Duration::from_millis(1);
        assert!(
            !interrupt_targets_an_unseen_run(admitted, Some(arrived_later)),
            "the run was already up when this message was written — that is \
             exactly what Interrupt means"
        );
    }

    #[test]
    fn an_interrupt_never_supersedes_a_run_admitted_after_it_started_waiting() {
        let arrived = std::time::Instant::now();
        let admitted_later = arrived + std::time::Duration::from_millis(1);
        assert!(
            interrupt_targets_an_unseen_run(admitted_later, Some(arrived)),
            "this is the predecessor's run: it did not exist when the message \
             was written, so cancelling it destroys work nobody asked to stop"
        );
    }

    #[test]
    fn a_message_that_never_queued_is_unconstrained() {
        // Producers with no lane ticket — the OpenAI-compat surface, a loop
        // tick, a delegated child — keep the pre-existing behaviour exactly.
        // Getting this arm wrong would disable Interrupt outright rather than
        // fix the burst.
        assert!(!interrupt_targets_an_unseen_run(
            std::time::Instant::now(),
            None
        ));
    }

    /// The composition, against the real lane: A and B arrive as a burst, A is
    /// admitted, and B must NOT read A's brand-new run as its interrupt target.
    /// Then a genuinely later message does.
    ///
    /// Before this rule, every message in an interrupt-mode burst killed its
    /// own predecessor milliseconds after the lane admitted it, so N messages
    /// left one survivor and N-1 destroyed turns.
    #[test]
    fn a_burst_of_interrupts_does_not_eat_itself() {
        use crate::gateway::busy_queue;
        let key = "agent:burst";

        // R0 was already running when the burst arrived.
        let r0_admitted = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(2));

        let a = busy_queue::register(key, 8, "run-a").expect("lane accepts A");
        let b = busy_queue::register(key, 8, "run-b").expect("lane accepts B");

        // A supersedes R0 — the one cancellation the burst is entitled to.
        assert!(!interrupt_targets_an_unseen_run(
            r0_admitted,
            busy_queue::waiting_since(key, "run-a")
        ));

        // A is admitted and becomes the live run.
        busy_queue::mark_admitted(key, "run-a");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let a_admitted = std::time::Instant::now();

        // B, still waiting since before A ever ran, must leave it alone.
        assert!(
            interrupt_targets_an_unseen_run(a_admitted, busy_queue::waiting_since(key, "run-b")),
            "B was queued behind A; A's run is not the task B meant to interrupt"
        );

        // A message that arrives now, with A visibly running, still interrupts.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let c = busy_queue::register(key, 8, "run-c").expect("lane accepts C");
        assert!(
            !interrupt_targets_an_unseen_run(a_admitted, busy_queue::waiting_since(key, "run-c")),
            "suppressing this one would be the cheap-and-wrong rule: it is the \
             genuine mid-run interrupt the mode exists for"
        );

        drop(a);
        drop(b);
        drop(c);
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
                author_user_id: None,
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

    fn rec_run_started() -> SessionEventRecord {
        SessionEventRecord {
            seq: 0,
            created_at_ms: now_ms(),
            event: SessionEvent::RunStarted {
                run_id: uuid::Uuid::new_v4().to_string(),
                at: now_ms(),
                project_root: None,
            },
        }
    }

    /// Round-10 — the run's own opening message is not a steering message, on
    /// **every** run and not merely the first one on a session.
    ///
    /// `pending_is_zero_before_any_assistant_turn` above covers the first run
    /// (no assistant turn exists, so the boundary search finds nothing). On the
    /// second and later runs there IS a preceding assistant turn, so the old
    /// boundary fell behind the new seed and counted it — for the whole of that
    /// run's first provider call. Two visible consequences, both silent:
    /// the *first* steer of the run looked like the second and lost the
    /// scratchpad reconcile preamble, and the burst cap bound N-1.
    #[test]
    fn a_later_runs_seed_prompt_is_not_counted_as_a_steer() {
        // Turn 1 answered, then turn 2 starts. Nothing has been steered yet.
        let fresh = [
            rec_user("task-1", false),
            rec_run_started(),
            rec_assistant("done"),
            rec_user("task-2", false),
            rec_run_started(),
        ];
        assert_eq!(
            count_pending_steering(&fresh),
            0,
            "the message that started this run is what the run is answering, \
             not a steer waiting to be answered"
        );

        // ...and the first real steer of that run is its first, so the
        // reconcile preamble applies to it.
        let mut steered = fresh.to_vec();
        steered.push(rec_user("actually, also do X", false));
        assert_eq!(count_pending_steering(&steered), 1);
    }

    /// The narrower half of the same boundary: `RunStarted` is only ever
    /// appended while no run is live (per-session mutual exclusion), so it must
    /// not be treated as an event that drains a LIVE burst. Widening
    /// `drains_steering_burst` alongside the count would wake lane waiters on
    /// an edge that can never help them.
    #[test]
    fn a_run_start_is_not_a_live_burst_drain() {
        assert!(!drains_steering_burst(&rec_run_started().event));
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

    /// The wake edge's predicate and the count it exists to reset have to agree.
    /// Deliberately not a second `matches!` (that would restate
    /// `drains_steering_burst`, and restatements agree with anything): build a
    /// real burst, append the event the predicate calls a drain, and assert the
    /// COUNT goes to zero. Move `count_pending_steering`'s boundary — to a
    /// prompt watermark, say — without moving the predicate and this fails.
    #[test]
    fn the_drain_predicate_agrees_with_the_count_it_resets() {
        let mut events = vec![rec_user("task", false), rec_assistant("turn-1")];
        for i in 0..3 {
            events.push(rec_user(&format!("steer-{i}"), false));
        }
        assert_eq!(count_pending_steering(&events), 3);

        let drain = rec_assistant("turn-2");
        assert!(drains_steering_burst(&drain.event));
        events.push(drain);
        assert_eq!(
            count_pending_steering(&events),
            0,
            "the event the wake edge fires on must be the one that empties the burst"
        );

        // The counter-example, so a predicate that answered `true` for
        // everything (waking the lane on every tool output) fails here too.
        let not_a_drain = rec_user("steer-4", false);
        assert!(!drains_steering_burst(&not_a_drain.event));
        events.push(not_a_drain);
        assert_eq!(count_pending_steering(&events), 1);
    }

    /// The producer half of that edge. `try_inject_steering`'s cap branch is
    /// unreachable without a live `Orchestrator`, so the mark it leaves behind
    /// is asserted through the helper it delegates to — and asserted by its
    /// EFFECT: the lane wakes a waiter it would otherwise ignore. Drop the
    /// `mark_awaiting_burst_drain` call and this goes red.
    #[tokio::test]
    async fn a_backpressure_defer_marks_the_ticket_the_drain_edge_looks_for() {
        use crate::gateway::busy_queue;
        let key = "steer-test-defer-marks";
        let ticket = busy_queue::register(key, 8, "steer-defer-run").expect("lane accepts it");
        let wake = ticket.wake_handle();
        let parked = wake.notified();
        tokio::pin!(parked);

        // Unmarked, the drain edge deliberately ignores this lane.
        busy_queue::notify_burst_drained(key);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut parked)
                .await
                .is_err(),
            "a lane with no backpressured waiter must not wake on an assistant turn"
        );

        assert!(
            !defer_for_backpressure(key, "steer-defer-run", 16, 16),
            "deferring must report `not injected` to try_inject_steering"
        );
        busy_queue::notify_burst_drained(key);
        tokio::time::timeout(std::time::Duration::from_millis(500), &mut parked)
            .await
            .expect("after the defer, the drained burst must wake the message it deferred");
    }

    /// Round-10 — the success arm's producer half, asserted the same way and
    /// for the same reason: a wake edge that only ever runs in production is a
    /// wake edge nobody notices is missing.
    ///
    /// Two assertions, because the load-bearing part is not "an edge fired" but
    /// "it fired on the INTERRUPTING request's session". Both sides derive the
    /// registry key inside `steer_signal`, and this is what proves the producer
    /// hands it the right `SessionKey` in the first place.
    #[tokio::test]
    async fn a_successful_injection_wakes_a_tool_parked_on_that_session() {
        use crate::session::steer_signal;

        let steered = SessionKey::peer("main", "steer-accept-wakes");
        let bystander = SessionKey::peer("main", "steer-accept-bystander");
        let mut parked = steer_signal::watch_session(&steered);
        let mut elsewhere = steer_signal::watch_session(&bystander);

        assert!(
            accept_injection(&steered, "run-accept"),
            "a successful injection reports `injected` to try_inject_steering"
        );

        tokio::time::timeout(std::time::Duration::from_millis(500), parked.steered())
            .await
            .expect("a tool parked on the steered session must be woken");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), elsewhere.steered())
                .await
                .is_err(),
            "a park on another session must be left alone"
        );
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
