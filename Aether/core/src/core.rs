/// AetherCore - Main entry point for the Aether library
///
/// Orchestrates hotkey listening, clipboard management, and event callbacks.
use crate::clipboard::{ArboardManager, ClipboardManager};
use crate::error::{AetherError, Result};
use crate::event_handler::{AetherEventHandler, ErrorType, ProcessingState};
use crate::hotkey::{HotkeyListener, RdevListener};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

/// Context for last request (used for retry)
#[derive(Debug, Clone)]
struct RequestContext {
    clipboard_content: String,
    provider: String,
    retry_count: u32,
}

/// Main core struct for Aether
///
/// Manages lifecycle of all core components and coordinates
/// between hotkey detection, clipboard operations, and client callbacks.
pub struct AetherCore {
    event_handler: Arc<dyn AetherEventHandler>,
    hotkey_listener: Arc<dyn HotkeyListener>,
    clipboard_manager: Arc<dyn ClipboardManager>,
    #[allow(dead_code)]
    runtime: Arc<Runtime>,
    last_request: Arc<Mutex<Option<RequestContext>>>,
}

impl AetherCore {
    /// Create a new AetherCore instance with the provided event handler
    ///
    /// # Arguments
    /// * `event_handler` - Handler for receiving callbacks from Rust
    ///
    /// # Returns
    /// * `Result<Self>` - New AetherCore instance or error
    pub fn new(event_handler: Box<dyn AetherEventHandler>) -> Result<Self> {
        let event_handler: Arc<dyn AetherEventHandler> = Arc::from(event_handler);
        // Initialize tokio runtime for async operations
        let runtime = Runtime::new()
            .map_err(|e| AetherError::other(format!("Failed to create tokio runtime: {}", e)))?;

        // Clone event handler for the hotkey callback
        let handler_clone = Arc::clone(&event_handler);
        let clipboard_manager: Arc<dyn ClipboardManager> = Arc::new(ArboardManager::new());
        let clipboard_clone = Arc::clone(&clipboard_manager);

        // Create hotkey listener with callback
        let hotkey_listener: Arc<dyn HotkeyListener> = Arc::new(RdevListener::new(move || {
            // When hotkey is detected, read clipboard and invoke callback
            handler_clone.on_state_changed(ProcessingState::Listening);

            match clipboard_clone.read_text() {
                Ok(content) => {
                    handler_clone.on_hotkey_detected(content);
                }
                Err(e) => {
                    handler_clone.on_error(format!("Failed to read clipboard: {}", e));
                }
            }
        }));

        Ok(Self {
            event_handler,
            hotkey_listener,
            clipboard_manager,
            runtime: Arc::new(runtime),
            last_request: Arc::new(Mutex::new(None)),
        })
    }

    /// Start listening for hotkey events
    ///
    /// Spawns background thread to monitor keyboard events.
    pub fn start_listening(&self) -> Result<()> {
        self.event_handler
            .on_state_changed(ProcessingState::Listening);

        self.hotkey_listener.start_listening()?;
        Ok(())
    }

    /// Stop listening for hotkey events
    ///
    /// Terminates background thread and releases resources.
    pub fn stop_listening(&self) -> Result<()> {
        self.hotkey_listener.stop_listening()?;
        self.event_handler.on_state_changed(ProcessingState::Idle);
        Ok(())
    }

    /// Get current clipboard text content
    ///
    /// # Returns
    /// * `Result<String>` - Clipboard text or error
    pub fn get_clipboard_text(&self) -> Result<String> {
        self.clipboard_manager.read_text()
    }

    /// Check if currently listening for hotkeys
    pub fn is_listening(&self) -> bool {
        self.hotkey_listener.is_listening()
    }

    /// Test method: Simulate streaming AI response (for development/testing only)
    ///
    /// Sends chunks of text to the event handler with delays to simulate streaming.
    /// This is a placeholder for Phase 4 AI provider integration.
    #[cfg(debug_assertions)]
    pub fn test_streaming_response(&self) {
        use std::thread;
        use std::time::Duration;

        // Simulate a streaming response
        let chunks = vec![
            "Hello, ",
            "this is ",
            "a streaming ",
            "AI response. ",
            "Each chunk ",
            "appears with ",
            "a slight delay ",
            "to demonstrate ",
            "the streaming ",
            "text feature.",
        ];

        self.event_handler
            .on_state_changed(ProcessingState::Processing);

        for i in 0..chunks.len() {
            // Simulate network delay
            thread::sleep(Duration::from_millis(100));

            // Accumulate text and send full text so far
            let accumulated: String = chunks[..=i].concat();
            self.event_handler.on_response_chunk(accumulated);
        }

        // Simulate completion
        thread::sleep(Duration::from_millis(500));
        self.event_handler
            .on_state_changed(ProcessingState::Success);
    }

