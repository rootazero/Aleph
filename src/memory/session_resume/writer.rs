//! SnapshotWriter — persist SessionSnapshot to disk as JSON.

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
    pub fn default_path() -> Option<Self> {
        dirs::home_dir().map(|h| Self::new(h.join(".aleph/data/sessions")))
    }

    /// Write a snapshot to `{base}/{session_id}/resume.json`.
    ///
    /// Returns the path of the written file on success.
    pub fn write(&self, snapshot: &SessionSnapshot) -> std::io::Result<PathBuf> {
        let dir = self.base_dir.join(&snapshot.session_id);
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
        assert!(path.ends_with("session-abc/resume.json"));

        let content = std::fs::read_to_string(&path).unwrap();
        let restored: SessionSnapshot = serde_json::from_str(&content).unwrap();
        assert_eq!(restored.session_id, "session-abc");
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
