//! JSONL backup service for memory facts.
//!
//! Provides export/import functionality for memory facts using the JSONL
//! (JSON Lines) format, with rolling retention to limit disk usage.
//!
//! ## Features
//!
//! - **Export**: Dump all valid facts to a date-stamped `.jsonl` file
//! - **Restore**: Re-import facts from a backup file
//! - **Rolling retention**: Automatically prune old backups beyond `max_backups`

use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::store::MemoryBackend;
use crate::memory::store::MemoryStore;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Service for exporting and importing memory facts as JSONL backups.
///
/// Each backup file is named `memory-backup-YYYY-MM-DD.jsonl` and contains
/// one JSON-serialized `MemoryFact` per line. The service maintains a rolling
/// window of `max_backups` files, automatically deleting the oldest when the
/// limit is exceeded.
pub struct MemoryBackupService {
    /// Shared memory backend (LanceDB).
    database: MemoryBackend,
    /// Directory where backup files are stored.
    backup_dir: PathBuf,
    /// Maximum number of backup files to retain.
    max_backups: usize,
}

impl MemoryBackupService {
    /// Create a new backup service.
    ///
    /// # Arguments
    ///
    /// * `database` - Shared memory backend for reading/writing facts
    /// * `backup_dir` - Directory path where `.jsonl` files will be stored
    /// * `max_backups` - Maximum number of backup files to keep (oldest pruned first)
    pub fn new(database: MemoryBackend, backup_dir: PathBuf, max_backups: usize) -> Self {
        Self {
            database,
            backup_dir,
            max_backups,
        }
    }

    /// Export all valid memory facts to a JSONL backup file.
    ///
    /// Creates the backup directory if it does not exist, serializes each
    /// valid fact as a single JSON line, writes to a date-stamped file, and
    /// cleans up old backups exceeding the retention limit.
    ///
    /// Returns the path to the newly created backup file.
    pub async fn export_backup(&self) -> Result<PathBuf, AlephError> {
        // Ensure backup directory exists
        tokio::fs::create_dir_all(&self.backup_dir)
            .await
            .map_err(|e| AlephError::IoError(format!("Failed to create backup dir: {e}")))?;

        // Build date-stamped filename
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let filename = format!("memory-backup-{date}.jsonl");
        let path = self.backup_dir.join(&filename);

        // Fetch all valid facts (include_invalid = false)
        let facts = self.database.get_all_facts(false).await?;
        info!(
            count = facts.len(),
            path = %path.display(),
            "Exporting memory facts to JSONL backup"
        );

        // Serialize each fact as a JSON line
        let mut lines = Vec::with_capacity(facts.len());
        for fact in &facts {
            match serde_json::to_string(fact) {
                Ok(json) => lines.push(json),
                Err(e) => {
                    warn!(fact_id = %fact.id, error = %e, "Skipping fact: serialization failed");
                }
            }
        }

        let content = lines.join("\n");
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| AlephError::IoError(format!("Failed to write backup file: {e}")))?;

        info!(
            facts_written = lines.len(),
            path = %path.display(),
            "Backup export complete"
        );

        // Prune old backups
        self.cleanup_old_backups().await;

        Ok(path)
    }

    /// Remove the oldest backup files to stay within the `max_backups` limit.
    ///
    /// Files are identified by the `memory-backup-*.jsonl` naming pattern and
    /// sorted lexicographically (which, given the `YYYY-MM-DD` date format,
    /// produces chronological order).
    async fn cleanup_old_backups(&self) {
        let mut entries = match tokio::fs::read_dir(&self.backup_dir).await {
            Ok(entries) => entries,
            Err(e) => {
                warn!(error = %e, "Failed to read backup directory for cleanup");
                return;
            }
        };

        let mut backup_files: Vec<PathBuf> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("memory-backup-") && name_str.ends_with(".jsonl") {
                backup_files.push(entry.path());
            }
        }

        // Sort ascending (oldest first) — date-based names sort chronologically
        backup_files.sort();

        if backup_files.len() > self.max_backups {
            let to_remove = backup_files.len() - self.max_backups;
            for path in backup_files.iter().take(to_remove) {
                if let Err(e) = tokio::fs::remove_file(path).await {
                    warn!(path = %path.display(), error = %e, "Failed to remove old backup");
                } else {
                    info!(path = %path.display(), "Removed old backup file");
                }
            }
        }
    }

    /// Restore memory facts from a JSONL backup file.
    ///
    /// Reads the file line-by-line, parses each line as a `MemoryFact`, and
    /// inserts it into the database. Lines that fail to parse are logged and
    /// skipped.
    ///
    /// Returns the number of facts successfully restored.
    pub async fn restore_from_backup(&self, path: &Path) -> Result<usize, AlephError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| AlephError::IoError(format!("Failed to read backup file: {e}")))?;

        let mut restored = 0usize;
        let mut skipped = 0usize;

        for (line_num, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<MemoryFact>(line) {
                Ok(fact) => {
                    if let Err(e) = self.database.insert_fact(&fact).await {
                        warn!(
                            line = line_num + 1,
                            fact_id = %fact.id,
                            error = %e,
                            "Failed to insert fact during restore"
                        );
                        skipped += 1;
                    } else {
                        restored += 1;
                    }
                }
                Err(e) => {
                    warn!(
                        line = line_num + 1,
                        error = %e,
                        "Skipping malformed line in backup"
                    );
                    skipped += 1;
                }
            }
        }

        info!(
            restored,
            skipped,
            path = %path.display(),
            "Backup restore complete"
        );

        Ok(restored)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[test]
    fn test_backup_filename_format() {
        let date = "2026-02-24";
        let filename = format!("memory-backup-{date}.jsonl");
        assert!(filename.starts_with("memory-backup-"));
        assert!(filename.ends_with(".jsonl"));
        assert_eq!(filename, "memory-backup-2026-02-24.jsonl");
    }

    #[test]
    fn test_backup_filename_sorting_is_chronological() {
        let mut files = vec![
            "memory-backup-2026-02-25.jsonl".to_string(),
            "memory-backup-2026-01-15.jsonl".to_string(),
            "memory-backup-2026-02-24.jsonl".to_string(),
        ];
        files.sort();
        assert_eq!(files[0], "memory-backup-2026-01-15.jsonl");
        assert_eq!(files[1], "memory-backup-2026-02-24.jsonl");
        assert_eq!(files[2], "memory-backup-2026-02-25.jsonl");
    }

    #[test]
    fn test_backup_filename_pattern_matching() {
        let valid = "memory-backup-2026-02-24.jsonl";
        let invalid_prefix = "backup-2026-02-24.jsonl";
        let invalid_suffix = "memory-backup-2026-02-24.json";

        assert!(valid.starts_with("memory-backup-") && valid.ends_with(".jsonl"));
        assert!(!(invalid_prefix.starts_with("memory-backup-") && invalid_prefix.ends_with(".jsonl")));
        assert!(!(invalid_suffix.starts_with("memory-backup-") && invalid_suffix.ends_with(".jsonl")));
    }
}
