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
use crate::session::events::{now_ms, MessageContent, SessionEvent};
use crate::session::service::SessionId;
use crate::sync_primitives::Arc;

/// Decide whether `session_key` already has *another* `Running` sibling run.
///
/// Pure and synchronous so the same-session predicate can be unit-tested
/// without spinning up an orchestrator. `new_run_id` is the just-reserved run
/// that lost the `try_start_run` race; it is excluded so a run never counts
/// itself as its own steering target.
pub(super) fn find_steering_target(
    runs: &HashMap<String, ActiveRun>,
    new_run_id: &str,
    session_key: &SessionKey,
) -> bool {
    runs.iter().any(|(id, run)| {
        id != new_run_id
            && matches!(run.state, RunState::Running)
            && &run.request.session_key == session_key
    })
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

/// Try to deliver `request` as a mid-loop steering message into the session's
/// already-running loop. Returns `true` when the message was injected (the
/// caller should treat the run as accepted and skip the `AgentBusy` path).
///
/// Returns `false` — leaving the legacy busy/retry behaviour intact — when:
/// * steering is disabled by config, or
/// * no *other* run is active on this exact session (a cross-session busy
///   agent is genuinely unavailable; the inbound router should still retry), or
/// * the orchestrator / session service is not yet wired, or
/// * appending the event failed.
///
/// # Boundary
///
/// If injection lands during the running loop's *final* LLM call, the message
/// sits at the tail of the event log unanswered until the next interaction,
/// when `get_events` returns it alongside the next user turn. It is never
/// dropped — only, in that one race, deferred. This is strictly better than the
/// previous "blocked until the whole run finishes" behaviour.
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
    // 3b: if this session is driving a scratchpad execution list, tell the
    // model to reconcile it before continuing. Mechanical lookup, no I/O.
    let has_active_scratchpad =
        crate::builtin_tools::scratchpad_registry::active(&request.session_key.to_key_string())
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
}
