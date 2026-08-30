//! Wizard session state machine.
//!
//! Manages the lifecycle of a wizard session, coordinating between
//! the flow implementation and the client.

use crate::sync_primitives::{Arc, RwLock};
use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error};

use super::prompter::{PendingAnswer, RpcPrompter};
use super::types::{StepType, WizardNextResult, WizardStatus, WizardStep};

/// Wizard session errors
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WizardSessionError {
    #[error("Session cancelled")]
    Cancelled,

    #[error("Session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("Step not found: {0}")]
    StepNotFound(String),

    #[error("Invalid answer: {0}")]
    InvalidAnswer(String),

    #[error("Timed out after {timeout_secs}s waiting for an answer to step '{step_id}'")]
    AnswerTimeout { step_id: String, timeout_secs: u64 },

    #[error("Flow error: {0}")]
    FlowError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Validate a client answer against the step that requested it.
///
/// The client is untrusted: `wizard.answer` accepts arbitrary JSON, and flows
/// deserialise it straight into config values (provider ids, model ids). Without
/// this gate a client can smuggle a value that was never offered — e.g. an
/// unlisted provider or a `disabled` option — past the select UI. Shape checks
/// (bool/string) also turn a confusing downstream `InvalidAnswer("Expected
/// boolean")` from the prompter into an error naming the offending step.
///
/// Steps without an option list cannot be domain-checked, so they only get the
/// shape check.
fn validate_answer(step: &WizardStep, value: &Value) -> Result<(), WizardSessionError> {
    let offered = |candidate: &Value| -> bool {
        step.options
            .as_ref()
            .is_none_or(|options| options.iter().any(|o| !o.disabled && &o.value == candidate))
    };

    match step.step_type {
        // Notes carry no input; anything (including null) is an acknowledgement.
        StepType::Note => Ok(()),
        StepType::Confirm => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(WizardSessionError::InvalidAnswer(format!(
                    "Step '{}' expects a boolean",
                    step.id
                )))
            }
        }
        StepType::Text => {
            if value.is_string() {
                Ok(())
            } else {
                Err(WizardSessionError::InvalidAnswer(format!(
                    "Step '{}' expects a string",
                    step.id
                )))
            }
        }
        StepType::Select => {
            if offered(value) {
                Ok(())
            } else {
                Err(WizardSessionError::InvalidAnswer(format!(
                    "Answer for step '{}' is not one of the offered options",
                    step.id
                )))
            }
        }
        StepType::MultiSelect => {
            let Some(items) = value.as_array() else {
                return Err(WizardSessionError::InvalidAnswer(format!(
                    "Step '{}' expects an array",
                    step.id
                )));
            };
            if items.iter().all(offered) {
                Ok(())
            } else {
                Err(WizardSessionError::InvalidAnswer(format!(
                    "Answer for step '{}' contains values that are not offered options",
                    step.id
                )))
            }
        }
    }
}

/// A wizard flow that can be run by a session
#[async_trait]
pub trait WizardFlow: Send + Sync {
    /// Run the wizard flow
    ///
    /// The flow should use the prompter to ask questions and collect answers.
    /// Returns Ok(()) on success, Err on failure or cancellation.
    async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError>;

    /// Get the flow name (for logging)
    fn name(&self) -> &str {
        "wizard"
    }
}

/// Wizard session managing the flow execution
pub struct WizardSession {
    id: String,
    status: Arc<RwLock<WizardStatus>>,
    current_step: Arc<RwLock<Option<WizardStep>>>,
    // step_tx is intentionally NOT stored here: only the prompter (inside the spawned
    // flow task) holds a sender.  When the flow task completes the prompter is dropped,
    // the last sender goes away, the channel closes, and `next()` receives None which
    // lets it surface the Done/Error result.  Storing step_tx here would keep the
    // channel alive indefinitely and cause `next()` to block forever.
    step_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<WizardStep>>>,
    answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
    error: Arc<RwLock<Option<String>>>,
    cancel_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
}

impl WizardSession {
    /// Create a new wizard session and start the flow
    ///
    /// # Panics
    ///
    /// Panics if called outside a tokio runtime context: the flow is driven by
    /// a task spawned here, not lazily on first `next()`.
    #[must_use]
    pub fn new(flow: Box<dyn WizardFlow>) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let (step_tx, step_rx) = mpsc::channel(16);
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let answers: Arc<RwLock<HashMap<String, PendingAnswer>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Create prompter for the flow.
        // step_tx is moved into the prompter; WizardSession does NOT keep a copy.
        // This ensures the channel closes when the flow task ends (prompter drop),
        // which lets session.next()'s rx.recv() return None and surface Done/Error.
        let prompter = RpcPrompter::new(step_tx, answers.clone());

