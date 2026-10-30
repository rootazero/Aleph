//! Skills manifest — tracks installed skills, their sources, and bundled version.
//!
//! Location: `~/.aleph/skills/manifest.json`

use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, warn};

/// Monotonic counter for unique tmp filenames in `save()`. Process-local; the
/// (pid, counter, nanos) triple is unique-by-construction so two concurrent
/// saves in the same process (or a crash-leftover from a prior save) cannot
/// collide on the tmp path.
static SAVE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRegistry {
    /// Version of the last successfully extracted bundled content.
    pub bundled_version: String,
    /// Per-skill metadata keyed by skill directory name.
    pub skills: BTreeMap<String, SkillEntry>,
}

/// Where a skill was installed from.
///
/// Named `SkillOrigin` to avoid collision with `domain::skill::SkillSource`
/// and `skills::registry::SkillSource`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillOrigin {
    /// Bundled with the binary, extracted on startup.
    Official,
    /// Installed from a GitHub URL.
    Github,
    /// Manually placed in the skills directory.
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Where this skill came from.
    pub source: SkillOrigin,
    /// Version when installed (for official skills, matches `bundled_version`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Source URL (for github-installed skills).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl InstallRegistry {
    /// Load manifest from disk. Returns None if file doesn't exist or is corrupt.
    pub fn load(skills_dir: &Path) -> Option<Self> {
        let path = skills_dir.join("manifest.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(manifest) => Some(manifest),
                Err(e) => {
                    warn!(error = %e, path = %path.display(), "Corrupt manifest.json, will recreate");
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                warn!(error = %e, path = %path.display(), "Failed to read manifest.json");
                None
            }
        }
    }

    /// Save manifest to disk.
    ///
    /// Atomic: write to a process-unique temp file via `create_new(true)` so two
    /// parallel writers cannot collide on the tmp path, then atomically rename
    /// onto the destination. On Windows the rename of an existing file is
    /// rejected (`AlreadyExists`); the trailing remove-then-rename covers that
    /// case and restores the previous manifest if that rename fails, so a
    /// failed swap can never leave the manifest permanently missing. POSIX
    /// rename replaces atomically, so the retry never fires there.
    pub fn save(&self, skills_dir: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let path = skills_dir.join("manifest.json");
        let content = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // BUNDLED-R4-01 + BUNDLED-R4-07: unique-per-call tmp name avoids both
        // the symlink-planting race (attacker plants a symlink at a fixed
        // tmp path to redirect the write) and the stale-tmp crash-leftover
        // (a previous crash leaves a fixed tmp name on disk, blocking all
        // future saves). Each save now picks its own atomic-counter+nanos
        // tmp name, eliminating both surfaces.
        let tmp_path = unique_tmp_path(skills_dir, "manifest.tmp");
        // On any error below, drop the staging file so the next save does not
        // find a half-written one (create_new(true) would block on it).
        let cleanup_tmp = |e: std::io::Error| -> std::io::Result<()> {
            let _ = std::fs::remove_file(&tmp_path);
            Err(e)
        };
        // `create_new(true)` refuses on a pre-existing tmp file, so the writer
        // gets a unique-by-construction handle even when two startups race.
        // A concurrent writer is mid-save: refuse rather than clobber its tmp;
        // the next save will pick a fresh name.
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        if let Err(e) = f.write_all(content.as_bytes()) {
            return cleanup_tmp(e);
        }
        match std::fs::rename(&tmp_path, &path) {
            Ok(()) => {}
            // BUNDLED-R4-07: Windows' rename refuses on an existing target.
            // POSIX rename replaces atomically, so the retry never fires there.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // B41-7: keep the previous manifest recoverable before the
                // destination is unlinked. If the fallback rename fails, the
                // manifest must be put back: a missing manifest is read as a
                // first run, which reclassifies every official skill as Local
                // and drops its provenance permanently.
                let previous = match std::fs::read(&path) {
                    Ok(bytes) => bytes,
                    Err(read_err) => return cleanup_tmp(read_err),
                };
                if let Err(rm) = std::fs::remove_file(&path) {
                    return cleanup_tmp(rm);
                }
                if let Err(rn) = std::fs::rename(&tmp_path, &path) {
                    if let Err(restore_err) = restore_manifest(skills_dir, &path, &previous) {
                        warn!(
                            error = %restore_err,
                            path = %path.display(),
                            "Failed to restore previous skills manifest after fallback rename failed"
                        );
                    }
                    return cleanup_tmp(rn);
                }
            }
            Err(e) => return cleanup_tmp(e),
        }
        debug!(path = %path.display(), "Saved skills manifest");
        Ok(())
    }

    /// Create an empty manifest with the given version.
    #[must_use]
    pub fn new(version: &str) -> Self {
        Self {
            bundled_version: version.to_string(),
            skills: BTreeMap::new(),
        }
    }

    /// Reconcile manifest with actual directory contents.
    /// - Directories not in manifest → add as Local
    /// - Manifest entries without directories → remove
    ///
    /// Returns `Err` if the skills directory could not be read (caller should
    /// avoid saving a potentially stale manifest). Per-entry errors during the
    /// `read_dir` iteration are counted and logged: silently dropping them
    /// would let a race-on-unlink or a permissions glitch on a subdir classify
    /// the affected entry as "removed" and remove it from the manifest on the
    /// next save — a quiet loss of provenance.
    pub fn reconcile(&mut self, skills_dir: &Path) -> std::io::Result<()> {
        // Find directories on disk
        let entries = std::fs::read_dir(skills_dir)?;
        // A `Cell` because the two closures below both need to bump the
        // counter; ordinary `&mut` would double-borrow through the iterator
        // chain. The closure body is single-threaded by construction.
        let skip_count: Cell<u32> = Cell::new(0);
        let on_disk: HashSet<String> = entries
            .filter_map(|e| match e {
                Ok(e) => Some(e),
                Err(err) => {
                    skip_count.set(skip_count.get() + 1);
                    tracing::warn!(
                        error = %err,
                        dir = %skills_dir.display(),
                        "read_dir entry error during reconcile (will be treated as absent)"
                    );
                    None
                }
            })
            .filter(|e| {
                // Use symlink_metadata so we don't follow symlinks — a symlink
                // pointing outside the skills dir should not be treated as a skill.
                match e.path().symlink_metadata() {
                    Ok(m) => m.is_dir(),
                    Err(err) => {
                        skip_count.set(skip_count.get() + 1);
                        tracing::warn!(
                            error = %err,
                            path = %e.path().display(),
                            "symlink_metadata error during reconcile (will be treated as absent)"
                        );
                        false
                    }
                }
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        let skipped = skip_count.get();
        if skipped > 0 {
            // Fail-closed: skip BOTH the "add missing" and the "retain"
            // passes. The previous shape logged a warning but still ran
            // `self.skills.retain(...)`, which silently reaped the
            // affected entries' manifest rows on the next save — a quiet
            // loss of provenance on a transient read error.
            tracing::warn!(
                skipped = skipped,
                dir = %skills_dir.display(),
                "reconcile: skipping provenance update to preserve entries that could not be read"
            );
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                format!(
                    "reconcile: {skipped} entries in {} could not be read; \
                     not updating manifest to preserve provenance",
                    skills_dir.display(),
                ),
            ));
        }

        // Add missing directories as Local
        for name in &on_disk {
            if !self.skills.contains_key(name) {
                debug!(skill = %name, "Discovered untracked skill, marking as local");
                self.skills.insert(
                    name.clone(),
                    SkillEntry {
                        source: SkillOrigin::Local,
                        version: None,
                        url: None,
                    },
                );
            }
        }

        // Remove manifest entries for deleted directories
        self.skills.retain(|name, _| on_disk.contains(name));
        Ok(())
    }

    /// Check if a skill is official.
    #[must_use]
    pub fn is_official(&self, name: &str) -> bool {
        self.skills
            .get(name)
            .is_some_and(|e| e.source == SkillOrigin::Official)
    }
}

