/// AetherCore - Main entry point for the Aether library
///
/// Orchestrates hotkey listening, clipboard management, and event callbacks.
use crate::clipboard::{ArboardManager, ClipboardManager};
use crate::error::{AetherError, Result};
use crate::event_handler::{AetherEventHandler, ProcessingState};
use crate::hotkey::{HotkeyListener, RdevListener};
use std::sync::Arc;
use tokio::runtime::Runtime;

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
}
