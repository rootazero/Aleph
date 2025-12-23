//! Hotkey detection trait and implementations
//!
//! This module provides global hotkey listening using the rdev crate.
//! Phase 1 hardcodes Cmd+~ (Command+Grave) detection.
mod rdev_listener;

use crate::error::Result;
pub use rdev_listener::RdevListener;

/// Trait for hotkey listening operations
///
/// This trait allows for swappable hotkey listener implementations
/// and enables easy mocking in tests.
pub trait HotkeyListener: Send + Sync {
    /// Start listening for hotkey events
    ///
    /// Spawns a background thread to monitor keyboard events.
    /// Returns error if listener cannot start (e.g., missing permissions).
    fn start_listening(&self) -> Result<()>;

    /// Stop listening for hotkey events
    ///
    /// Terminates the background thread and releases resources.
    fn stop_listening(&self) -> Result<()>;

    /// Check if currently listening
    fn is_listening(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rdev_listener_creation() {
        let callback = || {};
        let _listener = RdevListener::new(callback);
        // Should not panic
    }

    #[test]
    fn test_rdev_listener_initial_state() {
        let callback = || {};
        let listener = RdevListener::new(callback);
        assert!(!listener.is_listening());
    }
}
