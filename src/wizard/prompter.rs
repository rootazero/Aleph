//! Wizard prompter implementations.
//!
//! Provides abstractions for collecting user input during wizard flows.

use crate::sync_primitives::{Arc, AtomicU64, Ordering, RwLock};
use std::collections::HashMap;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

use super::session::WizardSessionError;
use super::types::{StepType, WizardOption, WizardStep};

/// Progress handle for tracking long-running operations
pub trait ProgressHandle: Send + Sync {
    /// Update progress message
    fn update(&self, message: &str);
    /// Finish with success message
    fn finish(&self, message: &str);
    /// Finish with error message
    fn finish_error(&self, message: &str);
}

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

    /// Progress indicator
    fn progress(&self, label: &str) -> Box<dyn ProgressHandle>;
}

/// Pending answer state
pub(crate) struct PendingAnswer {
    pub sender: oneshot::Sender<Value>,
}

/// RPC-based prompter for Gateway sessions
pub struct RpcPrompter {
    step_tx: mpsc::Sender<WizardStep>,
    answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
    finish_data: Arc<RwLock<Option<Value>>>,
    step_counter: AtomicU64,
}

impl RpcPrompter {
    /// Create a new RPC prompter
    pub(crate) const fn new(
        step_tx: mpsc::Sender<WizardStep>,
        answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
        finish_data: Arc<RwLock<Option<Value>>>,
    ) -> Self {
        Self {
            step_tx,
            answers,
            finish_data,
            step_counter: AtomicU64::new(0),
        }
    }

    /// Generate next step ID
    fn next_id(&self) -> String {
        let id = self.step_counter.fetch_add(1, Ordering::Relaxed) + 1;
        format!("step-{id}")
    }

    /// Send a step and wait for answer
    pub async fn prompt(&self, step: WizardStep) -> Result<Value, WizardSessionError> {
        let (tx, rx) = oneshot::channel();
        let step_id = step.id.clone();

        // Register pending answer
        {
            let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
            answers.insert(step_id.clone(), PendingAnswer { sender: tx });
        }

        // Send step; on failure the pending sender would otherwise leak in
        // the answers map for the rest of the session, so remove it first.
        if self.step_tx.send(step.clone()).await.is_err() {
            let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
            answers.remove(&step_id);
            return Err(WizardSessionError::Internal("Channel closed".to_string()));
        }

        debug!(step_id = %step.id, "Waiting for answer");

        // Wait for answer.  If the sender is dropped without sending,
        // treat it as an internal error (the flow task may have panicked
        // or the channel was closed unexpectedly). The PendingAnswer entry
        // is removed by `Session::answer` on success and intentionally
        // left in place on cancellation/error so a duplicate answer()
        // surfaces `StepNotFound` rather than silently swallowing.
        rx.await.map_err(|_| {
            WizardSessionError::Internal(
                "Answer channel closed unexpectedly (flow may have panicked)".to_string(),
            )
        })
    }

    /// Mark the flow as complete with a payload that propagates back through
    /// the next `wizard.next` response in `WizardNextResult.data`.
    pub async fn finish(&self, data: Value) -> Result<(), WizardSessionError> {
        *self.finish_data.write().unwrap_or_else(|e| e.into_inner()) = Some(data);
        Ok(())
    }

    /// Send a step without waiting (for notes)
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

    fn progress(&self, label: &str) -> Box<dyn ProgressHandle> {
        Box::new(RpcProgressHandle {
            label: label.to_string(),
        })
    }
}

/// RPC progress handle
struct RpcProgressHandle {
    label: String,
}

impl ProgressHandle for RpcProgressHandle {
    fn update(&self, message: &str) {
        debug!(label = %self.label, message = %message, "Progress update");
    }

    fn finish(&self, message: &str) {
        debug!(label = %self.label, message = %message, "Progress finished");
    }

    fn finish_error(&self, message: &str) {
        debug!(label = %self.label, message = %message, "Progress error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_prompter_id_generation() {
        let (tx, _rx) = mpsc::channel(16);
        let answers = Arc::new(RwLock::new(HashMap::new()));
        let finish_data = Arc::new(RwLock::new(None));
        let prompter = RpcPrompter::new(tx, answers, finish_data);

        let id1 = prompter.next_id();
        let id2 = prompter.next_id();

        assert_eq!(id1, "step-1");
        assert_eq!(id2, "step-2");
    }

    #[tokio::test]
    async fn finish_stores_payload() {
        let (tx, _rx) = mpsc::channel(16);
        let answers = Arc::new(RwLock::new(HashMap::new()));
        let finish_data = Arc::new(RwLock::new(None));
        let prompter = RpcPrompter::new(tx, answers, finish_data.clone());

        prompter
            .finish(serde_json::json!({ "token": "secret" }))
            .await
            .unwrap();

        let stored = finish_data
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap();
        assert_eq!(stored["token"], "secret");
    }
}
