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
        if self.step_tx.send(step).await.is_err() {
            let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
            answers.remove(&step_id);
            return Err(WizardSessionError::Internal("Channel closed".to_string()));
        }

        debug!(step_id = %step_id, "Waiting for answer");

        // Wait for answer.  If the sender is dropped without sending,
        // treat it as an internal error (the flow task may have panicked
        // or the channel was closed unexpectedly). On every code path the
        // entry leaves the answers map — either by `Session::answer` (the
        // normal success and `sender.send` failure paths both remove it) or
        // by the channel-closed branch above — so a duplicate `answer()` for
        // the same step surfaces `StepNotFound` instead of silently resending.
        rx.await.map_err(|_| {
            WizardSessionError::Internal(
                "Answer channel closed unexpectedly (flow may have panicked)".to_string(),
            )
        })
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
}
