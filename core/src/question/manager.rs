// Aleph/core/src/question/manager.rs
//! Question manager for handling structured user interaction.

use super::error::QuestionError;
use crate::event::question::{Answer, QuestionReply, QuestionRequest};
use crate::event::{AlephEvent, EventBus};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, RwLock};
use tracing::{info, warn};

/// Configuration for the question manager
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct QuestionManagerConfig {
    /// Default timeout for question requests (0 = no timeout)
    pub timeout_ms: u64,
}


/// A pending question request waiting for user response
pub struct PendingQuestion {
    /// The original request
    pub request: QuestionRequest,
    /// Channel to send the result
    response_tx: oneshot::Sender<Result<Vec<Answer>, QuestionError>>,
}

/// Question manager
///
/// Handles structured question/answer interaction with users.
pub struct QuestionManager {
    /// Pending question requests
    pending: RwLock<HashMap<String, PendingQuestion>>,
    /// Event bus for publishing events
    event_bus: Arc<EventBus>,
    /// Configuration
    config: QuestionManagerConfig,
}

impl QuestionManager {
    /// Create a new question manager
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self::with_config(event_bus, QuestionManagerConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(event_bus: Arc<EventBus>, config: QuestionManagerConfig) -> Self {
        Self {
            pending: RwLock::new(HashMap::new()),
            event_bus,
            config,
        }
    }

    /// Ask the user a question
    ///
    /// Returns the user's answers, or an error if rejected/timed out.
    pub async fn ask(&self, request: QuestionRequest) -> Result<Vec<Answer>, QuestionError> {
        let (tx, rx) = oneshot::channel();
        let request_id = request.id.clone();

        // Store pending request
        {
            let mut pending = self.pending.write().await;
            pending.insert(
                request_id.clone(),
                PendingQuestion {
                    request: request.clone(),
                    response_tx: tx,
                },
            );
        }

        info!(
            request_id = %request_id,
            question_count = request.questions.len(),
            "Asking user question(s)"
        );

        // Publish event
        self.event_bus
            .publish(AlephEvent::QuestionAsked(request))
            .await;

        // Wait for response with optional timeout
        let result = if self.config.timeout_ms > 0 {
            tokio::time::timeout(Duration::from_millis(self.config.timeout_ms), rx).await
        } else {
            Ok(rx.await)
        };

        match result {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                // Channel dropped - treat as rejection
                warn!(request_id = %request_id, "Question request channel dropped");
                Err(QuestionError::Rejected)
            }
            Err(_) => {
                // Timeout
                self.cleanup_pending(&request_id).await;
                Err(QuestionError::timeout(request_id, self.config.timeout_ms))
            }
        }
    }

    /// Handle user reply to a question request
    pub async fn reply(&self, request_id: &str, reply: QuestionReply) -> Result<(), QuestionError> {
        let pending = {
            let mut pending = self.pending.write().await;
            pending.remove(request_id)
        };

        let Some(pending) = pending else {
            warn!(request_id = %request_id, "Reply for unknown question request");
            return Ok(());
        };

        let session_id = pending.request.session_id.clone();

        info!(
            request_id = %request_id,
            answer_count = reply.answers.len(),
            "User replied to question"
        );

        // Publish reply event
        self.event_bus
            .publish(AlephEvent::QuestionReplied {
                session_id,
                request_id: request_id.to_string(),
                answers: reply.answers.clone(),
            })
            .await;

        let _ = pending.response_tx.send(Ok(reply.answers));
        Ok(())
    }

    /// Handle user rejection of a question
    pub async fn reject(&self, request_id: &str) -> Result<(), QuestionError> {
        let pending = {
            let mut pending = self.pending.write().await;
            pending.remove(request_id)
        };

        let Some(pending) = pending else {
            warn!(request_id = %request_id, "Reject for unknown question request");
            return Ok(());
        };

        let session_id = pending.request.session_id.clone();

        info!(request_id = %request_id, "User rejected question");

        // Publish reject event
        self.event_bus
            .publish(AlephEvent::QuestionRejected {
                session_id,
                request_id: request_id.to_string(),
            })
            .await;

        let _ = pending.response_tx.send(Err(QuestionError::Rejected));
        Ok(())
    }

    /// Clean up a pending request (e.g., on timeout)
    async fn cleanup_pending(&self, request_id: &str) {
        let mut pending = self.pending.write().await;
        pending.remove(request_id);
    }

    /// Get the list of pending question requests
    pub async fn list_pending(&self) -> Vec<QuestionRequest> {
        self.pending
            .read()
            .await
            .values()
            .map(|p| p.request.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::question::{QuestionInfo, QuestionOption};
    use crate::event::EventBus;

    fn create_test_manager() -> QuestionManager {
        let event_bus = Arc::new(EventBus::new());
        QuestionManager::new(event_bus)
    }

    fn create_test_question() -> QuestionInfo {
        QuestionInfo::new(
            "Which option do you prefer?",
            "Preference",
            vec![
                QuestionOption::new("Option A", "First option"),
                QuestionOption::new("Option B", "Second option"),
            ],
        )
    }

    #[tokio::test]
    async fn test_question_request_and_reply() {
        let manager = create_test_manager();
        let question = create_test_question();
        let request = QuestionRequest::single("q-1", "session-1", question);
        let request_id = request.id.clone();

        // Spawn a task to answer after a short delay
        let manager_clone = Arc::new(manager);
        let manager_for_reply = manager_clone.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let reply = QuestionReply::simple("Option A");
            manager_for_reply.reply(&request_id, reply).await.unwrap();
        });

        // Ask and wait for answer
        let result = manager_clone.ask(request).await;
        assert!(result.is_ok());

        let answers = result.unwrap();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0], vec!["Option A"]);
    }

    #[tokio::test]
    async fn test_question_rejection() {
        let manager = create_test_manager();
        let question = create_test_question();
        let request = QuestionRequest::single("q-1", "session-1", question);
        let request_id = request.id.clone();

        // Spawn a task to reject after a short delay
        let manager_clone = Arc::new(manager);
        let manager_for_reject = manager_clone.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            manager_for_reject.reject(&request_id).await.unwrap();
        });

        // Ask and expect rejection
        let result = manager_clone.ask(request).await;
        assert!(matches!(result, Err(QuestionError::Rejected)));
    }

    #[tokio::test]
    async fn test_list_pending() {
        let manager = create_test_manager();

        // Initially empty
        let pending = manager.list_pending().await;
        assert!(pending.is_empty());
    }
}
