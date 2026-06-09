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
pub(super) fn find_steering_target(
    runs: &HashMap<String, ActiveRun>,
    new_run_id: &str,
    session_key: &SessionKey,
) -> bool {
    find_steering_target_id(runs, new_run_id, session_key).is_some()
}

/// Render the user-visible session text for a request, mirroring the attachment
/// markers used when a message is first persisted in `execute()`. Extracted so
/// the steering-injection path and the normal store path stay byte-identical.
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

/// Upper bound on un-consumed steering messages a single run may accumulate
/// before the gateway applies backpressure (OpenSquilla `max_pending_per_session`
/// / OpenClaw FIFO-cap parity). A flooding channel that keeps sending while the
/// agent is mid-loop would otherwise append unbounded `UserMessage` events to the
/// live log, bloating the very next prompt. Past the cap the injection is
/// rejected so the inbound router's existing exponential busy/retry back-off
/// redelivers the message once the burst drains — backpressure, never a drop.
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

/// Try to deliver `request` as a mid-loop steering message into the session's
/// already-running loop. Returns `true` when the message was injected (the
/// caller should treat the run as accepted and skip the `AgentBusy` path).
///
/// Returns `false` — leaving the legacy busy/retry behaviour intact — when:
/// * steering is disabled by config, or
/// * no *other* run is active on this exact session (a cross-session busy
///   agent is genuinely unavailable; the inbound router should still retry), or
/// * the orchestrator / session service is not yet wired, or
/// * the run's un-consumed steering burst is already at [`MAX_PENDING_STEERING`]
///   (backpressure — the router retries once the burst drains), or
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
/// (`AgentHarness::last_prompt_log_len`), so a message at or beyond that
/// boundary — including one wedged before the trailing assistant message —
/// re-enters the loop and is answered in the same run. Only if the run has
/// already torn down (no live loop to continue) does it defer to the next
/// interaction, when `get_events` returns it alongside the next user turn. It
/// is never dropped.
pub(super) async fn try_inject_steering(
    enabled: bool,
    active_runs: &RwLock<HashMap<String, ActiveRun>>,
    orchestrator: &OnceLock<Arc<Orchestrator>>,
    request: &RunRequest,
    new_run_id: &str,
) -> bool {
    if !enabled {
        return false;
    }

    {
        let runs = active_runs.read().await;
        if !find_steering_target(&runs, new_run_id, &request.session_key) {
            return false;
        }
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
    let pending = match orchestrator
        .session_service
        .get_events(&session_id, None, None)
        .await
    {
        Ok(events) => count_pending_steering(&events),
        Err(_) => 0,
    };

    // Bound: cap the un-consumed steering burst. Past the cap, reject so the
    // inbound router's exponential busy/retry back-off redelivers once the loop
    // drains the burst or goes idle — backpressure against a flooding channel,
    // not a drop.
    if pending >= MAX_PENDING_STEERING {
        tracing::warn!(
            session = %request.session_key.to_key_string(),
            pending,
            "mid-loop steering: pending burst at cap; deferring to busy/retry backpressure",
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
            tracing::info!(
                session = %request.session_key.to_key_string(),
                "mid-loop steering: injected user message into running loop",
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                session = %request.session_key.to_key_string(),
                error = %e,
                "mid-loop steering: failed to inject; falling back to busy/retry",
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
