//! Event handler trait for callbacks from Rust to client code.
//!
//! This trait defines the callback interface that Swift/Kotlin clients
//! must implement to receive events from the Aether core. UniFFI will
//! generate a protocol/interface for each target language.

use crate::clarification::{ClarificationRequest, ClarificationResult};

/// Processing states for the Aether system
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingState {
    /// System is idle, not processing
    Idle,
    /// Listening for hotkey events
    Listening,
    /// Retrieving memories from database (Phase 7 - Task 7.5)
    RetrievingMemory,
    /// AI provider is processing the request (Phase 7 - Task 7.5)
    ProcessingWithAI,
    /// Processing AI request (kept for backward compatibility)
    Processing,
    /// Typewriter animation in progress (Phase 7.2)
    Typewriting,
    /// Operation completed successfully
    Success,
    /// Operation failed with error
    Error,
}

/// Error types for typed error handling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorType {
    /// Network connectivity error
    Network,
    /// Permission denied error
    Permission,
    /// API quota exceeded
    Quota,
    /// Request timeout
    Timeout,
    /// Unknown error type
    Unknown,
}

/// Trait for receiving events from Aether core
///
/// Clients (Swift, Kotlin, etc.) implement this trait to receive
/// callbacks when hotkeys are detected, states change, or errors occur.
pub trait AetherEventHandler: Send + Sync {
    /// Called when the processing state changes
    fn on_state_changed(&self, state: ProcessingState);

    // REMOVED: on_hotkey_detected() - hotkey handling now in Swift layer

    /// Called when an error occurs
    fn on_error(&self, message: String, suggestion: Option<String>);

    /// Called when AI response chunk arrives (for streaming display)
    fn on_response_chunk(&self, text: String);

    /// Called when typed error occurs with error type
    fn on_error_typed(&self, error_type: ErrorType, message: String);

    /// Called when progress update is available
    fn on_progress(&self, percent: f32);

    /// Called when AI processing starts with provider information (Phase 7 - Task 7.4)
    fn on_ai_processing_started(&self, provider_name: String, provider_color: String);

    /// Called when AI response is received with response preview (Phase 7 - Task 7.4)
    fn on_ai_response_received(&self, response_preview: String);

    /// Called when primary provider fails and fallback is attempted (Phase 10 - Task 10.2)
    fn on_provider_fallback(&self, from_provider: String, to_provider: String);

    /// Called when config file changes externally (Phase 6 - Task 6.1)
    fn on_config_changed(&self);

    /// Called during typewriter animation with progress percentage (0.0-1.0) (Phase 7.2)
    fn on_typewriter_progress(&self, percent: f32);

    /// Called when typewriter animation is cancelled (Escape key or new hotkey) (Phase 7.2)
    fn on_typewriter_cancelled(&self);

    /// Called when clarification is needed from the user (Phantom Flow interaction)
    ///
    /// This is a BLOCKING callback - the Rust core will wait for the Swift UI
    /// to display the clarification request and return the user's response.
    /// The callback returns when the user selects an option, enters text,
    /// cancels, or the request times out.
    fn on_clarification_needed(&self, request: ClarificationRequest) -> ClarificationResult;

    // ========================================================================
    // Multi-Turn Conversation Callbacks (add-multi-turn-conversation)
    // ========================================================================

    /// Called when a new conversation session starts.
    ///
    /// # Arguments
    /// * `session_id` - Unique identifier for the conversation session
    fn on_conversation_started(&self, session_id: String);

    /// Called when a conversation turn is completed.
    ///
    /// # Arguments
    /// * `turn` - The completed turn with user input and AI response
    fn on_conversation_turn_completed(&self, turn: crate::conversation::ConversationTurn);

    /// Called when the AI response is ready and continuation input can be shown.
    ///
    /// The UI should display the Halo input box for user to continue the conversation.
    fn on_conversation_continuation_ready(&self);

    /// Called when a conversation session ends.
    ///
    /// # Arguments
    /// * `session_id` - The ID of the ended session
    /// * `total_turns` - Total number of turns in the conversation
    fn on_conversation_ended(&self, session_id: String, total_turns: u32);
}

