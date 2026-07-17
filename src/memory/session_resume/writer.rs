//! `SnapshotWriter` — persist `SessionSnapshot` to disk as JSON.

use super::snapshot::SessionSnapshot;
use std::path::PathBuf;

/// Maximum number of session snapshot directories to retain.
const MAX_SNAPSHOTS: usize = 10;

/// Writes [`SessionSnapshot`] instances to disk.
pub struct SnapshotWriter {
    base_dir: PathBuf,
}

impl SnapshotWriter {
    /// Create a writer targeting the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Create a writer using the default path `~/.aleph/data/sessions/`.
    #[must_use]
    pub fn default_path() -> Option<Self> {
        dirs::home_dir().map(|h| Self::new(h.join(".aleph/data/sessions")))
    }

    /// Build a snapshot from a session-end summary and persist it.
    ///
    /// Producer-side convenience for the session-end hook: derives
    /// `key_decisions` from the summary via
    /// [`SessionSnapshot::extract_decisions`] and stamps `created_at` now.
    /// The remaining fields have no session-end source today and stay empty
    /// ([`SessionSnapshot::to_prompt_text`] / the assembler's snapshot
    /// candidate both omit empty sections).
    pub fn write_from_summary(&self, session_id: &str, summary: &str) -> std::io::Result<PathBuf> {
        let snapshot = SessionSnapshot {
            session_id: session_id.to_string(),
            created_at: chrono::Utc::now(),
            summary: summary.to_string(),
            key_decisions: SessionSnapshot::extract_decisions(summary),
            active_files: Vec::new(),
            tool_state: None,
            pending_tasks: Vec::new(),
        };
        self.write(&snapshot)
    }

    /// Write a snapshot to `{base}/{session_id}/resume.json`.
    ///
    /// Returns the path of the written file on success.
    pub fn write(&self, snapshot: &SessionSnapshot) -> std::io::Result<PathBuf> {
        // Sanitize the session id so it cannot escape `base_dir` via path
        // separators or parent references.
        let safe_id = snapshot
            .session_id
            .replace(['/', '\\', '\0'], "_")
            .replace("..", "__");
        let dir = self.base_dir.join(&safe_id);
        std::fs::create_dir_all(&dir)?;

        let path = dir.join("resume.json");
        let json = serde_json::to_string_pretty(snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, json)?;

        self.cleanup_old_snapshots();
        Ok(path)
    }

    /// Remove the oldest snapshot directories beyond [`MAX_SNAPSHOTS`].
    fn cleanup_old_snapshots(&self) {
        let Ok(entries) = std::fs::read_dir(&self.base_dir) else {
            return;
        };

        let mut dirs: Vec<(PathBuf, std::time::SystemTime)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let resume = e.path().join("resume.json");
                let modified = std::fs::metadata(&resume).ok()?.modified().ok()?;
                Some((e.path(), modified))
            })
            .collect();

        if dirs.len() <= MAX_SNAPSHOTS {
            return;
        }

        // Sort oldest first
        dirs.sort_by_key(|(_, t)| *t);

        let to_remove = dirs.len() - MAX_SNAPSHOTS;
        for (path, _) in dirs.into_iter().take(to_remove) {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_snapshot(id: &str) -> SessionSnapshot {
        SessionSnapshot {
            session_id: id.to_string(),
            created_at: Utc::now(),
            summary: format!("Summary for {id}"),
            key_decisions: vec![],
            active_files: vec![],
            tool_state: None,
            pending_tasks: vec![],
        }
    }

    #[test]
    fn write_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let snap = make_snapshot("session-abc");
        let path = writer.write(&snap).unwrap();

        assert!(path.exists());
        assert!(
            path.ends_with("session-abc\\resume.json") || path.ends_with("session-abc/resume.json")
        );

        let content = std::fs::read_to_string(&path).unwrap();
        let restored: SessionSnapshot = serde_json::from_str(&content).unwrap();
        assert_eq!(restored.session_id, "session-abc");
    }

    #[test]
    fn write_from_summary_roundtrips_through_reader() {
        use crate::memory::session_resume::SnapshotReader;

        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        let summary = "We decided to use SQLite. Everything else was routine.";
        writer.write_from_summary("sess-prev", summary).unwrap();

        // The next session (a different id) must see the previous snapshot.
        let restored = reader.load_latest("sess-next").unwrap();
        assert_eq!(restored.session_id, "sess-prev");
        assert_eq!(restored.summary, summary);
        assert_eq!(
            restored.key_decisions,
            vec!["We decided to use SQLite".to_string()],
            "decisions must be derived from the summary"
        );
        assert!(restored.active_files.is_empty());
        assert!(restored.pending_tasks.is_empty());
    }

    #[test]
    fn cleanup_removes_oldest_beyond_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());

        // Write 12 snapshots
        for i in 0..12 {
            let snap = make_snapshot(&format!("s-{i:02}"));
            writer.write(&snap).unwrap();
            // Small delay to ensure distinct modification times
            std::thread::sleep(std::time::Duration::from_millis(15));
        }

        // Count remaining directories with resume.json
        let count = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("resume.json").exists())
            .count();

        assert_eq!(count, MAX_SNAPSHOTS);
    }
}
