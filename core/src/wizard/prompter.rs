//! Wizard prompter implementations.
//!
//! Provides abstractions for collecting user input during wizard flows.

use std::collections::HashMap;
use crate::sync_primitives::{Arc, RwLock};

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
    step_counter: Arc<RwLock<u64>>,
}

impl RpcPrompter {
    /// Create a new RPC prompter
    pub(crate) fn new(
        step_tx: mpsc::Sender<WizardStep>,
        answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
    ) -> Self {
        Self {
            step_tx,
            answers,
            step_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Generate next step ID
    fn next_id(&self) -> String {
        let mut counter = self.step_counter.write().unwrap_or_else(|e| e.into_inner());
        *counter += 1;
        format!("step-{}", *counter)
    }

    /// Send a step and wait for answer
    pub async fn prompt(&self, step: WizardStep) -> Result<Value, WizardSessionError> {
        let (tx, rx) = oneshot::channel();

        // Register pending answer
        {
            let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
            answers.insert(step.id.clone(), PendingAnswer { sender: tx });
        }

        // Send step
        self.step_tx
            .send(step.clone())
            .await
            .map_err(|_| WizardSessionError::Internal("Channel closed".to_string()))?;

        debug!(step_id = %step.id, "Waiting for answer");

        // Wait for answer
        rx.await
            .map_err(|_| WizardSessionError::Cancelled)
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
        let step = WizardStep::note(self.next_id(), title)
            .with_title("Welcome");
        self.prompt_no_wait(step).await
    }

    async fn outro(&self, message: &str) -> Result<(), WizardSessionError> {
        let step = WizardStep::note(self.next_id(), message)
            .with_title("Complete");
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
        serde_json::from_value(value)
            .map_err(|e| WizardSessionError::InvalidAnswer(e.to_string()))
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
        serde_json::from_value(value)
            .map_err(|e| WizardSessionError::InvalidAnswer(e.to_string()))
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
        let step = WizardStep::confirm(self.next_id(), message)
            .with_initial(default);

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

/// CLI-based prompter using dialoguer
pub struct CliPrompter;

impl CliPrompter {
    /// Create a new CLI prompter
    pub fn new() -> Self {
        Self
    }
}

impl Default for CliPrompter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WizardPrompter for CliPrompter {
    async fn intro(&self, title: &str) -> Result<(), WizardSessionError> {
        println!("\n╭─────────────────────────────────────╮");
        println!("│  {}  │", title);
        println!("╰─────────────────────────────────────╯\n");
        Ok(())
    }

    async fn outro(&self, message: &str) -> Result<(), WizardSessionError> {
        println!("\n✓ {}\n", message);
        Ok(())
    }

    async fn note(&self, message: &str, title: Option<&str>) -> Result<(), WizardSessionError> {
        if let Some(t) = title {
            println!("\n📋 {}", t);
        }
        println!("{}\n", message);
        Ok(())
    }

    async fn select<T: DeserializeOwned + Send>(
        &self,
        message: &str,
        options: Vec<WizardOption>,
    ) -> Result<T, WizardSessionError> {
        println!("\n{}", message);
        for (i, opt) in options.iter().enumerate() {
            let disabled = if opt.disabled { " (disabled)" } else { "" };
            let hint = opt.hint.as_deref().map(|h| format!(" - {}", h)).unwrap_or_default();
            println!("  {}. {}{}{}", i + 1, opt.label, hint, disabled);
        }

        // In a real implementation, we'd use dialoguer here
        // For now, return the first option
        if let Some(opt) = options.first() {
            serde_json::from_value(opt.value.clone())
                .map_err(|e| WizardSessionError::InvalidAnswer(e.to_string()))
        } else {
            Err(WizardSessionError::InvalidAnswer("No options available".to_string()))
        }
    }

    async fn multi_select<T: DeserializeOwned + Send>(
        &self,
        message: &str,
        options: Vec<WizardOption>,
    ) -> Result<Vec<T>, WizardSessionError> {
        println!("\n{}", message);
        for (i, opt) in options.iter().enumerate() {
            println!("  [{}] {}", i + 1, opt.label);
        }

        // Return empty for now
        Ok(vec![])
    }

    async fn text(
        &self,
        message: &str,
        placeholder: Option<&str>,
        sensitive: bool,
    ) -> Result<String, WizardSessionError> {
        let placeholder_str = placeholder.map(|p| format!(" ({})", p)).unwrap_or_default();
        let sensitive_str = if sensitive { " [hidden]" } else { "" };
        println!("\n{}{}{}: ", message, placeholder_str, sensitive_str);

        // In a real implementation, we'd read from stdin
        Ok(String::new())
    }

    async fn confirm(&self, message: &str, default: bool) -> Result<bool, WizardSessionError> {
        let default_str = if default { "Y/n" } else { "y/N" };
        println!("\n{} [{}]: ", message, default_str);

        // Return default for now
        Ok(default)
    }

    fn progress(&self, label: &str) -> Box<dyn ProgressHandle> {
        println!("⏳ {}...", label);
        Box::new(CliProgressHandle {
            label: label.to_string(),
        })
    }
}

/// CLI progress handle
struct CliProgressHandle {
    label: String,
}

impl ProgressHandle for CliProgressHandle {
    fn update(&self, message: &str) {
        println!("  → {}", message);
    }

    fn finish(&self, message: &str) {
        println!("✓ {} - {}", self.label, message);
    }

    fn finish_error(&self, message: &str) {
        println!("✗ {} - {}", self.label, message);
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

    #[tokio::test]
    async fn test_cli_prompter_intro() {
        let prompter = CliPrompter::new();
        let result = prompter.intro("Test Wizard").await;
        assert!(result.is_ok());
    }
}
