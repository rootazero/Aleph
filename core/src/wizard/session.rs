//! Wizard session state machine.
//!
//! Manages the lifecycle of a wizard session, coordinating between
//! the flow implementation and the client.

use std::collections::HashMap;
use crate::sync_primitives::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error};

use super::prompter::{PendingAnswer, RpcPrompter};
use super::types::{WizardNextResult, WizardStatus, WizardStep};

/// Wizard session errors
#[derive(Debug, Error)]
pub enum WizardSessionError {
    #[error("Session cancelled")]
    Cancelled,

    #[error("Session already done")]
    AlreadyDone,

    #[error("Step not found: {0}")]
    StepNotFound(String),

    #[error("Invalid answer: {0}")]
    InvalidAnswer(String),

    #[error("Flow error: {0}")]
    FlowError(String),

    #[error("Internal error: {0}")]
    Internal(String),
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
    step_tx: mpsc::Sender<WizardStep>,
    step_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<WizardStep>>>,
    answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
    error: Arc<RwLock<Option<String>>>,
    cancel_tx: Option<oneshot::Sender<()>>,
}

impl WizardSession {
    /// Create a new wizard session and start the flow
    pub fn new(flow: Box<dyn WizardFlow>) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let (step_tx, step_rx) = mpsc::channel(16);
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let session = Self {
            id: id.clone(),
            status: Arc::new(RwLock::new(WizardStatus::Running)),
            current_step: Arc::new(RwLock::new(None)),
            step_tx,
            step_rx: Arc::new(tokio::sync::Mutex::new(step_rx)),
            answers: Arc::new(RwLock::new(HashMap::new())),
            error: Arc::new(RwLock::new(None)),
            cancel_tx: Some(cancel_tx),
        };

        // Create prompter for the flow
        let prompter = RpcPrompter::new(
            session.step_tx.clone(),
            session.answers.clone(),
        );

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
                            *status.write().unwrap_or_else(|e| e.into_inner()) = WizardStatus::Done;
                        }
                        Err(WizardSessionError::Cancelled) => {
                            debug!(id = %id, "Wizard flow cancelled");
                            *status.write().unwrap_or_else(|e| e.into_inner()) = WizardStatus::Cancelled;
                        }
                        Err(e) => {
                            error!(id = %id, error = %e, "Wizard flow error");
                            *status.write().unwrap_or_else(|e| e.into_inner()) = WizardStatus::Error;
                            *error.write().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
                        }
                    }
                }
                _ = cancel_rx => {
                    debug!(id = %id, "Wizard flow cancelled via signal");
                    *status.write().unwrap_or_else(|e| e.into_inner()) = WizardStatus::Cancelled;
                }
            }
        });

        session
    }

    /// Get the session ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the current status
    pub fn status(&self) -> WizardStatus {
        *self.status.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Get the next step (blocks until a step is available or done)
    pub async fn next(&self) -> WizardNextResult {
        // Check if already done
        let status = self.status();
        if status != WizardStatus::Running {
            return match status {
                WizardStatus::Done => WizardNextResult::done(),
                WizardStatus::Cancelled => WizardNextResult::cancelled(),
                WizardStatus::Error => {
                    let error = self.error.read().unwrap_or_else(|e| e.into_inner()).clone();
                    WizardNextResult::error(error.unwrap_or_else(|| "Unknown error".to_string()))
                }
                _ => WizardNextResult::done(),
            };
        }

        // Wait for next step from flow
        let mut rx = self.step_rx.lock().await;
        match rx.recv().await {
            Some(step) => {
                // Store current step
                *self.current_step.write().unwrap_or_else(|e| e.into_inner()) = Some(step.clone());
                WizardNextResult::step(step)
            }
            None => {
                // Channel closed, check final status
                let status = self.status();
                match status {
                    WizardStatus::Done => WizardNextResult::done(),
                    WizardStatus::Cancelled => WizardNextResult::cancelled(),
                    WizardStatus::Error => {
                        let error = self.error.read().unwrap_or_else(|e| e.into_inner()).clone();
                        WizardNextResult::error(error.unwrap_or_else(|| "Unknown error".to_string()))
                    }
                    _ => WizardNextResult::done(),
                }
            }
        }
    }

    /// Answer the current step
    pub async fn answer(&self, step_id: &str, value: Value) -> Result<(), WizardSessionError> {
        // Verify step ID matches current
        {
            let current = self.current_step.read().unwrap_or_else(|e| e.into_inner());
            if let Some(ref step) = *current {
                if step.id != step_id {
                    return Err(WizardSessionError::InvalidAnswer(format!(
                        "Expected answer for step '{}', got '{}'",
                        step.id, step_id
                    )));
                }
            } else {
                return Err(WizardSessionError::StepNotFound(step_id.to_string()));
            }
        }

        // Find and resolve the pending answer
        let sender = {
            let mut answers = self.answers.write().unwrap_or_else(|e| e.into_inner());
            answers.remove(step_id).map(|p| p.sender)
        };

        if let Some(sender) = sender {
            sender.send(value).map_err(|_| {
                WizardSessionError::Internal("Failed to send answer".to_string())
            })?;
            Ok(())
        } else {
            Err(WizardSessionError::StepNotFound(step_id.to_string()))
        }
    }

    /// Cancel the wizard
    pub fn cancel(&mut self) {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(());
        }
        *self.status.write().unwrap_or_else(|e| e.into_inner()) = WizardStatus::Cancelled;
    }

    /// Check if the session is done
    pub fn is_done(&self) -> bool {
        self.status() != WizardStatus::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        // Give flow time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Next should return done
        let result = session.next().await;
        assert!(result.done);
        assert_eq!(result.status, WizardStatus::Done);
    }
}
