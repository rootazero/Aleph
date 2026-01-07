//! Conversation session manager.
//!
//! Handles session lifecycle, context building, and history management.

use super::session::{ConversationSession, ConversationTurn};
use crate::core::CapturedContext;
use tracing::{debug, info, warn};

/// Default maximum number of turns per session.
const DEFAULT_MAX_TURNS: u32 = 10;
/// Default maximum characters in conversation history.
const DEFAULT_MAX_HISTORY_CHARS: usize = 4000;
/// Default session timeout in seconds (5 minutes).
const DEFAULT_TIMEOUT_SECONDS: i64 = 300;

/// Configuration for conversation management.
#[derive(Debug, Clone)]
pub struct ConversationConfig {
    /// Maximum turns per session
    pub max_turns: u32,
    /// Maximum characters in history context
    pub max_history_chars: usize,
    /// Session timeout in seconds
    pub timeout_seconds: i64,
    /// Whether to store conversations to memory
    pub store_to_memory: bool,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_TURNS,
            max_history_chars: DEFAULT_MAX_HISTORY_CHARS,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            store_to_memory: true,
        }
    }
}

/// Manager for multi-turn conversation sessions.
#[derive(Debug)]
pub struct ConversationManager {
    /// Currently active session (if any)
    active_session: Option<ConversationSession>,
    /// Configuration
    config: ConversationConfig,
}

impl ConversationManager {
    /// Create a new conversation manager with default config.
    pub fn new() -> Self {
        Self {
            active_session: None,
            config: ConversationConfig::default(),
        }
    }

    /// Create a new conversation manager with custom config.
    pub fn with_config(config: ConversationConfig) -> Self {
        Self {
            active_session: None,
            config,
        }
    }

    /// Start a new conversation session.
    ///
    /// If there's an existing session, it will be ended first.
    ///
    /// # Arguments
    /// * `context` - The captured context (app, window) at session start
    ///
    /// # Returns
    /// The session ID of the new session.
    pub fn start_session(&mut self, context: CapturedContext) -> String {
        // End any existing session
        if self.active_session.is_some() {
            warn!("Starting new session while previous session was active");
            self.end_session();
        }

        let session = ConversationSession::new(context);
        let session_id = session.session_id.clone();

        info!(
            session_id = %session_id,
            app = %session.origin_app(),
            "Started new conversation session"
        );

        self.active_session = Some(session);
        session_id
    }

    /// Add a turn to the active session.
    ///
    /// # Arguments
    /// * `user_input` - The user's input
    /// * `ai_response` - The AI's response
    ///
    /// # Returns
    /// The added turn, or None if no active session.
    pub fn add_turn(&mut self, user_input: String, ai_response: String) -> Option<ConversationTurn> {
        let session = self.active_session.as_mut()?;

        // Check if we've exceeded max turns
        if session.turn_count() >= self.config.max_turns {
            warn!(
                session_id = %session.id(),
                turns = session.turn_count(),
                max = self.config.max_turns,
                "Max turns reached, ending session"
            );
            self.end_session();
            return None;
        }

        let turn = session.add_turn(user_input, ai_response).clone();

        debug!(
            session_id = %session.id(),
            turn_id = turn.turn_id,
            "Added turn to conversation"
        );

        Some(turn)
    }

    /// Build conversation history for AI context.
    ///
    /// Formats previous turns for inclusion in the AI prompt.
    /// Respects max_history_chars limit.
    pub fn build_context_prompt(&self) -> String {
        let session = match &self.active_session {
            Some(s) => s,
            None => return String::new(),
        };

        if session.turns.is_empty() {
            return String::new();
        }

        let mut history = String::from("Previous conversation:\n");

        // Build history from oldest to newest, but may need to truncate older turns
        let mut turns_to_include: Vec<&ConversationTurn> = Vec::new();
        let mut total_chars = history.len();

        // Start from most recent and work backwards
        for turn in session.turns.iter().rev() {
            let turn_text = format!(
                "User: {}\nAssistant: {}\n\n",
                turn.user_input, turn.ai_response
            );
            let turn_len = turn_text.len();

            if total_chars + turn_len > self.config.max_history_chars {
                // Would exceed limit, stop adding
                break;
            }

            turns_to_include.push(turn);
            total_chars += turn_len;
        }

        // Reverse to get chronological order
        turns_to_include.reverse();

        // Build final history string
        for turn in turns_to_include {
            history.push_str(&format!(
                "User: {}\nAssistant: {}\n\n",
                turn.user_input, turn.ai_response
            ));
        }

        history
    }

