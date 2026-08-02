//! `SnapshotReader` — load the most recent `SessionSnapshot` from disk.

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

    /// Create a reader using the default path `<data_dir>/sessions/`.
    ///
    /// Same resolution as [`super::SnapshotWriter::default_path`] — through
    /// [`crate::utils::paths::get_data_dir`], so `ALEPH_HOME` relocates the
    /// read side and the write side together. A reader that ignored the knob
    /// while the writer honoured it (or vice versa) would hand one instance's
    /// summaries to another as its own "previous session".
    #[must_use]
    pub fn default_path() -> Option<Self> {
        crate::utils::paths::get_data_dir()
            .ok()
            .map(|d| Self::new(d.join("sessions")))
    }

    /// Load `agent_id`'s most recently modified snapshot, excluding
    /// `exclude_session_id`.
    ///
    /// All agents share one snapshot directory, so candidates are filtered to
    /// the requesting agent — agent B's prompt assembly must never inject
    /// agent A's session summary. Legacy agent-less snapshots (written before
    /// the agent dimension existed) carry an empty `agent_id` and are treated
    /// as non-matching. The exclude comparison runs on the sanitized form of
    /// the id — the same mapping the writer uses for directory names.
    ///
    /// Returns `None` when no valid snapshot is found or the base directory
    /// does not exist.
    #[must_use]
    pub fn load_latest(&self, agent_id: &str, exclude_session_id: &str) -> Option<SessionSnapshot> {
        let entries = std::fs::read_dir(&self.base_dir).ok()?;
        let exclude = super::sanitize_session_id(exclude_session_id);

        let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().is_dir() && e.file_name().to_str().is_some_and(|name| name != exclude)
            })
            .filter_map(|e| {
                let resume = e.path().join("resume.json");
                let modified = std::fs::metadata(&resume).ok()?.modified().ok()?;
                Some((resume, modified))
            })
            .collect();

        // Sort newest first
        candidates.sort_by_key(|x| std::cmp::Reverse(x.1));

        for (path, _) in candidates {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&content) {
                    if snapshot.agent_id == agent_id {
                        return Some(snapshot);
                    }
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

    fn make_snapshot(id: &str, agent: &str, summary: &str) -> SessionSnapshot {
        SessionSnapshot {
            session_id: id.to_string(),
            agent_id: agent.to_string(),
            created_at: Utc::now(),
            summary: summary.to_string(),
        }
    }

    #[test]
    fn load_latest_returns_most_recent() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer
            .write(&make_snapshot("old", "main", "Old session"))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        writer
            .write(&make_snapshot("new", "main", "New session"))
            .unwrap();

        let latest = reader.load_latest("main", "none").unwrap();
        assert_eq!(latest.session_id, "new");
        assert_eq!(latest.summary, "New session");
    }

    #[test]
    fn load_latest_excludes_current_session() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer
            .write(&make_snapshot("old", "main", "Old session"))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        writer
            .write(&make_snapshot("current", "main", "Current session"))
            .unwrap();

        let latest = reader.load_latest("main", "current").unwrap();
        assert_eq!(latest.session_id, "old");
    }

    #[test]
    fn load_latest_excludes_current_session_with_gateway_key() {
        // The exclude id arrives raw (`agent:main:main`) while the directory
        // name is sanitized (`agent_main_main`); the comparison must use the
        // same mapping or fixing the writer would break exclusion.
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer
            .write(&make_snapshot("agent:main:main", "main", "Current session"))
            .unwrap();

        assert!(
            reader.load_latest("main", "agent:main:main").is_none(),
            "the current session's own snapshot must be excluded"
        );
    }

    #[test]
    fn load_latest_filters_by_agent() {
        // Agent B must never see agent A's session summary.
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer
            .write(&make_snapshot("a-sess", "agent-a", "Agent A session"))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        writer
            .write(&make_snapshot("b-sess", "agent-b", "Agent B session"))
            .unwrap();

        // Agent A gets its own snapshot even though B's is newer.
        let for_a = reader.load_latest("agent-a", "none").unwrap();
        assert_eq!(for_a.session_id, "a-sess");
        // Agent B gets its own.
        let for_b = reader.load_latest("agent-b", "none").unwrap();
        assert_eq!(for_b.session_id, "b-sess");
        // An agent with no snapshots gets nothing — not someone else's.
        assert!(reader.load_latest("agent-c", "none").is_none());
    }

    #[test]
    fn load_latest_skips_legacy_agentless_snapshots() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = SnapshotReader::new(tmp.path());

        // Simulate a pre-agent-dimension file: no agent_id key at all. It also
        // still carries the four structured fields that were later cut — real
        // on-disk files do, and unknown keys must not fail the load.
        let dir = tmp.path().join("legacy-sess");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("resume.json"),
            r#"{
                "session_id": "legacy-sess",
                "created_at": "2026-01-01T00:00:00Z",
                "summary": "Legacy snapshot.",
                "key_decisions": [],
                "active_files": [],
                "tool_state": null,
                "pending_tasks": []
            }"#,
        )
        .unwrap();

        assert!(
            reader.load_latest("main", "none").is_none(),
            "agent-less legacy snapshots must be treated as non-matching"
        );
    }

    #[test]
    fn load_latest_returns_none_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = SnapshotReader::new(tmp.path());

        assert!(reader.load_latest("main", "any").is_none());
    }

    #[test]
    fn load_latest_returns_none_when_all_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        writer
            .write(&make_snapshot("only", "main", "Only session"))
            .unwrap();

        assert!(reader.load_latest("main", "only").is_none());
    }
}
