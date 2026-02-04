// core/src/memory/scratchpad/history.rs

//! Session History Log
//!
//! Archives completed scratchpad content for traceability.

use crate::error::AlephError;
use std::path::PathBuf;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

/// Manages the session history log file
pub struct SessionHistory {
    path: PathBuf,
}

/// A parsed history entry
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub timestamp: String,
    pub session_id: String,
    pub content: String,
}

impl SessionHistory {
    /// Create a new SessionHistory manager
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Append content to the history log
    pub async fn append(&self, content: &str, session_id: &str) -> Result<(), AlephError> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AlephError::other(format!("Failed to create history dir: {}", e)))?;
        }

        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S");
        let entry = format!(
            "\n--- Archived: {} (Session: {}) ---\n{}\n",
            timestamp, session_id, content
        );

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|e| AlephError::other(format!("Failed to open history file: {}", e)))?;

        file.write_all(entry.as_bytes())
            .await
            .map_err(|e| AlephError::other(format!("Failed to write history: {}", e)))?;

        Ok(())
    }

    /// Read recent history entries
    pub async fn read_recent(&self, max_entries: usize) -> Result<Vec<HistoryEntry>, AlephError> {
        let content = match fs::read_to_string(&self.path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(AlephError::other(format!("Failed to read history: {}", e))),
        };

        let entries: Vec<HistoryEntry> = content
            .split("\n--- Archived:")
            .skip(1) // Skip empty first split
            .filter_map(|entry| self.parse_entry(entry))
            .collect();

        // Return most recent entries
        let start = entries.len().saturating_sub(max_entries);
        Ok(entries[start..].to_vec())
    }

    /// Parse a single history entry
    fn parse_entry(&self, raw: &str) -> Option<HistoryEntry> {
        let lines: Vec<&str> = raw.lines().collect();
        if lines.is_empty() {
            return None;
        }

        // First line format: " TIMESTAMP (Session: ID) ---"
        let header = lines[0];
        let timestamp_end = header.find(" (Session:")?;
        let timestamp = header[1..timestamp_end].trim().to_string();

        let session_start = header.find("Session:")? + 8;
        let session_end = header.find(") ---")?;
        let session_id = header[session_start..session_end].trim().to_string();

        let content = lines[1..].join("\n").trim().to_string();

        Some(HistoryEntry {
            timestamp,
            session_id,
            content,
        })
    }

    /// Get total size of history file
    pub async fn size_bytes(&self) -> Result<u64, AlephError> {
        match fs::metadata(&self.path).await {
            Ok(metadata) => Ok(metadata.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(AlephError::other(format!("Failed to get history metadata: {}", e))),
        }
    }

    /// Rotate history if it exceeds max size
    pub async fn rotate_if_needed(&self, max_size_bytes: u64) -> Result<bool, AlephError> {
        let current_size = self.size_bytes().await?;

        if current_size <= max_size_bytes {
            return Ok(false);
        }

        // Rename current file to .old
        let old_path = self.path.with_extension("log.old");
        // Try to remove old file if it exists (EAFP pattern - handle NotFound gracefully)
        if let Err(e) = fs::remove_file(&old_path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(AlephError::other(format!("Failed to remove old history: {}", e)));
            }
        }

        fs::rename(&self.path, &old_path)
            .await
            .map_err(|e| AlephError::other(format!("Failed to rotate history: {}", e)))?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_append_and_read() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("history.log"));

        history.append("First entry", "sess-1").await.unwrap();
        history.append("Second entry", "sess-2").await.unwrap();

        let entries = history.read_recent(10).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id, "sess-1");
        assert_eq!(entries[1].session_id, "sess-2");
    }

    #[tokio::test]
    async fn test_read_empty() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("nonexistent.log"));

        let entries = history.read_recent(10).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_size_bytes() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("history.log"));

        assert_eq!(history.size_bytes().await.unwrap(), 0);

        history.append("Some content here", "sess").await.unwrap();

        assert!(history.size_bytes().await.unwrap() > 0);
    }

    #[tokio::test]
    async fn test_rotate() {
        let temp = tempdir().unwrap();
        let history = SessionHistory::new(temp.path().join("history.log"));

        // Write enough to exceed 100 bytes
        history.append("A".repeat(100).as_str(), "sess").await.unwrap();

        let rotated = history.rotate_if_needed(50).await.unwrap();
        assert!(rotated);

        // Old file should exist
        assert!(temp.path().join("history.log.old").exists());
    }
}
