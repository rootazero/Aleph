//! Wizard prompter implementations.
//!
//! Provides abstractions for collecting user input during wizard flows.

use crate::sync_primitives::{Arc, AtomicU64, Ordering, RwLock};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use super::session::WizardSessionError;
use super::types::{StepType, WizardOption, WizardStep};

/// How long a flow waits for a client answer before abandoning the step.
///
/// Without this bound, a client that starts a wizard and then disappears (tab
/// closed, socket dropped, process killed) leaves the flow task parked on
/// `rx.await` forever, pinning its session entry in the gateway's session map.
/// Repeating that is an unbounded resource leak, so every prompt is bounded.
/// The window is deliberately generous: steps ask humans to paste API keys.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A wizard prompter that can collect user input
#[async_trait]
pub trait WizardPrompter: Send + Sync {
    /// Show intro message
    async fn intro(&self, title: &str) -> Result<(), WizardSessionError>;

    /// Show outro message
    async fn outro(&self, message: &str) -> Result<(), WizardSessionError>;

    /// Show a note
    async fn note(&self, message: &str, title: Option<&str>) -> Result<(), WizardSessionError>;

    /// Single selection
    async fn select<T: DeserializeOwned + Send>(
        &self,
        message: &str,
        options: Vec<WizardOption>,
    ) -> Result<T, WizardSessionError>;

    /// Multi selection
    async fn multi_select<T: DeserializeOwned + Send>(
        &self,
        message: &str,
        options: Vec<WizardOption>,
    ) -> Result<Vec<T>, WizardSessionError>;

    /// Text input
    async fn text(
        &self,
        message: &str,
        placeholder: Option<&str>,
        sensitive: bool,
    ) -> Result<String, WizardSessionError>;

    /// Confirmation
    async fn confirm(&self, message: &str, default: bool) -> Result<bool, WizardSessionError>;
}

/// Pending answer state
pub(crate) struct PendingAnswer {
    pub sender: oneshot::Sender<Value>,
}

/// RPC-based prompter for Gateway sessions
pub struct RpcPrompter {
    step_tx: mpsc::Sender<WizardStep>,
    answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
    step_counter: AtomicU64,
}

impl RpcPrompter {
    /// Create a new RPC prompter
    pub(crate) const fn new(
        step_tx: mpsc::Sender<WizardStep>,
        answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
    ) -> Self {
        Self {
            step_tx,
            answers,
            step_counter: AtomicU64::new(0),
        }
    }

    /// Generate next step ID
    fn next_id(&self) -> String {
        let id = self.step_counter.fetch_add(1, Ordering::Relaxed) + 1;
        format!("step-{id}")
    }

    /// Send a step and wait for answer
    ///
    /// Bounded by [`ANSWER_TIMEOUT`]; a silent client fails the step instead of
    /// parking the flow task forever.
    pub async fn prompt(&self, step: WizardStep) -> Result<Value, WizardSessionError> {
        let (tx, mut rx) = oneshot::channel();
        let step_id = step.id.clone();

        // Register pending answer. A collision means the flow issued two
        // prompts with the same step id while the first is still pending;
        // silently overwriting would drop the first sender and fail the older
        // prompt with a confusing "channel closed", so reject it outright.
        {
            let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
            match answers.entry(step_id.clone()) {
                Entry::Occupied(_) => {
                    warn!(step_id = %step_id, "Duplicate pending step id");
                    return Err(WizardSessionError::Internal(format!(
                        "Duplicate pending step id '{step_id}'"
                    )));
                }
                Entry::Vacant(slot) => {
                    slot.insert(PendingAnswer { sender: tx });
                }
            }
        }

        // Send step; on failure the pending sender would otherwise leak in
        // the answers map for the rest of the session, so remove it first.
        if self.step_tx.send(step).await.is_err() {
            let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
            answers.remove(&step_id);
            return Err(WizardSessionError::Internal("Channel closed".to_string()));
        }

        debug!(step_id = %step_id, "Waiting for answer");

        // Wait for answer.  If the sender is dropped without sending,
        // treat it as an internal error (the flow task may have panicked
        // or the channel was closed unexpectedly). On every code path the
        // entry leaves the answers map — by `Session::answer`, by the
        // channel-closed branch above, or by the timeout branch below — so the
        // map never accumulates orphaned senders.
        match tokio::time::timeout(ANSWER_TIMEOUT, &mut rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(WizardSessionError::Internal(
                "Answer channel closed unexpectedly (flow may have panicked)".to_string(),
            )),
            Err(_) => {
                // Reclaim our sender so it cannot outlive the prompt. If it is
                // already gone, an `answer()` raced the deadline — accept the
                // value if it landed rather than discarding a real answer.
                let reclaimed = {
                    let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
                    answers.remove(&step_id)
                };
                let raced_answer = if reclaimed.is_none() {
                    rx.try_recv().ok()
                } else {
                    None
                };
                if let Some(value) = raced_answer {
                    return Ok(value);
                }
                warn!(step_id = %step_id, "Timed out waiting for wizard answer");
                Err(WizardSessionError::AnswerTimeout {
                    step_id,
                    timeout_secs: ANSWER_TIMEOUT.as_secs(),
                })
            }
        }
    }

    /// Send a step without waiting (for notes)
    ///
    /// No `PendingAnswer` is registered: notes carry no input. A client that
    /// uniformly calls `wizard.answer` for every step it is handed is
    /// tolerated by [`super::session::WizardSession::answer`], which
    /// acknowledges answers to note steps instead of failing them.
    async fn prompt_no_wait(&self, step: WizardStep) -> Result<(), WizardSessionError> {
        self.step_tx
            .send(step)
            .await
            .map_err(|_| WizardSessionError::Internal("Channel closed".to_string()))
    }
}

