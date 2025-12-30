//! Event handler trait for callbacks from Rust to client code.
//!
//! This trait defines the callback interface that Swift/Kotlin clients
//! must implement to receive events from the Aether core. UniFFI will
//! generate a protocol/interface for each target language.

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
        }
    }

    pub fn get_state_changes(&self) -> Vec<ProcessingState> {
        self.state_changes.lock().unwrap().clone()
    }

    // REMOVED: get_hotkey_events() - hotkey handling now in Swift layer

    pub fn get_errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }

    pub fn get_response_chunks(&self) -> Vec<String> {
        self.response_chunks.lock().unwrap().clone()
    }

    pub fn get_typed_errors(&self) -> Vec<(ErrorType, String)> {
        self.typed_errors.lock().unwrap().clone()
    }

    pub fn get_progress_updates(&self) -> Vec<f32> {
        self.progress_updates.lock().unwrap().clone()
    }

    pub fn get_ai_processing_started(&self) -> Vec<(String, String)> {
        self.ai_processing_started.lock().unwrap().clone()
    }

    pub fn get_ai_responses(&self) -> Vec<String> {
        self.ai_responses.lock().unwrap().clone()
    }

    pub fn get_provider_fallbacks(&self) -> Vec<(String, String)> {
        self.provider_fallbacks.lock().unwrap().clone()
    }

    pub fn get_config_change_count(&self) -> u32 {
        *self.config_changes.lock().unwrap()
    }

    pub fn get_typewriter_progress(&self) -> Vec<f32> {
        self.typewriter_progress.lock().unwrap().clone()
    }

    pub fn get_typewriter_cancelled_count(&self) -> u32 {
        *self.typewriter_cancelled.lock().unwrap()
    }
}

#[cfg(test)]
impl AetherEventHandler for MockEventHandler {
    fn on_state_changed(&self, state: ProcessingState) {
        self.state_changes.lock().unwrap().push(state);
    }

    // REMOVED: on_hotkey_detected() - hotkey handling now in Swift layer

    fn on_error(&self, message: String, _suggestion: Option<String>) {
        self.errors.lock().unwrap().push(message);
    }

    fn on_response_chunk(&self, text: String) {
        self.response_chunks.lock().unwrap().push(text);
    }

    fn on_error_typed(&self, error_type: ErrorType, message: String) {
        self.typed_errors
            .lock()
            .unwrap()
            .push((error_type, message));
    }

    fn on_progress(&self, percent: f32) {
        self.progress_updates.lock().unwrap().push(percent);
    }

    fn on_ai_processing_started(&self, provider_name: String, provider_color: String) {
        self.ai_processing_started
            .lock()
            .unwrap()
            .push((provider_name, provider_color));
    }

    fn on_ai_response_received(&self, response_preview: String) {
        self.ai_responses.lock().unwrap().push(response_preview);
    }

    fn on_provider_fallback(&self, from_provider: String, to_provider: String) {
        self.provider_fallbacks
            .lock()
            .unwrap()
            .push((from_provider, to_provider));
    }

    fn on_config_changed(&self) {
        let mut count = self.config_changes.lock().unwrap();
        *count += 1;
    }

    fn on_typewriter_progress(&self, percent: f32) {
        self.typewriter_progress.lock().unwrap().push(percent);
    }

    fn on_typewriter_cancelled(&self) {
        let mut count = self.typewriter_cancelled.lock().unwrap();
        *count += 1;
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
}