        let session = Self {
            id: id.clone(),
            status: Arc::new(RwLock::new(WizardStatus::Running)),
            current_step: Arc::new(RwLock::new(None)),
            step_rx: Arc::new(tokio::sync::Mutex::new(step_rx)),
            answers,
            error: Arc::new(RwLock::new(None)),
            cancel_tx: Arc::new(RwLock::new(Some(cancel_tx))),
        };

        // Spawn the flow runner
        let status = session.status.clone();
        let error = session.error.clone();
        let flow_name = flow.name().to_string();

        tokio::spawn(async move {
            debug!(id = %id, flow = %flow_name, "Starting wizard flow");

            tokio::select! {
                result = flow.run(&prompter) => {
                    match result {
                        Ok(()) => {
                            debug!(id = %id, "Wizard flow completed");
                            Self::settle(&status, WizardStatus::Done);
                        }
                        Err(WizardSessionError::Cancelled) => {
                            debug!(id = %id, "Wizard flow cancelled");
                            Self::settle(&status, WizardStatus::Cancelled);
                        }
                        Err(e) => {
                            error!(id = %id, error = %e, "Wizard flow error");
                            // Record the error message only if this call actually
                            // won the transition to Error (terminal is sticky).
                            if Self::settle(&status, WizardStatus::Error) {
                                *error.write().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
                            }
                        }
                    }
                }
                _ = cancel_rx => {
                    debug!(id = %id, "Wizard flow cancelled via signal");
                    Self::settle(&status, WizardStatus::Cancelled);
                }
            }
        });

