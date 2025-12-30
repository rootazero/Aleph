//! Hotkey detection trait and implementations
//!
//! DEPRECATED: Hotkey listening is now handled by Swift layer (GlobalHotkeyMonitor.swift)
//! to avoid thread conflicts with macOS event system.
//!
//! This module is kept for backward compatibility but is no longer actively used.
mod event_tap_listener;
mod rdev_listener;

use crate::error::Result;

#[allow(dead_code)]
pub use rdev_listener::RdevListener;

/// Trait for hotkey listening operations (DEPRECATED)
///
/// This trait is no longer used as hotkey listening has been moved to Swift layer.
/// It's kept for backward compatibility only.
pub trait HotkeyListener: Send + Sync {
    /// Start listening for hotkey events
    fn start_listening(&self) -> Result<()>;

    /// Stop listening for hotkey events
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
