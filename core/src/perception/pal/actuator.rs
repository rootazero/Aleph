//! Cross-platform input actuation trait

use async_trait::async_trait;

use crate::AlephError;

/// Cross-platform trait for simulating user input
///
/// This trait provides a unified interface for simulating mouse and keyboard
/// input across different platforms. Implementations use platform-specific
/// APIs (CGEvent on macOS, SendInput on Windows, XTest on Linux).
#[async_trait]
pub trait InputActuator: Send + Sync {
    /// Click at absolute screen coordinates
    ///
    /// Simulates a mouse click at the specified screen position.
    /// Coordinates are in pixels from the top-left corner of the primary display.
    ///
    /// # Arguments
    /// * `x` - X coordinate in pixels
    /// * `y` - Y coordinate in pixels
    ///
    /// # Errors
    /// * `AlephError::not_supported` - Platform doesn't support input simulation
    /// * `AlephError::permission` - Missing input monitoring permissions
    async fn click(&self, x: i32, y: i32) -> Result<(), AlephError>;

    /// Type text at current input focus
    ///
    /// Simulates typing the given text string. Respects the current keyboard
    /// focus - text will be inserted wherever the cursor is currently positioned.
    ///
    /// # Arguments
    /// * `text` - Text to type
    ///
    /// # Errors
    /// * `AlephError::not_supported` - Platform doesn't support input simulation
    /// * `AlephError::permission` - Missing input monitoring permissions
    async fn type_text(&self, text: &str) -> Result<(), AlephError>;

    /// Press a key with optional modifiers
    ///
    /// Simulates pressing a key, optionally with modifier keys held down.
    ///
    /// # Arguments
    /// * `key` - Key to press
    /// * `modifiers` - Modifier keys to hold (Command, Control, Alt, Shift)
    ///
    /// # Errors
    /// * `AlephError::not_supported` - Platform doesn't support input simulation
    /// * `AlephError::permission` - Missing input monitoring permissions
    async fn press_key(&self, key: Key, modifiers: &[Modifier]) -> Result<(), AlephError>;

    /// Check if actuator is available
    ///
    /// Returns false if:
    /// - Running in headless environment
    /// - Missing required permissions
    /// - Platform not supported
    fn is_available(&self) -> bool;
}

/// Keyboard key
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Return,
    Tab,
    Escape,
    Space,
    Backspace,
    Delete,
    LeftArrow,
    RightArrow,
    UpArrow,
    DownArrow,
    Home,
    End,
    PageUp,
    PageDown,
    // Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    // Character keys (use type_text for strings)
    Char(char),
}

/// Modifier key
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Command, // Cmd on macOS, Win on Windows
    Control,
    Alt,
    Shift,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_actuator_trait_object_safety() {
        // Verify trait is object-safe
        let _: Option<Box<dyn InputActuator>> = None;
    }
}