/// Build a process-unique temp path inside `skills_dir`.
///
/// The (pid, counter, nanos) triple is unique-by-construction so a
/// concurrent writer in the same process, or a crash-leftover from a prior
/// save, cannot collide with the returned path.
fn unique_tmp_path(skills_dir: &Path, tag: &str) -> PathBuf {
    skills_dir.join(format!(
        ".{tag}.{}.{}.{}",
        std::process::id(),
        SAVE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

/// Write `content` to `path` through a fresh unique temp file and rename it
/// into place, so the restored manifest is never observed half-written.
fn restore_manifest(skills_dir: &Path, path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let restore_tmp = unique_tmp_path(skills_dir, "manifest.restore.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&restore_tmp)?;
    if let Err(e) = f.write_all(content) {
        let _ = std::fs::remove_file(&restore_tmp);
        return Err(e);
    }
    match std::fs::rename(&restore_tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&restore_tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn official_entry(version: &str) -> SkillEntry {
        SkillEntry {
            source: SkillOrigin::Official,
            version: Some(version.to_string()),
            url: None,
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = InstallRegistry::new("1.0.0");
        m.skills.insert("foo".to_string(), official_entry("1.0.0"));
        m.save(dir.path()).unwrap();

        let loaded = InstallRegistry::load(dir.path()).expect("manifest should load");
        assert_eq!(loaded.bundled_version, "1.0.0");
        assert!(loaded.is_official("foo"));
    }

    #[test]
    fn load_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(InstallRegistry::load(dir.path()).is_none());
    }

    #[test]
    fn load_returns_none_for_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("manifest.json"), b"{ not json").unwrap();
        assert!(InstallRegistry::load(dir.path()).is_none());
    }

    #[test]
    fn reconcile_adds_untracked_dir_as_local() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("user-skill")).unwrap();

        let mut m = InstallRegistry::new("1.0.0");
        m.reconcile(dir.path()).unwrap();

        let entry = m.skills.get("user-skill").expect("untracked dir tracked");
        assert_eq!(entry.source, SkillOrigin::Local);
        assert!(!m.is_official("user-skill"));
    }

    #[test]
    fn reconcile_removes_entries_for_deleted_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = InstallRegistry::new("1.0.0");
        // Tracked in manifest but never present on disk.
        m.skills
            .insert("ghost".to_string(), official_entry("1.0.0"));

        m.reconcile(dir.path()).unwrap();
        assert!(!m.skills.contains_key("ghost"));
    }

    #[test]
    fn reconcile_preserves_official_entry_present_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("off")).unwrap();

        let mut m = InstallRegistry::new("1.0.0");
        m.skills.insert("off".to_string(), official_entry("1.0.0"));
        m.reconcile(dir.path()).unwrap();

        // Still official — reconcile must not downgrade tracked official skills.
        assert!(m.is_official("off"));
    }

    #[test]
    fn restore_manifest_writes_previous_content_back() {
        // The Windows fallback path unlinks the destination before retrying the
        // rename; if that rename fails the previous manifest must be restored
        // so provenance is not lost.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        std::fs::write(&path, b"{\"bundled_version\":\"1.0.0\"}").unwrap();
        // The fallback path unlinks the destination before the retry, so the
        // restore runs against a missing destination.
        std::fs::remove_file(&path).unwrap();

        restore_manifest(dir.path(), &path, b"{\"bundled_version\":\"1.0.0\"}").unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"bundled_version\":\"1.0.0\"}"
        );
        // The staging file used for the restore must not be left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".manifest.restore.tmp."))
            .collect();
        assert!(leftovers.is_empty(), "unexpected leftovers: {leftovers:?}");
    }

    #[test]
    fn reconcile_errors_when_dir_unreadable() {
        // Pointing at a non-existent directory must surface an error so the
        // caller avoids saving a stale manifest.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let mut m = InstallRegistry::new("1.0.0");
        assert!(m.reconcile(&missing).is_err());
    }
}
