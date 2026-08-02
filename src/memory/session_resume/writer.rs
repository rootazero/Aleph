//! `SnapshotWriter` — persist `SessionSnapshot` to disk as JSON.

use super::snapshot::SessionSnapshot;
use std::collections::HashMap;
use std::path::PathBuf;

/// Maximum number of session snapshot directories to retain **per agent**.
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

    /// Create a writer using the default path `<data_dir>/sessions/`.
    ///
    /// Resolved through [`crate::utils::paths::get_data_dir`] — the single
    /// authoritative knob for relocating Aleph state, so `ALEPH_HOME` moves
    /// this store exactly like every sibling under `data/`. Hardcoding
    /// `~/.aleph/data/sessions` made a second instance write its summaries
    /// into the operator's real home and read that instance's snapshots back
    /// as its own "previous session" — contamination in both directions.
    #[must_use]
    pub fn default_path() -> Option<Self> {
        crate::utils::paths::get_data_dir()
            .ok()
            .map(|d| Self::new(d.join("sessions")))
    }

    /// Build a snapshot from a session-end summary and persist it.
    ///
    /// Producer-side convenience for the session-end hook. The owning agent
    /// is derived from the session key itself (`agent:main:main` → `main`),
    /// falling back to `fallback_agent_id` for ids that don't parse as
    /// gateway keys — the reader filters on this so agents never see each
    /// other's snapshots. The summary is the whole payload: it is LLM-written
    /// and already carries decisions / files / pending work verbatim, and
    /// deterministic keyword-scraping of that natural language is exactly what
    /// R7/P8 ban (see [`SessionSnapshot`]).
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
        };
        self.write(&snapshot)
    }

    /// Write a snapshot to `{base}/{sanitized session_id}/resume.json`.
    ///
    /// Returns the path of the written file on success.
    ///
    /// Synchronous on purpose — it is plain file work (a `create_dir_all`, an
    /// atomic write, and a retention sweep that can `remove_dir_all` up to N
    /// directories), so async callers must run it on a blocking pool rather
    /// than on a tokio worker.
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
        // Atomic (temp + fsync + rename), not truncate-then-write: a daemon
        // killed mid-write used to leave a truncated `resume.json` holding the
        // NEWEST mtime, which `load_latest` silently skips (it swallows the
        // parse error and falls through to an older session) while the corrupt
        // directory kept counting toward retention and evicting good ones.
        crate::utils::atomic_io::write_atomic(&path, json.as_bytes())?;

        self.cleanup_old_snapshots();
        Ok(path)
    }

    /// Delete `session_id`'s snapshot directory, if it has one.
    ///
    /// Lives here because this module owns the directory naming: callers must
    /// not re-derive [`super::sanitize_session_id`], or a spelling drift would
    /// silently purge a directory that never existed and leave the real one on
    /// disk. A missing directory is success — the caller only asserts that no
    /// snapshot for that session survives.
    pub fn remove_for_session(&self, session_id: &str) -> std::io::Result<()> {
        let dir = self.base_dir.join(super::sanitize_session_id(session_id));
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Remove the oldest snapshot directories beyond [`MAX_SNAPSHOTS`],
    /// **per owning agent**.
    ///
    /// Retention has to be scoped the same way the read path is or it deletes
    /// what nobody else can: `SnapshotReader::load_latest` filters survivors to
    /// one `agent_id`, so a global "newest 10 win" sweep let agent `main`
    /// rolling its conversation eleven times in an afternoon silently delete
    /// every other agent's ONLY snapshot — after which those agents got `None`
    /// forever with no diagnostic. The owner is read out of the snapshot the
    /// sweep already has to stat; a `resume.json` that will not parse has no
    /// known owner, so it is retained (and evicted) in its own bucket where it
    /// can never push out a real agent's snapshot.
    fn cleanup_old_snapshots(&self) {
        let Ok(entries) = std::fs::read_dir(&self.base_dir) else {
            return;
        };

        let mut by_agent: HashMap<Option<String>, Vec<(PathBuf, std::time::SystemTime)>> =
            HashMap::new();
        for entry in entries.filter_map(|e| e.ok()) {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let resume = dir.join("resume.json");
            let Ok(modified) = std::fs::metadata(&resume).and_then(|m| m.modified()) else {
                continue;
            };
            let owner = std::fs::read_to_string(&resume)
                .ok()
                .and_then(|text| serde_json::from_str::<SessionSnapshot>(&text).ok())
                .map(|snap| snap.agent_id);
            by_agent.entry(owner).or_default().push((dir, modified));
        }

        for mut dirs in by_agent.into_values() {
            if dirs.len() <= MAX_SNAPSHOTS {
                continue;
            }
            // Sort oldest first
            dirs.sort_by_key(|(_, t)| *t);

            let to_remove = dirs.len() - MAX_SNAPSHOTS;
            for (path, _) in dirs.into_iter().take(to_remove) {
                let _ = std::fs::remove_dir_all(path);
            }
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
    fn write_leaves_no_staging_files_behind() {
        // The atomic write stages a temp file next to the destination; a leaked
        // staging file would be indistinguishable from a snapshot directory's
        // content and would survive every future sweep.
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let path = writer.write(&make_snapshot("session-atomic")).unwrap();

        let siblings: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(siblings, vec!["resume.json".to_string()]);
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
        assert_eq!(
            restored.summary, summary,
            "the summary is the whole payload — nothing is scraped out of it"
        );
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

    #[test]
    fn cleanup_retains_per_agent_not_globally() {
        use crate::memory::session_resume::SnapshotReader;

        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let reader = SnapshotReader::new(tmp.path());

        // A quiet agent's ONE snapshot, written first (so it is the oldest by
        // mtime and a global sweep would evict it first).
        writer
            .write_from_summary("agent:quiet:main", "Quiet agent's only session.", "quiet")
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));

        // A busy agent rolls its conversation well past the retention limit.
        for i in 0..MAX_SNAPSHOTS + 4 {
            writer
                .write_from_summary(&format!("agent:busy:s-{i:02}"), "Busy.", "busy")
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        assert!(
            reader.load_latest("quiet", "none").is_some(),
            "a busy agent's retention must not delete another agent's only snapshot"
        );
        let busy_dirs = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("agent_busy_"))
            })
            .count();
        assert_eq!(
            busy_dirs, MAX_SNAPSHOTS,
            "the busy agent is still capped at MAX_SNAPSHOTS of its own"
        );
    }

    #[test]
    fn cleanup_never_lets_an_unparsable_snapshot_evict_a_good_one() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());

        // A truncated `resume.json` with the newest mtime — the exact residue a
        // daemon killed mid-write used to leave. It has no readable owner, so
        // it must not count against any agent's budget.
        for i in 0..MAX_SNAPSHOTS {
            writer
                .write_from_summary(&format!("agent:main:s-{i:02}"), "Good.", "main")
                .unwrap();
        }
        let corrupt = tmp.path().join("agent_main_truncated");
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::write(corrupt.join("resume.json"), r#"{"session_id": "agent:m"#).unwrap();

        // One more good write triggers the sweep.
        writer
            .write_from_summary("agent:main:s-99", "Good.", "main")
            .unwrap();

        let good = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                std::fs::read_to_string(e.path().join("resume.json"))
                    .ok()
                    .and_then(|t| serde_json::from_str::<SessionSnapshot>(&t).ok())
                    .is_some()
            })
            .count();
        assert_eq!(
            good, MAX_SNAPSHOTS,
            "the corrupt directory must not have evicted a readable snapshot"
        );
    }

    #[test]
    fn remove_for_session_uses_the_sanitized_directory_name() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = SnapshotWriter::new(tmp.path());
        let path = writer.write(&make_snapshot("agent:main:main")).unwrap();
        assert!(path.exists());

        // The caller passes the RAW session key; the mapping to
        // `agent_main_main` must happen in here.
        writer.remove_for_session("agent:main:main").unwrap();
        assert!(!path.exists());
        assert!(!tmp.path().join("agent_main_main").exists());

        // Absent is success, not an error.
        writer.remove_for_session("agent:main:main").unwrap();
    }
}