/// Mock event handler for testing
///
/// Records all callback invocations for assertion in tests
#[cfg(test)]
pub struct MockEventHandler {
    pub state_changes: std::sync::Arc<std::sync::Mutex<Vec<ProcessingState>>>,
    // REMOVED: hotkey_events - hotkey handling now in Swift layer
    pub errors: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub response_chunks: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub typed_errors: std::sync::Arc<std::sync::Mutex<Vec<(ErrorType, String)>>>,
    pub progress_updates: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
    pub ai_processing_started: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    pub ai_responses: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub provider_fallbacks: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    pub config_changes: std::sync::Arc<std::sync::Mutex<u32>>, // Count of config change events
    pub typewriter_progress: std::sync::Arc<std::sync::Mutex<Vec<f32>>>, // Typewriter progress updates
    pub typewriter_cancelled: std::sync::Arc<std::sync::Mutex<u32>>,     // Count of cancellations
    pub clarification_requests: std::sync::Arc<std::sync::Mutex<Vec<ClarificationRequest>>>, // Phantom Flow requests
    pub clarification_response: std::sync::Arc<std::sync::Mutex<Option<ClarificationResult>>>, // Mock response to return
    // Multi-turn conversation tracking
    pub conversation_started: std::sync::Arc<std::sync::Mutex<Vec<String>>>, // Session IDs
    pub conversation_turns: std::sync::Arc<std::sync::Mutex<Vec<crate::conversation::ConversationTurn>>>,
    pub conversation_continuation_ready_count: std::sync::Arc<std::sync::Mutex<u32>>,
    pub conversation_ended: std::sync::Arc<std::sync::Mutex<Vec<(String, u32)>>>, // (session_id, total_turns)
}