#[async_trait]
impl WizardPrompter for RpcPrompter {
    async fn intro(&self, title: &str) -> Result<(), WizardSessionError> {
        let step = WizardStep::note(self.next_id(), title).with_title("Welcome");
        self.prompt_no_wait(step).await
    }

    async fn outro(&self, message: &str) -> Result<(), WizardSessionError> {
        let step = WizardStep::note(self.next_id(), message).with_title("Complete");
        self.prompt_no_wait(step).await
    }

    async fn note(&self, message: &str, title: Option<&str>) -> Result<(), WizardSessionError> {
        let mut step = WizardStep::note(self.next_id(), message);
        if let Some(t) = title {
            step = step.with_title(t);
        }
        self.prompt_no_wait(step).await
    }

    async fn select<T: DeserializeOwned + Send>(
        &self,
        message: &str,
        options: Vec<WizardOption>,
    ) -> Result<T, WizardSessionError> {
        let step = WizardStep::select(self.next_id(), message, options);
        let value = self.prompt(step).await?;
        serde_json::from_value(value).map_err(|e| WizardSessionError::InvalidAnswer(e.to_string()))
    }

    async fn multi_select<T: DeserializeOwned + Send>(
        &self,
        message: &str,
        options: Vec<WizardOption>,
    ) -> Result<Vec<T>, WizardSessionError> {
        let mut step = WizardStep::new(self.next_id(), StepType::MultiSelect);
        step.message = Some(message.to_string());
        step.options = Some(options);

        let value = self.prompt(step).await?;
        serde_json::from_value(value).map_err(|e| WizardSessionError::InvalidAnswer(e.to_string()))
    }

    async fn text(
        &self,
        message: &str,
        placeholder: Option<&str>,
        sensitive: bool,
    ) -> Result<String, WizardSessionError> {
        let mut step = WizardStep::text(self.next_id(), message);
        if let Some(p) = placeholder {
            step = step.with_placeholder(p);
        }
        step = step.with_sensitive(sensitive);

        let value = self.prompt(step).await?;
        value
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| WizardSessionError::InvalidAnswer("Expected string".to_string()))
    }

    async fn confirm(&self, message: &str, default: bool) -> Result<bool, WizardSessionError> {
        let step = WizardStep::confirm(self.next_id(), message).with_initial(default);

        let value = self.prompt(step).await?;
        value
            .as_bool()
            .ok_or_else(|| WizardSessionError::InvalidAnswer("Expected boolean".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_prompter_id_generation() {
        let (tx, _rx) = mpsc::channel(16);
        let answers = Arc::new(RwLock::new(HashMap::new()));
        let prompter = RpcPrompter::new(tx, answers);

        let id1 = prompter.next_id();
        let id2 = prompter.next_id();

        assert_eq!(id1, "step-1");
        assert_eq!(id2, "step-2");
    }

    /// A client that never answers must not park the flow task forever, and the
    /// abandoned sender must not linger in the answers map.
    #[tokio::test(start_paused = true)]
    async fn prompt_times_out_and_reclaims_pending_sender() {
        let (tx, mut rx) = mpsc::channel(16);
        let answers = Arc::new(RwLock::new(HashMap::new()));
        let prompter = RpcPrompter::new(tx, answers.clone());

        let step = WizardStep::text("step-1", "Enter your API key");
        let (prompt_result, delivered) = tokio::join!(prompter.prompt(step), async {
            // Drain the step so the send succeeds, then deliberately never answer.
            rx.recv().await
        });

        assert!(delivered.is_some(), "step should have been delivered");
        assert!(
            matches!(
                &prompt_result,
                Err(WizardSessionError::AnswerTimeout { step_id, .. }) if step_id == "step-1"
            ),
            "expected AnswerTimeout, got {prompt_result:?}"
        );
        assert!(
            answers.read().unwrap_or_else(|e| e.into_inner()).is_empty(),
            "timed-out prompt must not leak its sender"
        );
    }

    /// Two overlapping prompts sharing a step id must be rejected rather than
    /// silently evicting the first prompt's sender.
    #[tokio::test]
    async fn duplicate_pending_step_id_is_rejected() {
        let (tx, _rx) = mpsc::channel(16);
        let answers: Arc<RwLock<HashMap<String, PendingAnswer>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let prompter = RpcPrompter::new(tx, answers.clone());

        // Pre-register a pending answer for "dup" to simulate an in-flight prompt.
        let (pending_tx, _pending_rx) = oneshot::channel();
        answers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert("dup".to_string(), PendingAnswer { sender: pending_tx });

        let err = prompter
            .prompt(WizardStep::text("dup", "again?"))
            .await
            .expect_err("duplicate step id must fail");
        assert!(
            matches!(&err, WizardSessionError::Internal(m) if m.contains("Duplicate")),
            "unexpected error: {err:?}"
        );
    }

    /// A closed step channel must remove the just-registered pending sender.
    #[tokio::test]
    async fn closed_step_channel_removes_pending_sender() {
        let (tx, rx) = mpsc::channel(16);
        drop(rx);
        let answers = Arc::new(RwLock::new(HashMap::new()));
        let prompter = RpcPrompter::new(tx, answers.clone());

        let err = prompter
            .prompt(WizardStep::text("step-1", "hi"))
            .await
            .expect_err("send on a closed channel must fail");
        assert!(matches!(err, WizardSessionError::Internal(_)));
        assert!(answers.read().unwrap_or_else(|e| e.into_inner()).is_empty());
    }
}