    /// Test method: Simulate typed error (for development/testing only)
    #[cfg(debug_assertions)]
    pub fn test_typed_error(&self, error_type: ErrorType, message: String) {
        self.event_handler
            .on_error_typed(error_type, message);
    }

    /// Retry the last failed request
    ///
    /// Implements exponential backoff: 2s, 4s, 8s
    /// Max 2 auto-retries, then manual retry only
    ///
    /// # Returns
    /// * `Result<()>` - Ok if retry initiated, Error if no request to retry or max retries exceeded
    pub fn retry_last_request(&self) -> Result<()> {
        use std::thread;
        use std::time::Duration;

        let mut last_request_lock = self.last_request.lock().unwrap();

        let request_ctx = last_request_lock
            .as_mut()
            .ok_or_else(|| AetherError::other("No request to retry".to_string()))?;

        // Check max retry limit
        const MAX_RETRIES: u32 = 2;
        if request_ctx.retry_count >= MAX_RETRIES {
            return Err(AetherError::other(format!(
                "Maximum retry limit ({}) reached",
                MAX_RETRIES
            )));
        }

        // Increment retry count
        request_ctx.retry_count += 1;

        // Calculate exponential backoff: 2^retry_count seconds
        let backoff_seconds = 2u64.pow(request_ctx.retry_count);

        // Clone data for async operation (will be used in Phase 4)
        let _clipboard_content = request_ctx.clipboard_content.clone();
        let _provider = request_ctx.provider.clone();
        let _retry_count = request_ctx.retry_count;

        drop(last_request_lock); // Release lock before sleep

        // Wait with exponential backoff
        thread::sleep(Duration::from_secs(backoff_seconds));

        // Notify state change
        self.event_handler
            .on_state_changed(ProcessingState::Processing);

        // TODO: When AI provider integration is implemented in Phase 4,
        // this should call the actual AI provider with the stored context.
        // For now, we'll simulate success after backoff.

        // Simulate processing
        thread::sleep(Duration::from_millis(500));

        // Simulate success (in real implementation, this would be actual API call result)
        self.event_handler
            .on_state_changed(ProcessingState::Success);

        Ok(())
    }

    /// Store request context for retry (called when initiating AI request)
    ///
    /// This should be called before making an AI API request to enable retry functionality.
    ///
    /// # Arguments
    /// * `clipboard_content` - The content being processed
    /// * `provider` - The AI provider being used
    pub fn store_request_context(&self, clipboard_content: String, provider: String) {
        let mut last_request = self.last_request.lock().unwrap();
        *last_request = Some(RequestContext {
            clipboard_content,
            provider,
            retry_count: 0,
        });
    }

    /// Clear stored request context
    pub fn clear_request_context(&self) {
        let mut last_request = self.last_request.lock().unwrap();
        *last_request = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_handler::MockEventHandler;

    #[test]
    fn test_core_creation() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();
        assert!(!core.is_listening());
    }

    #[test]
    fn test_start_stop_listening() {
        let core = AetherCore::new(Box::new(MockEventHandler::new())).unwrap();

        assert!(!core.is_listening());

        core.start_listening().unwrap();
        assert!(core.is_listening());

        core.stop_listening().unwrap();
        assert!(!core.is_listening());
    }

    #[test]
    fn test_clipboard_read() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Write test content to clipboard
        core.clipboard_manager.write_text("test content").unwrap();

        // Read it back via core
        let content = core.get_clipboard_text().unwrap();
        assert_eq!(content, "test content");
    }

    #[test]
    fn test_multiple_start_stop_cycles() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        for _ in 0..3 {
            core.start_listening().unwrap();
            assert!(core.is_listening());

            core.stop_listening().unwrap();
            assert!(!core.is_listening());
        }
    }

    #[test]
    fn test_request_context_storage() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Store request context
        core.store_request_context(
            "Test clipboard content".to_string(),
            "openai".to_string(),
        );

        // Verify context is stored by attempting retry
        let result = core.retry_last_request();
        assert!(result.is_ok());
    }

    #[test]
    fn test_retry_without_context() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Attempt retry without storing context first
        let result = core.retry_last_request();
        assert!(result.is_err());
    }

    #[test]
    fn test_retry_max_limit() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Store request context
        core.store_request_context(
            "Test content".to_string(),
            "openai".to_string(),
        );

        // First retry should succeed
        assert!(core.retry_last_request().is_ok());

        // Second retry should succeed
        assert!(core.retry_last_request().is_ok());

        // Third retry should fail (max limit reached)
        let result = core.retry_last_request();
        assert!(result.is_err());
    }

    #[test]
    fn test_clear_request_context() {
        let handler = Box::new(MockEventHandler::new());
        let core = AetherCore::new(handler).unwrap();

        // Store and then clear context
        core.store_request_context(
            "Test content".to_string(),
            "openai".to_string(),
        );
        core.clear_request_context();

        // Retry should fail after clearing
        let result = core.retry_last_request();
        assert!(result.is_err());
    }
}
