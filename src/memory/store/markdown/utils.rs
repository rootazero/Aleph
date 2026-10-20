//! Utility functions for Markdown session storage.

use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::error::AlephError;

/// Format Unix timestamp as ISO 8601 string.
#[must_use]
pub fn format_timestamp(timestamp: i64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or_default();
    dt.format("%H:%M:%S").to_string()
}

/// Format Unix timestamp as full ISO 8601 string.
#[must_use]
pub fn format_timestamp_full(timestamp: i64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(timestamp, 0).unwrap_or_default();
    dt.to_rfc3339()
}

/// Compute SHA256 hash of data and return hex string.
#[must_use]
pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Get first 16 hex characters of SHA256 hash for file naming.
#[must_use]
pub fn hash_prefix(hash: &str) -> &str {
    &hash[..16.min(hash.len())]
}

/// Atomic write: write to temp file, then rename.
pub async fn atomic_write_markdown(path: &Path, content: &str) -> Result<(), AlephError> {
    let temp_path = path.with_extension("tmp");

    fs::write(&temp_path, content).await.map_err(|e| {
        AlephError::other(format!("Failed to write temp file {:?}: {}", temp_path, e))
    })?;

    fs::rename(&temp_path, path).await.map_err(|e| {
        AlephError::other(format!(
            "Failed to rename {:?} to {:?}: {}",
            temp_path, path, e
        ))
    })?;

    Ok(())
}

/// Append to Markdown file with file lock.
pub async fn append_markdown(path: &Path, content: &str) -> Result<(), AlephError> {
    // For now, use simple append. File locking will be added later.
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .await
        .map_err(|e| AlephError::other(format!("Failed to open {:?} for append: {}", path, e)))?;

    file.write_all(content.as_bytes())
        .await
        .map_err(|e| AlephError::other(format!("Failed to write to {:?}: {}", path, e)))?;

    file.write_all(b"\n")
        .await
        .map_err(|e| AlephError::other(format!("Failed to write newline to {:?}: {}", path, e)))?;

    file.flush()
        .await
        .map_err(|e| AlephError::other(format!("Failed to flush {:?}: {}", path, e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp() {
        let ts = 1729267535; // 2024-10-18 18:05:35 UTC
        let formatted = format_timestamp(ts);
        assert!(formatted.contains(":"));
    }

    #[test]
    fn test_compute_sha256() {
        let data = b"hello world";
        let hash = compute_sha256(data);
        assert_eq!(hash.len(), 64); // SHA256 = 32 bytes = 64 hex chars
        assert_eq!(&hash[..16], "b94d27b9934d3e08");
    }

    #[test]
    fn test_hash_prefix() {
        let hash = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let prefix = hash_prefix(hash);
        assert_eq!(prefix, "b94d27b9934d3e08");
        assert_eq!(prefix.len(), 16);
    }
}