    /// End the active session.
    ///
    /// # Returns
    /// The ended session, or None if no active session.
    pub fn end_session(&mut self) -> Option<ConversationSession> {
        let mut session = self.active_session.take()?;
        session.end();

        info!(
            session_id = %session.id(),
            turns = session.turn_count(),
            "Ended conversation session"
        );

        Some(session)
    }

    /// Check if there's an active session.
    pub fn has_active_session(&self) -> bool {
        self.active_session.is_some()
    }

    /// Get a reference to the active session.
    pub fn active_session(&self) -> Option<&ConversationSession> {
        self.active_session.as_ref()
    }

    /// Get the current turn count.
    pub fn turn_count(&self) -> u32 {
        self.active_session
            .as_ref()
            .map(|s| s.turn_count())
            .unwrap_or(0)
    }

    /// Get the origin app bundle ID of the active session.
    pub fn origin_app(&self) -> Option<&str> {
        self.active_session.as_ref().map(|s| s.origin_app())
    }

    /// Check and handle session timeout.
    ///
    /// # Returns
    /// True if session was timed out and ended.
    pub fn check_timeout(&mut self) -> bool {
        let should_timeout = self
            .active_session
            .as_ref()
            .map(|s| s.is_timed_out(self.config.timeout_seconds))
            .unwrap_or(false);

        if should_timeout {
            warn!("Session timed out, ending");
            self.end_session();
            true
        } else {
            false
        }
    }

    /// Update configuration.
    pub fn set_config(&mut self, config: ConversationConfig) {
        self.config = config;
    }
}

impl Default for ConversationManager {
    fn default() -> Self {
        Self::new()
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
    fn test_manager_creation() {
        let manager = ConversationManager::new();
        assert!(!manager.has_active_session());
        assert_eq!(manager.turn_count(), 0);
    }

    #[test]
    fn test_start_session() {
        let mut manager = ConversationManager::new();
        let context = create_test_context();

        let session_id = manager.start_session(context);
        assert!(!session_id.is_empty());
        assert!(manager.has_active_session());
        assert_eq!(manager.origin_app(), Some("com.apple.Notes"));
    }

    #[test]
    fn test_add_turn() {
        let mut manager = ConversationManager::new();
        manager.start_session(create_test_context());

        let turn = manager.add_turn("Hello".to_string(), "Hi!".to_string());
        assert!(turn.is_some());
        assert_eq!(manager.turn_count(), 1);

        let turn = turn.unwrap();
        assert_eq!(turn.turn_id, 0);
        assert_eq!(turn.user_input, "Hello");
    }

    #[test]
    fn test_build_context_prompt() {
        let mut manager = ConversationManager::new();
        manager.start_session(create_test_context());

        // Empty session
        assert!(manager.build_context_prompt().is_empty());

        // Add turns
        manager.add_turn("Question 1".to_string(), "Answer 1".to_string());
        manager.add_turn("Question 2".to_string(), "Answer 2".to_string());

        let context = manager.build_context_prompt();
        assert!(context.contains("Previous conversation:"));
        assert!(context.contains("User: Question 1"));
        assert!(context.contains("Assistant: Answer 1"));
        assert!(context.contains("User: Question 2"));
        assert!(context.contains("Assistant: Answer 2"));
    }

    #[test]
    fn test_end_session() {
        let mut manager = ConversationManager::new();
        manager.start_session(create_test_context());
        manager.add_turn("Hello".to_string(), "Hi!".to_string());

        let session = manager.end_session();
        assert!(session.is_some());
        assert!(!manager.has_active_session());

        let session = session.unwrap();
        assert!(!session.active);
        assert_eq!(session.turn_count(), 1);
    }

    #[test]
    fn test_max_turns_limit() {
        let config = ConversationConfig {
            max_turns: 2,
            ..Default::default()
        };
        let mut manager = ConversationManager::with_config(config);
        manager.start_session(create_test_context());

        // Add up to max
        manager.add_turn("Q1".to_string(), "A1".to_string());
        manager.add_turn("Q2".to_string(), "A2".to_string());

        // Next turn should fail and end session
        let turn = manager.add_turn("Q3".to_string(), "A3".to_string());
        assert!(turn.is_none());
        assert!(!manager.has_active_session());
    }

    #[test]
    fn test_history_truncation() {
        let config = ConversationConfig {
            max_history_chars: 100, // Very small limit
            ..Default::default()
        };
        let mut manager = ConversationManager::with_config(config);
        manager.start_session(create_test_context());

        // Add many turns
        for i in 0..5 {
            manager.add_turn(
                format!("This is question number {}", i),
                format!("This is a somewhat long answer number {}", i),
            );
        }

        let context = manager.build_context_prompt();
        // Should be truncated to fit within limit
        assert!(context.len() <= 150); // Some buffer for "Previous conversation:" prefix
    }
}
