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
    /// Processing AI request
    Processing,
    /// Operation completed successfully
    Success,
    /// Operation failed with error
    Error,
}

/// Trait for receiving events from Aether core
///
/// Clients (Swift, Kotlin, etc.) implement this trait to receive
/// callbacks when hotkeys are detected, states change, or errors occur.
pub trait AetherEventHandler: Send + Sync {
    /// Called when the processing state changes
    fn on_state_changed(&self, state: ProcessingState);

    /// Called when a hotkey is detected with clipboard content
    fn on_hotkey_detected(&self, clipboard_content: String);

    /// Called when an error occurs
    fn on_error(&self, message: String);
}

/// Mock event handler for testing
///
/// Records all callback invocations for assertion in tests
#[cfg(test)]
pub struct MockEventHandler {
    pub state_changes: std::sync::Arc<std::sync::Mutex<Vec<ProcessingState>>>,
    pub hotkey_events: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub errors: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[cfg(test)]
impl MockEventHandler {
    pub fn new() -> Self {
        Self {
            state_changes: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            hotkey_events: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            errors: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn get_state_changes(&self) -> Vec<ProcessingState> {
        self.state_changes.lock().unwrap().clone()
    }

    pub fn get_hotkey_events(&self) -> Vec<String> {
        self.hotkey_events.lock().unwrap().clone()
    }

    pub fn get_errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl AetherEventHandler for MockEventHandler {
    fn on_state_changed(&self, state: ProcessingState) {
        self.state_changes.lock().unwrap().push(state);
    }

    fn on_hotkey_detected(&self, clipboard_content: String) {
        self.hotkey_events.lock().unwrap().push(clipboard_content);
    }

    fn on_error(&self, message: String) {
        self.errors.lock().unwrap().push(message);
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

    #[test]
    fn test_mock_handler_hotkey_events() {
        let handler = MockEventHandler::new();
        handler.on_hotkey_detected("test content".to_string());
        handler.on_hotkey_detected("more content".to_string());

        let events = handler.get_hotkey_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], "test content");
        assert_eq!(events[1], "more content");
    }

    #[test]
    fn test_mock_handler_errors() {
        let handler = MockEventHandler::new();
        handler.on_error("error 1".to_string());
        handler.on_error("error 2".to_string());

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
