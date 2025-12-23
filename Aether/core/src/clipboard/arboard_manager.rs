/// arboard-based clipboard manager implementation
use super::ClipboardManager;
use crate::error::{AetherError, Result};
use arboard::Clipboard;

/// Clipboard manager using the arboard crate
///
/// Provides cross-platform clipboard access using arboard.
/// Each operation creates a new Clipboard instance to avoid
/// lifetime issues with the internal clipboard connection.
pub struct ArboardManager;

impl ArboardManager {
    /// Create a new arboard clipboard manager
    pub fn new() -> Self {
        Self
    }
}

impl Default for ArboardManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardManager for ArboardManager {
    fn read_text(&self) -> Result<String> {
        let mut clipboard = Clipboard::new()
            .map_err(|e| AetherError::clipboard(format!("Failed to access clipboard: {}", e)))?;

        clipboard
            .get_text()
            .map_err(|e| AetherError::clipboard(format!("Failed to read clipboard text: {}", e)))
    }

    fn write_text(&self, content: &str) -> Result<()> {
        let mut clipboard = Clipboard::new()
            .map_err(|e| AetherError::clipboard(format!("Failed to access clipboard: {}", e)))?;

        clipboard
            .set_text(content)
            .map_err(|e| AetherError::clipboard(format!("Failed to write clipboard text: {}", e)))
    }

    fn has_image(&self) -> bool {
        // Placeholder for Phase 4 - multimodal support
        // arboard supports image reading, but we'll implement this later
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_instance() {
        let _manager = ArboardManager::new();
        // Should not panic
    }

    #[test]
    fn test_default() {
        let _manager = ArboardManager;
        // Should not panic
    }

    #[test]
    fn test_read_write_cycle() {
        let manager = ArboardManager::new();

        let test_content = "arboard test content";
        manager.write_text(test_content).unwrap();

        let read_content = manager.read_text().unwrap();
        assert_eq!(read_content, test_content);
    }

    #[test]
    fn test_empty_string() {
        let manager = ArboardManager::new();

        manager.write_text("").unwrap();
        let content = manager.read_text().unwrap();
        assert_eq!(content, "");
    }

    #[test]
    fn test_multiline_text() {
        let manager = ArboardManager::new();

        let multiline = "Line 1\nLine 2\nLine 3";
        manager.write_text(multiline).unwrap();

        let content = manager.read_text().unwrap();
        assert_eq!(content, multiline);
    }

    #[test]
    fn test_has_image_returns_false() {
        let manager = ArboardManager::new();
        // Phase 1: always returns false
        assert!(!manager.has_image());
    }
}
