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
    /// Producer-side convenience for the session-end hook. The owning agent
    /// is derived from the session key itself (`agent:main:main` → `main`),
    /// falling back to `fallback_agent_id` for ids that don't parse as
    /// gateway keys — the reader filters on this so agents never see each
    /// other's snapshots. `key_decisions` stays empty: the summary text is
    /// LLM-written and already carries decisions verbatim; deterministic
    /// keyword-scraping of that natural language is exactly what R7/P8 ban.
    /// The remaining fields have no session-end source today and stay empty
    /// ([`SessionSnapshot::to_prompt_text`] / the assembler's snapshot
    /// candidate both omit empty sections).
    pub fn write_from_summary(
        &self,
        session_id: &str,
        summary: &str,
        fallback_agent_id: &str,
    ) -> std::io::Result<PathBuf> {
        let agent_id = crate::routing::session_key::SessionKey::from_key_string(session_id)
            .map_or_else(
                || fallback_agent_id.to_string(),
                |k| k.agent_id().to_string(),
            );
        let snapshot = SessionSnapshot {
            session_id: session_id.to_string(),
            agent_id,
            created_at: chrono::Utc::now(),
            summary: summary.to_string(),
            key_decisions: Vec::new(),
            active_files: Vec::new(),
            tool_state: None,
            pending_tasks: Vec::new(),
        };
        self.write(&snapshot)
    }

    /// Write a snapshot to `{base}/{sanitized session_id}/resume.json`.
    ///
    /// Returns the path of the written file on success.
    pub fn write(&self, snapshot: &SessionSnapshot) -> std::io::Result<PathBuf> {
        // Sanitize the session id so it cannot escape `base_dir` and so the
        // directory name is legal on Windows (gateway keys contain `:`) —
        // see `sanitize_session_id`.
        let safe_id = super::sanitize_session_id(&snapshot.session_id);
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
            agent_id: "main".to_string(),
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
    fn write_sanitizes_gateway_key_session_ids() {
        // Gateway keys like `agent:main:main` contain `:`, which is illegal
        // in Windows file names — the directory name must not carry it.
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let snap = make_snapshot("agent:main:main");
        let path = writer.write(&snap).unwrap();

        assert!(path.exists());
        let dir_name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap();
        assert_eq!(dir_name, "agent_main_main");
        // The snapshot itself keeps the original id.
        let content = std::fs::read_to_string(&path).unwrap();
        let restored: SessionSnapshot = serde_json::from_str(&content).unwrap();
        assert_eq!(restored.session_id, "agent:main:main");
    }

    #[test]
    fn write_from_summary_roundtrips_through_reader() {
        use crate::memory::session_resume::SnapshotReader;

        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        let summary = "We decided to use SQLite. Everything else was routine.";
        writer
            .write_from_summary("agent:main:prev", summary, "main")
            .unwrap();

        // The next session (a different id, same agent) must see the previous
        // snapshot.
        let restored = reader.load_latest("main", "agent:main:next").unwrap();
        assert_eq!(restored.session_id, "agent:main:prev");
        assert_eq!(restored.agent_id, "main", "agent derived from session key");
        assert_eq!(restored.summary, summary);
        assert!(
            restored.key_decisions.is_empty(),
            "no deterministic decision scraping — the summary carries decisions verbatim"
        );
        assert!(restored.active_files.is_empty());
        assert!(restored.pending_tasks.is_empty());
    }

    #[test]
    fn write_from_summary_falls_back_when_session_id_is_not_a_key() {
        use crate::memory::session_resume::SnapshotReader;

        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer
            .write_from_summary("adhoc-session", "Some summary.", "fallback-agent")
            .unwrap();
        let restored = reader.load_latest("fallback-agent", "other").unwrap();
        assert_eq!(restored.agent_id, "fallback-agent");
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
