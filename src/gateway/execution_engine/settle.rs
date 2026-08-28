//! Terminal turn settle — the work a turn owes regardless of how it ended.
//!
//! # Why this is its own seam
//!
//! `execute()` ends in a `match` with two arms, and "what a turn leaves behind"
//! kept getting written into the `Ok` one. Each time, the failure is the same
//! shape and it is silent: the turn *did* move the conversation — the harness
//! appends the user message before dispatch, and any partial assistant text and
//! the error receipt are persisted too — so anything keyed on "this turn
//! happened" is still owed, and skipping it leaves state that describes a world
//! where the turn never occurred.
//!
//! Two of those have already been fixed one at a time: `announce_turn_end` was
//! moved into both arms, and `set_session_source_channel` was hoisted ahead of
//! the run entirely. The two that were left — persisting the user-chosen
//! project folder and generating the conversation topic — are worse than the
//! ones already fixed, because they are keyed on a **one-shot latch**.
//!
//! `is_first_message` is computed before dispatch from an empty history. After
//! a failed first turn the history is no longer empty, so it is `false` for
//! every subsequent turn and the two stamps can never fire again. The user
//! picks a folder, the first message fails (rate limit, stale key, Stop on a
//! slow turn), and the binding the Panel reads back to restore that folder is
//! null forever: every later turn in that conversation runs in
//! `~/.aleph/workspaces/<agent>` with the model told so, and the only escape is
//! to start a new conversation. The topic loss rides along — the sidebar row
//! keeps its untitled default permanently.
//!
//! So the settle work lives here, in one function both arms call, rather than
//! in a block that has to be remembered twice.

use tracing::{info, warn};

use super::engine::ExecutionEngine;
use super::RunRequest;
use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// The one-shot stamps a conversation's first turn owes, on either outcome.
    ///
    /// Both are best-effort and both are spawned: a metadata write failure or a
    /// slow topic model must never block or fail the turn, which has already
    /// produced its answer (or its receipt) by the time this runs.
    ///
    /// `is_first_message` is the caller's pre-dispatch latch, not a re-read of
    /// the history — by settle time the harness has appended the user message,
    /// so a re-read would answer `false` for every turn including the one this
    /// is for.
    pub(super) fn settle_first_message(&self, request: &RunRequest, is_first_message: bool) {
        if !is_first_message {
            return;
        }
        self.stamp_project_root(request);
        self.spawn_auto_topic(request);
    }

    /// Persist the user-chosen project folder onto the session so the Panel can
    /// restore it after a reload (project workspaces G3).
    ///
    /// Stamped on the first message, mirroring source-channel/topic stamping;
    /// the project↔session binding is fixed at session creation, so a later
    /// switch starts a fresh session and re-stamps.
    fn stamp_project_root(&self, request: &RunRequest) {
        let (Some(sm), Some(root)) = (
            self.session_manager.clone(),
            request
                .workspace_override
                .as_ref()
                .map(|p| p.display().to_string()),
        ) else {
            return;
        };
        let key = request.session_key.clone();
        tokio::spawn(async move {
            if let Err(e) = sm.set_project_root(&key, Some(&root)).await {
                warn!(error = %e, "failed to persist session project_root");
            }
        });
    }

    /// Auto-generate the conversation topic from the first real message.
    fn spawn_auto_topic(&self, request: &RunRequest) {
        let (Some(sm), Some(eb)) = (self.session_manager.clone(), self.event_bus.clone()) else {
            info!(
                "Auto-topic: skipped (session_manager={}, event_bus={})",
                self.session_manager.is_some(),
                self.event_bus.is_some()
            );
            return;
        };
        let topic_provider = self
            .provider_registry
            .get("haiku")
            .unwrap_or_else(|| self.provider_registry.default_provider());
        let topic_session_key = request.session_key.clone();
        let topic_message = request.input.clone();
        info!(
            session_key = %topic_session_key.to_key_string(),
            "Auto-topic: spawning generation for first message"
        );
        tokio::spawn(async move {
            let topic_text =
                super::topic::generate_conversation_topic(&topic_provider, &topic_message).await;

            if let Err(e) = sm.set_topic(&topic_session_key, &topic_text).await {
                warn!(error = %e, "Auto-topic: failed to persist topic");
            } else {
                let event_json = serde_json::json!({
                    "method": "stream.session_updated",
                    "params": {
                        "session_key": topic_session_key.to_key_string(),
                        "topic": topic_text,
                    }
                });
                eb.publish(event_json.to_string());
                info!(
                    session_key = %topic_session_key.to_key_string(),
                    topic = %topic_text,
                    "Auto-topic: session topic set"
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::utils::source_scan::{code_text, production_prefix};

    /// Both terminal arms of `execute()` must settle the first-message stamps.
    ///
    /// Counting call sites is not enough: two calls satisfy a count while both
    /// sit in the `Ok` arm, which is the state this guard was written to end.
    /// So it names the **failure path** specifically — the settle call must
    /// appear after `Err(e) =>` and before that arm's closing `Err(e)`.
    ///
    /// Source-level because the runtime shapes are indistinguishable: a
    /// conversation whose first turn failed and one that was never given a
    /// folder both have a null `project_root`, and nothing logs the difference.
    #[test]
    fn the_failure_arm_settles_the_first_message_stamps_too() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/gateway/execution_engine/execute.rs");
        let body = code_text(&production_prefix(
            &std::fs::read_to_string(&path).expect("execute.rs"),
        ));

        let err_arm_at = body.find("Err(e) => {").expect(
            "execute()'s terminal match no longer has an `Err(e) => {` arm — this guard \
                     is scanning for a shape that stopped existing, so its green means nothing",
        );
        let arm = &body[err_arm_at..];
        let settle_at = arm.find("settle_first_message(");

        assert!(
            settle_at.is_some(),
            "the Err arm of execute()'s terminal match does not call \
             `settle_first_message`. A conversation whose FIRST turn fails then \
             loses its project folder and its topic permanently: \
             `is_first_message` is a one-shot latch over an empty history, and \
             the harness has already appended the user message by the time the \
             arm runs, so no later turn can re-fire it. Silent — the Panel just \
             shows an untitled conversation running in the default workspace."
        );

        // And the success arm still does. A fix that moved the call instead of
        // duplicating it would satisfy the assertion above while regressing the
        // path that always worked.
        assert!(
            body[..err_arm_at].contains("settle_first_message("),
            "the success arm of execute()'s terminal match stopped calling \
             `settle_first_message` — the stamps moved rather than being shared"
        );
    }
}