        session
    }

    /// Transition `status` to a terminal state, but only from `Running`.
    ///
    /// Terminal states are sticky: once `Done`/`Cancelled`/`Error` is set, later
    /// transitions are ignored. This guarantees the first settled outcome wins,
    /// so a late `cancel()` cannot clobber a completed flow's `Done` status.
    /// Returns `true` iff this call performed the transition.
    fn settle(status: &RwLock<WizardStatus>, terminal: WizardStatus) -> bool {
        let mut guard = status.write().unwrap_or_else(|e| e.into_inner());
        if *guard == WizardStatus::Running {
            *guard = terminal;
            true
        } else {
            false
        }
    }

    /// Get the session ID
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the current status
    #[must_use]
    pub fn status(&self) -> WizardStatus {
        *self.status.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Get the next step (blocks until a step is available or done)
    pub async fn next(&self) -> WizardNextResult {
        let settled = self.is_done();

        // Acquire the receiver. Once the session has settled we must not block
        // behind a concurrent `next()` that still owns the receiver — report the
        // terminal status instead of hanging.
        let mut rx = if settled {
            match self.step_rx.try_lock() {
                Ok(rx) => rx,
                Err(_) => return self.terminal_result(),
            }
        } else {
            self.step_rx.lock().await
        };

        // Buffered steps outrank a terminal status. Notes are fire-and-forget
        // (`prompt_no_wait`), so a flow can queue its outro and settle `Done`
        // before the client polls; short-circuiting on status alone would drop
        // those steps on the floor.
        match rx.try_recv() {
            Ok(step) => return self.record_step(step),
            Err(mpsc::error::TryRecvError::Disconnected) => return self.terminal_result(),
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        if settled || self.is_done() {
            return self.terminal_result();
        }

        // Wait for next step from flow
        match rx.recv().await {
            Some(step) => self.record_step(step),
            None => self.terminal_result(),
        }
    }

    /// Record `step` as the current step and wrap it as a `next()` result.
    fn record_step(&self, step: WizardStep) -> WizardNextResult {
        *self.current_step.write().unwrap_or_else(|e| e.into_inner()) = Some(step.clone());
        WizardNextResult::step(step)
    }

    /// Build the result for a session with no more steps to deliver.
    fn terminal_result(&self) -> WizardNextResult {
        match self.status() {
            WizardStatus::Done => WizardNextResult::done(),
            WizardStatus::Cancelled => WizardNextResult::cancelled(),
            WizardStatus::Error => {
                let error = self.error.read().unwrap_or_else(|e| e.into_inner()).clone();
                WizardNextResult::error(error.unwrap_or_else(|| "Unknown error".to_string()))
            }
            // Still `Running` with a drained/closed step channel: the flow task
            // terminated without settling (likely panicked).
            _ => WizardNextResult::error("Wizard flow terminated unexpectedly".to_string()),
        }
    }

    /// Answer the current step
    pub async fn answer(&self, step_id: &str, value: Value) -> Result<(), WizardSessionError> {
        // Snapshot the current step, then drop the lock: validation needs the
        // step's type and options, and the `answers` lock must never be taken
        // while holding `current_step`.
        let current = self
            .current_step
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let Some(step) = current else {
            return Err(WizardSessionError::StepNotFound(step_id.to_string()));
        };
        if step.id != step_id {
            return Err(WizardSessionError::InvalidAnswer(format!(
                "Expected answer for step '{}', got '{step_id}'",
                step.id
            )));
        }

        // Reject answers the step never offered before they reach the flow.
        validate_answer(&step, &value)?;

        // Find and resolve the pending answer
        let sender = {
            let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
            answers.remove(step_id).map(|p| p.sender)
        };

        match sender {
            // Never interpolate the answer into the error: `sensitive` steps
            // carry API keys, and this string reaches the client and the logs.
            Some(sender) => sender.send(value).map_err(|_| {
                WizardSessionError::Internal(format!(
                    "Failed to deliver answer for step '{step_id}': flow is no longer waiting"
                ))
            }),
            // Notes are pushed without registering a pending answer. A client
            // that answers every step it is handed is not wrong, so treat this
            // as an acknowledgement rather than stalling the wizard.
            None if step.step_type == StepType::Note => {
                debug!(step_id = %step_id, "Acknowledged answer for note step");
                Ok(())
            }
            // The id matched the current step but nothing is pending, so the
            // step was already answered — say that instead of "not found".
            None => Err(WizardSessionError::InvalidAnswer(format!(
                "Step '{step_id}' has already been answered"
            ))),
        }
    }

    /// Cancel the wizard
    pub fn cancel(&self) {
        if let Some(tx) = self
            .cancel_tx
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = tx.send(());
        }
        // Only cancel a still-running flow; never clobber a flow that already
        // settled as Done/Error (which would discard its result).
        Self::settle(&self.status, WizardStatus::Cancelled);
    }

    /// Check if the session is done
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.status() != WizardStatus::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::types::{WizardOption, WizardStep as Step};
    use crate::wizard::WizardPrompter;
    use serde_json::json;

    struct TestFlow {
        steps: Vec<WizardStep>,
    }

    #[async_trait]
    impl WizardFlow for TestFlow {
        async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
            for step in &self.steps {
                prompter.prompt(step.clone()).await?;
            }
            Ok(())
        }

        fn name(&self) -> &str {
            "test"
        }
    }

    #[tokio::test]
    async fn test_session_creation() {
        let flow = TestFlow {
            steps: vec![WizardStep::note("intro", "Welcome!")],
        };

        let session = WizardSession::new(Box::new(flow));
        assert_eq!(session.status(), WizardStatus::Running);
        assert!(!session.is_done());
    }

    #[tokio::test]
    async fn test_empty_flow() {
        let flow = TestFlow { steps: vec![] };

        let session = WizardSession::new(Box::new(flow));

        // Poll for terminal status instead of a fixed sleep — robust against
        // any scheduler jitter between spawning the flow task and it settling.
        wait_for_terminal(&session).await;

        // Next should return done
        let result = session.next().await;
        assert!(result.done);
        assert_eq!(result.status, WizardStatus::Done);
    }

    #[tokio::test]
    async fn cancel_after_done_preserves_terminal_status() {
        // A completed flow that is later cancelled (e.g. client-disconnect
        // cleanup arriving in the result-pending window) must keep its Done
        // status.
        let flow = TestFlow { steps: vec![] };
        let session = WizardSession::new(Box::new(flow));

        wait_for_terminal(&session).await;
        assert_eq!(session.status(), WizardStatus::Done);

        session.cancel();
        assert_eq!(
            session.status(),
            WizardStatus::Done,
            "cancel() must not clobber a completed flow"
        );
    }

    /// Yield until the spawned flow task has settled the session status. Bounded
    /// so a hung task fails the test loudly rather than running forever.
    async fn wait_for_terminal(session: &WizardSession) {
        for _ in 0..200 {
            if session.is_done() {
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        }
        panic!("flow task did not settle within 1s");
    }

    /// Notes are pushed without waiting for an answer, so a short flow can
    /// settle `Done` while its notes are still queued. `next()` must drain them
    /// before reporting the terminal status.
    #[tokio::test]
    async fn buffered_notes_survive_flow_completion() {
        struct NotesOnlyFlow;

        #[async_trait]
        impl WizardFlow for NotesOnlyFlow {
            async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
                prompter.note("first", None).await?;
                prompter.outro("Aleph is ready!").await?;
                Ok(())
            }
        }

        let session = WizardSession::new(Box::new(NotesOnlyFlow));
        wait_for_terminal(&session).await;
        assert_eq!(session.status(), WizardStatus::Done);

        let first = session.next().await;
        assert!(!first.done, "queued note must be delivered before Done");
        assert_eq!(
            first.step.expect("note step").message.as_deref(),
            Some("first")
        );

        let second = session.next().await;
        assert_eq!(
            second.step.expect("outro step").message.as_deref(),
            Some("Aleph is ready!")
        );

        let third = session.next().await;
        assert!(third.done);
        assert_eq!(third.status, WizardStatus::Done);
    }

    /// A client that answers every step it is handed must not stall on notes,
    /// which carry no pending answer.
    #[tokio::test]
    async fn answering_a_note_is_acknowledged_and_flow_continues() {
        struct NoteThenTextFlow;

        #[async_trait]
        impl WizardFlow for NoteThenTextFlow {
            async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
                prompter.note("read me", None).await?;
                prompter.text("Your name?", None, false).await?;
                Ok(())
            }
        }

        let session = WizardSession::new(Box::new(NoteThenTextFlow));

        let note = session.next().await.step.expect("note step");
        assert_eq!(note.step_type, StepType::Note);
        session
            .answer(&note.id, json!(null))
            .await
            .expect("answering a note must be acknowledged");

        let text = session.next().await.step.expect("text step");
        assert_eq!(text.step_type, StepType::Text);
    }

    /// The client is untrusted: values that were never offered (or that belong
    /// to a disabled option) must not reach the flow.
    #[tokio::test]
    async fn answer_rejects_values_outside_the_offered_options() {
        let step = Step::select(
            "pick",
            "Pick one",
            vec![
                WizardOption::new(json!("a"), "A"),
                WizardOption::new(json!("b"), "B").disabled(),
            ],
        );
        let session = WizardSession::new(Box::new(TestFlow { steps: vec![step] }));

        assert_eq!(session.next().await.step.expect("step").id, "pick");

        let unlisted = session
            .answer("pick", json!("c"))
            .await
            .expect_err("unlisted value must be rejected");
        assert!(
            matches!(&unlisted, WizardSessionError::InvalidAnswer(m) if m.contains("not one of")),
            "unexpected error: {unlisted:?}"
        );

        let disabled = session
            .answer("pick", json!("b"))
            .await
            .expect_err("disabled option must be rejected");
        assert!(matches!(disabled, WizardSessionError::InvalidAnswer(_)));

        session
            .answer("pick", json!("a"))
            .await
            .expect("offered option must be accepted");
    }

    /// Answering the same step twice is a client bug, but `StepNotFound` sent
    /// the client hunting for a missing step instead of a duplicate submit.
    #[tokio::test]
    async fn second_answer_reports_already_answered() {
        let session = WizardSession::new(Box::new(TestFlow {
            steps: vec![Step::text("name", "Your name?")],
        }));

        assert_eq!(session.next().await.step.expect("step").id, "name");
        session.answer("name", json!("ada")).await.expect("first");

        let err = session
            .answer("name", json!("grace"))
            .await
            .expect_err("second answer must fail");
        assert!(
            matches!(&err, WizardSessionError::InvalidAnswer(m) if m.contains("already been answered")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn validate_answer_enforces_step_shape() {
        let confirm = Step::confirm("c", "Sure?");
        assert!(validate_answer(&confirm, &json!(true)).is_ok());
        assert!(validate_answer(&confirm, &json!("true")).is_err());

        let text = Step::text("t", "Name?");
        assert!(validate_answer(&text, &json!("ada")).is_ok());
        assert!(validate_answer(&text, &json!(42)).is_err());

        // A note accepts anything, including an explicit null acknowledgement.
        assert!(validate_answer(&Step::note("n", "hi"), &json!(null)).is_ok());

        // Options-less selects cannot be domain-checked; they must not reject.
        let bare_select = Step::new("s", StepType::Select);
        assert!(validate_answer(&bare_select, &json!("anything")).is_ok());

        let mut multi = Step::new("m", StepType::MultiSelect);
        multi.options = Some(vec![
            WizardOption::new(json!("telegram"), "Telegram"),
            WizardOption::new(json!("slack"), "Slack").disabled(),
        ]);
        assert!(validate_answer(&multi, &json!([])).is_ok());
        assert!(validate_answer(&multi, &json!(["telegram"])).is_ok());
        assert!(validate_answer(&multi, &json!(["telegram", "slack"])).is_err());
        assert!(validate_answer(&multi, &json!("telegram")).is_err());
    }
}
