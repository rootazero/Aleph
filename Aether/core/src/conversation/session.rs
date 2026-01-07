//! Conversation session and turn data structures.

use crate::core::CapturedContext;
use serde::{Deserialize, Serialize};

/// A single turn in a conversation (user input + AI response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    /// Sequential turn number (0-indexed)
    pub turn_id: u32,
    /// User's input for this turn
    pub user_input: String,
    /// AI's response for this turn
    pub ai_response: String,
    /// Unix timestamp when this turn occurred
    pub timestamp: i64,
}

impl ConversationTurn {
    /// Create a new conversation turn.
    pub fn new(turn_id: u32, user_input: String, ai_response: String) -> Self {
        Self {
            turn_id,
            user_input,
            ai_response,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }
}

/// A multi-turn conversation session.
#[derive(Debug, Clone)]
pub struct ConversationSession {
    /// Unique session identifier (UUID)
    pub session_id: String,
    /// All turns in this conversation
    pub turns: Vec<ConversationTurn>,
    /// Unix timestamp when session started
    pub start_time: i64,
    /// Unix timestamp of last activity
    pub last_activity: i64,
    /// Whether session is still active
    pub active: bool,
    /// Context captured at session start (for returning focus)
    pub context: CapturedContext,
}

impl ConversationSession {
    /// Create a new conversation session.
    pub fn new(context: CapturedContext) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            turns: Vec::new(),
            start_time: now,
            last_activity: now,
            active: true,
            context,
        }
    }

    /// Get the session ID.
    pub fn id(&self) -> &str {
        &self.session_id
    }

    /// Get the number of turns in this session.
    pub fn turn_count(&self) -> u32 {
        self.turns.len() as u32
    }

    /// Add a turn to the session.
    pub fn add_turn(&mut self, user_input: String, ai_response: String) -> &ConversationTurn {
        let turn_id = self.turns.len() as u32;
        let turn = ConversationTurn::new(turn_id, user_input, ai_response);
        self.turns.push(turn);
        self.last_activity = chrono::Utc::now().timestamp();
        self.turns.last().unwrap()
    }

    /// Get the last turn in the session.
    pub fn last_turn(&self) -> Option<&ConversationTurn> {
        self.turns.last()
    }

    /// End the session.
    pub fn end(&mut self) {
        self.active = false;
        self.last_activity = chrono::Utc::now().timestamp();
    }

    /// Check if session has timed out.
    pub fn is_timed_out(&self, timeout_seconds: i64) -> bool {
        let now = chrono::Utc::now().timestamp();
        (now - self.last_activity) > timeout_seconds
    }

    /// Get the app bundle ID where this session started.
    pub fn origin_app(&self) -> &str {
        &self.context.app_bundle_id
    }

    /// Get the window title where this session started.
    pub fn origin_window(&self) -> Option<&str> {
        self.context.window_title.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_context() -> CapturedContext {
        CapturedContext {
            app_bundle_id: "com.apple.Notes".to_string(),
            window_title: Some("Test Note".to_string()),
            attachments: None,
        }
    }

    #[test]
    fn test_conversation_turn_creation() {
        let turn = ConversationTurn::new(0, "Hello".to_string(), "Hi there!".to_string());
        assert_eq!(turn.turn_id, 0);
        assert_eq!(turn.user_input, "Hello");
        assert_eq!(turn.ai_response, "Hi there!");
        assert!(turn.timestamp > 0);
    }

    #[test]
    fn test_conversation_session_creation() {
        let context = create_test_context();
        let session = ConversationSession::new(context);

        assert!(!session.session_id.is_empty());
        assert!(session.turns.is_empty());
        assert!(session.active);
        assert_eq!(session.turn_count(), 0);
        assert_eq!(session.origin_app(), "com.apple.Notes");
    }

    #[test]
    fn test_add_turn() {
        let context = create_test_context();
        let mut session = ConversationSession::new(context);

        session.add_turn("Question 1".to_string(), "Answer 1".to_string());
        assert_eq!(session.turn_count(), 1);
        assert_eq!(session.last_turn().unwrap().user_input, "Question 1");

        session.add_turn("Question 2".to_string(), "Answer 2".to_string());
        assert_eq!(session.turn_count(), 2);
        assert_eq!(session.last_turn().unwrap().turn_id, 1);
    }

    #[test]
    fn test_session_end() {
        let context = create_test_context();
        let mut session = ConversationSession::new(context);

        assert!(session.active);
        session.end();
        assert!(!session.active);
    }
}