#[cfg(test)]
impl MockEventHandler {
    pub fn new() -> Self {
        Self {
            state_changes: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            // REMOVED: hotkey_events - hotkey handling now in Swift layer
            errors: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            response_chunks: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            typed_errors: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            progress_updates: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            ai_processing_started: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            ai_responses: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            provider_fallbacks: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            config_changes: std::sync::Arc::new(std::sync::Mutex::new(0)),
            typewriter_progress: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            typewriter_cancelled: std::sync::Arc::new(std::sync::Mutex::new(0)),
            clarification_requests: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            clarification_response: std::sync::Arc::new(std::sync::Mutex::new(None)),
            conversation_started: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            conversation_turns: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            conversation_continuation_ready_count: std::sync::Arc::new(std::sync::Mutex::new(0)),
            conversation_ended: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn get_state_changes(&self) -> Vec<ProcessingState> {
        self.state_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    // REMOVED: get_hotkey_events() - hotkey handling now in Swift layer

    pub fn get_errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_response_chunks(&self) -> Vec<String> {
        self.response_chunks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_typed_errors(&self) -> Vec<(ErrorType, String)> {
        self.typed_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_progress_updates(&self) -> Vec<f32> {
        self.progress_updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_ai_processing_started(&self) -> Vec<(String, String)> {
        self.ai_processing_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_ai_responses(&self) -> Vec<String> {
        self.ai_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_provider_fallbacks(&self) -> Vec<(String, String)> {
        self.provider_fallbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_config_change_count(&self) -> u32 {
        *self
            .config_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_typewriter_progress(&self) -> Vec<f32> {
        self.typewriter_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_typewriter_cancelled_count(&self) -> u32 {
        *self
            .typewriter_cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_clarification_requests(&self) -> Vec<ClarificationRequest> {
        self.clarification_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Set the mock response to return when on_clarification_needed is called
    pub fn set_clarification_response(&self, response: ClarificationResult) {
        *self
            .clarification_response
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(response);
    }

    pub fn get_conversation_started(&self) -> Vec<String> {
        self.conversation_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_conversation_turns(&self) -> Vec<crate::conversation::ConversationTurn> {
        self.conversation_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get_conversation_continuation_ready_count(&self) -> u32 {
        *self
            .conversation_continuation_ready_count
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn get_conversation_ended(&self) -> Vec<(String, u32)> {
        self.conversation_ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
impl AetherEventHandler for MockEventHandler {
    fn on_state_changed(&self, state: ProcessingState) {
        self.state_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(state);
    }

    // REMOVED: on_hotkey_detected() - hotkey handling now in Swift layer

    fn on_error(&self, message: String, _suggestion: Option<String>) {
        self.errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(message);
    }

    fn on_response_chunk(&self, text: String) {
        self.response_chunks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(text);
    }

    fn on_error_typed(&self, error_type: ErrorType, message: String) {
        self.typed_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((error_type, message));
    }

    fn on_progress(&self, percent: f32) {
        self.progress_updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(percent);
    }

    fn on_ai_processing_started(&self, provider_name: String, provider_color: String) {
        self.ai_processing_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((provider_name, provider_color));
    }

    fn on_ai_response_received(&self, response_preview: String) {
        self.ai_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(response_preview);
    }

    fn on_provider_fallback(&self, from_provider: String, to_provider: String) {
        self.provider_fallbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((from_provider, to_provider));
    }

    fn on_config_changed(&self) {
        let mut count = self
            .config_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    fn on_typewriter_progress(&self, percent: f32) {
        self.typewriter_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(percent);
    }

    fn on_typewriter_cancelled(&self) {
        let mut count = self
            .typewriter_cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    fn on_clarification_needed(&self, request: ClarificationRequest) -> ClarificationResult {
        // Record the request
        self.clarification_requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(request);

        // Return the pre-configured mock response, or default to cancelled
        self.clarification_response
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(ClarificationResult::cancelled)
    }

    fn on_conversation_started(&self, session_id: String) {
        self.conversation_started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_id);
    }

    fn on_conversation_turn_completed(&self, turn: crate::conversation::ConversationTurn) {
        self.conversation_turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(turn);
    }

    fn on_conversation_continuation_ready(&self) {
        let mut count = self
            .conversation_continuation_ready_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    fn on_conversation_ended(&self, session_id: String, total_turns: u32) {
        self.conversation_ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((session_id, total_turns));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_handler_state_changes() {
        let handler = MockEventHandler::new();
        handler.on_state_changed(ProcessingState::Idle);
        handler.on_state_changed(ProcessingState::Listening);

        let states = handler.get_state_changes();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0], ProcessingState::Idle);
        assert_eq!(states[1], ProcessingState::Listening);
    }

    // REMOVED: test_mock_handler_hotkey_events - hotkey handling now in Swift layer

    #[test]
    fn test_mock_handler_errors() {
        let handler = MockEventHandler::new();
        handler.on_error("error 1".to_string(), None);
        handler.on_error("error 2".to_string(), Some("Try again".to_string()));

        let errors = handler.get_errors();
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0], "error 1");
        assert_eq!(errors[1], "error 2");
    }

    #[test]
    fn test_processing_state_equality() {
        assert_eq!(ProcessingState::Idle, ProcessingState::Idle);
        assert_ne!(ProcessingState::Idle, ProcessingState::Listening);
    }

    #[test]
    fn test_mock_handler_clarification() {
        use crate::clarification::{ClarificationOption, ClarificationRequest, ClarificationResult};

        let handler = MockEventHandler::new();

        // Set up a mock response
        let mock_response = ClarificationResult::selected(1, "casual".to_string());
        handler.set_clarification_response(mock_response);

        // Create a clarification request
        let request = ClarificationRequest::select(
            "style-choice",
            "What style would you like?",
            vec![
                ClarificationOption::new("professional", "Professional"),
                ClarificationOption::new("casual", "Casual"),
            ],
        );

        // Call the handler
        let result = handler.on_clarification_needed(request);

        // Verify the request was recorded
        let requests = handler.get_clarification_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].id, "style-choice");

        // Verify the response
        assert!(result.is_success());
        assert_eq!(result.selected_index, Some(1));
        assert_eq!(result.value, Some("casual".to_string()));
    }

    #[test]
    fn test_mock_handler_clarification_default_cancelled() {
        use crate::clarification::{ClarificationRequest, ClarificationResultType};

        let handler = MockEventHandler::new();
        // Don't set a response - should default to cancelled

        let request = ClarificationRequest::text("test", "Enter name:", None);
        let result = handler.on_clarification_needed(request);

        assert_eq!(result.result_type, ClarificationResultType::Cancelled);
        assert!(!result.is_success());
    }
}
