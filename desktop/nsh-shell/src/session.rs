// Session state management for NSH Desktop
//
// This module manages the current session state, message list, and event subscriptions.
// Phase 3 initial implementation uses a mock client to enable UI development independently
// of server RPC availability (per Plan Risk 2 mitigation).

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Role of a message participant
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A single message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub message_id: String,
    pub role: Role,
    pub content: String,
    pub timestamp: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

/// Tool call within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool_name: String,
    pub arguments: String,
    pub result: Option<String>,
}

/// Session manager state
pub struct SessionManager {
    session_id: Arc<RwLock<Option<String>>>,
    messages: Arc<RwLock<Vec<Message>>>,
    is_generating: Arc<RwLock<bool>>,
    event_tx: Option<mpsc::Sender<SessionEvent>>,
}

/// Events emitted by the session
#[derive(Debug, Clone)]
pub enum SessionEvent {
    MessageAdded(Message),
    MessageDelta { delta: String },
    MessageComplete,
    GenerationStarted,
    GenerationComplete,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new() -> Self {
        Self {
            session_id: Arc::new(RwLock::new(None)),
            messages: Arc::new(RwLock::new(Vec::new())),
            is_generating: Arc::new(RwLock::new(false)),
            event_tx: None,
        }
    }

    /// Create a new session on the server
    ///
    /// Mock implementation: generates a local session ID
    pub async fn create_session(&self) -> Result<String, String> {
        let session_id = format!("session-{}", uuid::Uuid::new_v4());
        *self.session_id.write().await = Some(session_id.clone());
        Ok(session_id)
    }

    /// Get the current session ID
    #[allow(dead_code)]
    pub async fn session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Load message history for the current session
    ///
    /// Mock implementation: returns empty list (no persistence yet)
    #[allow(dead_code)]
    pub async fn load_messages(&self) -> Result<(), String> {
        // In production: call message.list RPC
        // For Phase 3 mock: no-op (messages only exist in memory)
        Ok(())
    }

    /// Get all messages in the current session
    #[allow(dead_code)]
    pub async fn messages(&self) -> Vec<Message> {
        self.messages.read().await.clone()
    }

    /// Send a message to the current session
    ///
    /// Mock implementation: simulates a message send with fake assistant response
    pub async fn send_message(&self, content: String) -> Result<(), String> {
        let _session_id = self
            .session_id
            .read()
            .await
            .clone()
            .ok_or("No active session")?;

        // Add user message
        let user_msg = Message {
            message_id: format!("msg-user-{}", uuid::Uuid::new_v4()),
            role: Role::User,
            content: content.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            tool_calls: vec![],
        };

        self.messages.write().await.push(user_msg.clone());

        if let Some(tx) = &self.event_tx {
            let _ = tx.send(SessionEvent::MessageAdded(user_msg)).await;
        }

        // Mock: simulate assistant response after a delay
        let messages = self.messages.clone();
        let event_tx = self.event_tx.clone();
        let is_generating = self.is_generating.clone();

        tokio::spawn(async move {
            *is_generating.write().await = true;

            if let Some(tx) = &event_tx {
                let _ = tx.send(SessionEvent::GenerationStarted).await;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // Simulate streaming response
            let response_text = format!("This is a mock response to: {}", content);
            let assistant_msg_id = format!("msg-assistant-{}", uuid::Uuid::new_v4());

            for word in response_text.split_whitespace() {
                if let Some(tx) = &event_tx {
                    let delta = format!("{} ", word);
                    let _ = tx
                        .send(SessionEvent::MessageDelta { delta })
                        .await;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }

            let assistant_msg = Message {
                message_id: assistant_msg_id,
                role: Role::Assistant,
                content: response_text,
                timestamp: chrono::Utc::now().to_rfc3339(),
                tool_calls: vec![],
            };

            messages.write().await.push(assistant_msg.clone());

            if let Some(tx) = &event_tx {
                let _ = tx.send(SessionEvent::MessageComplete).await;
                let _ = tx.send(SessionEvent::MessageAdded(assistant_msg)).await;
                let _ = tx.send(SessionEvent::GenerationComplete).await;
            }

            *is_generating.write().await = false;
        });

        Ok(())
    }

    /// Check if generation is in progress
    #[allow(dead_code)]
    pub async fn is_generating(&self) -> bool {
        *self.is_generating.read().await
    }

    /// Subscribe to session events
    ///
    /// Returns a receiver for session events (message updates, streaming deltas, etc.)
    pub fn subscribe_events(&mut self) -> mpsc::Receiver<SessionEvent> {
        let (tx, rx) = mpsc::channel(64);
        self.event_tx = Some(tx);
        rx
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

// Dependencies needed for this module
// uuid: already in Cargo.toml (used by other modules)
// chrono: already in Cargo.toml (used by other modules)

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_session() {
        let manager = SessionManager::new();
        let session_id = manager.create_session().await.unwrap();
        assert!(session_id.starts_with("session-"));
        assert_eq!(manager.session_id().await, Some(session_id));
    }

    #[tokio::test]
    async fn test_send_message_requires_session() {
        let manager = SessionManager::new();
        let result = manager.send_message("test".to_string()).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "No active session");
    }

    #[tokio::test]
    async fn test_send_message_adds_user_message() {
        let mut manager = SessionManager::new();
        manager.create_session().await.unwrap();

        let mut rx = manager.subscribe_events();

        manager.send_message("Hello".to_string()).await.unwrap();

        // Wait for user message event
        let event = tokio::time::timeout(
            tokio::time::Duration::from_secs(1),
            rx.recv()
        ).await.unwrap().unwrap();

        match event {
            SessionEvent::MessageAdded(msg) => {
                assert_eq!(msg.role, Role::User);
                assert_eq!(msg.content, "Hello");
            }
            _ => panic!("Expected MessageAdded event"),
        }
    }

    #[tokio::test]
    async fn test_streaming_response() {
        let mut manager = SessionManager::new();
        manager.create_session().await.unwrap();

        let mut rx = manager.subscribe_events();

        manager.send_message("test".to_string()).await.unwrap();

        // Skip user message event
        let _ = rx.recv().await;

        // Wait for generation started
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, SessionEvent::GenerationStarted));
        assert!(manager.is_generating().await);

        // Collect deltas
        let mut deltas = Vec::new();
        loop {
            match rx.recv().await.unwrap() {
                SessionEvent::MessageDelta { delta } => {
                    deltas.push(delta);
                }
                SessionEvent::MessageComplete => break,
                _ => {}
            }
        }

        assert!(!deltas.is_empty());

        // Wait for final events
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, SessionEvent::MessageAdded(_)));

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, SessionEvent::GenerationComplete));
        assert!(!manager.is_generating().await);
    }

    #[tokio::test]
    async fn test_messages_persist() {
        let mut manager = SessionManager::new();
        manager.create_session().await.unwrap();
        manager.subscribe_events(); // Start event subscription

        manager.send_message("First".to_string()).await.unwrap();

        // Wait for mock response to complete
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let messages = manager.messages().await;
        assert_eq!(messages.len(), 2); // User + Assistant
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[0].content, "First");
        assert_eq!(messages[1].role, Role::Assistant);
    }
}
