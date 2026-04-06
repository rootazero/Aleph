//! SnapshotReader — load the most recent SessionSnapshot from disk.

use super::snapshot::SessionSnapshot;
use std::path::PathBuf;

/// Reads [`SessionSnapshot`] instances from disk.
pub struct SnapshotReader {
    base_dir: PathBuf,
}

impl SnapshotReader {
    /// Create a reader targeting the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Create a reader using the default path `~/.aleph/data/sessions/`.
    pub fn default_path() -> Option<Self> {
        dirs::home_dir().map(|h| Self::new(h.join(".aleph/data/sessions")))
    }

    /// Load the most recently modified snapshot, excluding `exclude_session_id`.
    ///
    /// Returns `None` when no valid snapshot is found or the base directory
    /// does not exist.
    pub fn load_latest(&self, exclude_session_id: &str) -> Option<SessionSnapshot> {
        let entries = std::fs::read_dir(&self.base_dir).ok()?;

        let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().is_dir()
                    && e.file_name()
                        .to_str()
                        .is_some_and(|name| name != exclude_session_id)
            })
            .filter_map(|e| {
                let resume = e.path().join("resume.json");
                let modified = std::fs::metadata(&resume).ok()?.modified().ok()?;
                Some((resume, modified))
            })
            .collect();

        // Sort newest first
        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        for (path, _) in candidates {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&content) {
                    return Some(snapshot);
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::session_resume::SnapshotWriter;
    use chrono::Utc;

    fn make_snapshot(id: &str, summary: &str) -> SessionSnapshot {
        SessionSnapshot {
            session_id: id.to_string(),
            created_at: Utc::now(),
            summary: summary.to_string(),
            key_decisions: vec![],
            active_files: vec![],
            tool_state: None,
            pending_tasks: vec![],
        }
    }

    #[test]
    fn load_latest_returns_most_recent() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer.write(&make_snapshot("old", "Old session")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        writer
            .write(&make_snapshot("new", "New session"))
            .unwrap();

        let latest = reader.load_latest("none").unwrap();
        assert_eq!(latest.session_id, "new");
        assert_eq!(latest.summary, "New session");
    }

    #[test]
    fn load_latest_excludes_current_session() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer.write(&make_snapshot("old", "Old session")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        writer
            .write(&make_snapshot("current", "Current session"))
            .unwrap();

        let latest = reader.load_latest("current").unwrap();
        assert_eq!(latest.session_id, "old");
    }

    #[test]
    fn load_latest_returns_none_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = SnapshotReader::new(tmp.path());

        assert!(reader.load_latest("any").is_none());
    }

    #[test]
    fn load_latest_returns_none_when_all_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer
            .write(&make_snapshot("only", "Only session"))
            .unwrap();

        assert!(reader.load_latest("only").is_none());
    }
}
