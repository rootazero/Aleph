/// Clipboard management trait and implementations
///
/// This module provides a trait-based abstraction for clipboard operations,
/// with an implementation using the arboard crate.
mod arboard_manager;

use crate::error::Result;
pub use arboard_manager::ArboardManager;

/// Trait for clipboard management operations
///
/// This trait allows for swappable clipboard implementations
/// and enables easy mocking in tests.
pub trait ClipboardManager: Send + Sync {
    /// Read plain text from clipboard
    fn read_text(&self) -> Result<String>;

    /// Write plain text to clipboard
    fn write_text(&self, content: &str) -> Result<()>;

    /// Check if clipboard has image content (future use)
    fn has_image(&self) -> bool {
        false // Default implementation for Phase 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arboard_manager_write_read() {
        let manager = ArboardManager::new();

        // Write test content
        manager.write_text("test clipboard content").unwrap();

        // Read it back
        let content = manager.read_text().unwrap();
        assert_eq!(content, "test clipboard content");
    }

    #[test]
    fn test_arboard_manager_overwrite() {
        let manager = ArboardManager::new();

        manager.write_text("first").unwrap();
        manager.write_text("second").unwrap();

        let content = manager.read_text().unwrap();
        assert_eq!(content, "second");
    }

    #[test]
    fn test_arboard_manager_unicode() {
        let manager = ArboardManager::new();

        let unicode_text = "Hello 世界 🌍";
        manager.write_text(unicode_text).unwrap();

        let content = manager.read_text().unwrap();
        assert_eq!(content, unicode_text);
    }
}
