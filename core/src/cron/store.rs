//! JSON Atomic Store for Cron Jobs
//!
//! Crash-safe JSON file persistence using tmp+fsync+rename pattern.
//! The CronStore is designed to be wrapped in a `tokio::sync::Mutex`
//! by the service layer.

use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::time::SystemTime;

use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::cron::config::CronJob;

/// Current store format version
const CURRENT_VERSION: u32 = 1;

// ── On-disk format ─────────────────────────────────────────────────────

/// On-disk JSON format for cron job persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronStoreFile {
    pub version: u32,
    pub jobs: Vec<CronJob>,
}

// ── In-memory store ────────────────────────────────────────────────────

/// In-memory store with dirty tracking and atomic persistence
pub struct CronStore {
    path: PathBuf,
    file: CronStoreFile,
    last_mtime: Option<SystemTime>,
    dirty: bool,
}

// ── Atomic write ───────────────────────────────────────────────────────

/// Write data to a file atomically using tmp+fsync+rename pattern.
fn atomic_write(path: &PathBuf, data: &[u8]) -> io::Result<()> {
    // Create parent dirs if needed
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Build tmp file name: {filename}.{pid}.{rand:08x}.tmp
    let pid = std::process::id();
    let rand_suffix: u32 = rand::thread_rng().gen();
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let tmp_name = format!("{}.{}.{:08x}.tmp", file_name, pid, rand_suffix);
    let tmp_path = path.with_file_name(&tmp_name);

    // Write to tmp file
    let mut f = fs::File::create(&tmp_path)?;
    f.write_all(data)?;
    f.sync_all()?;

    // Rename existing file to .bak (best-effort)
    if path.exists() {
        let bak_path = path.with_extension("bak");
        let _ = fs::rename(path, &bak_path);
    }

    // Atomic swap: rename tmp to target
    fs::rename(&tmp_path, path)?;

    Ok(())
}

// ── Migration ──────────────────────────────────────────────────────────

/// Apply store migrations from older versions to current.
fn migrate_store(file: &mut CronStoreFile) {
    if file.version < 1 {
        // Version 0→1: placeholder for future migrations
        file.version = 1;
    }
    // Future migrations go here: if file.version < 2 { ... file.version = 2; }
}

// ── CronStore impl ────────────────────────────────────────────────────

impl CronStore {
    /// Load the store from disk, recovering from .bak if needed.
    ///
    /// - If `path` exists: read and parse JSON, apply migrations if needed.
    /// - If `path` doesn't exist but `.bak` does: rename bak→path and retry.
    /// - If neither exists: return empty store with version=1.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        if path.exists() {
            let data =
                fs::read_to_string(&path).map_err(|e| format!("failed to read store: {e}"))?;
            let mut file: CronStoreFile =
                serde_json::from_str(&data).map_err(|e| format!("failed to parse store: {e}"))?;

            if file.version < CURRENT_VERSION {
                migrate_store(&mut file);
            }

            let mtime = fs::metadata(&path).ok().and_then(|m| m.modified().ok());

            Ok(Self {
                path,
                file,
                last_mtime: mtime,
                dirty: false,
            })
        } else {
            // Try .bak recovery
            let bak_path = path.with_extension("bak");
            if bak_path.exists() {
                warn!("store file missing, recovering from .bak");
                fs::rename(&bak_path, &path)
                    .map_err(|e| format!("failed to recover .bak: {e}"))?;
                return Self::load(path);
            }

            // No file at all — empty store
            Ok(Self {
                path,
                file: CronStoreFile {
                    version: CURRENT_VERSION,
                    jobs: Vec::new(),
                },
                last_mtime: None,
                dirty: false,
            })
        }
    }

    /// Reload from disk if the file's mtime has changed.
    /// Returns true if the store was actually reloaded.
    pub fn reload_if_changed(&mut self) -> Result<bool, String> {
        if !self.path.exists() {
            return Ok(false);
        }

        let current_mtime = fs::metadata(&self.path)
            .ok()
            .and_then(|m| m.modified().ok());

        if current_mtime == self.last_mtime {
            return Ok(false);
        }

        self.force_reload()?;
        Ok(true)
    }

    /// Always reload from disk, discarding in-memory state.
    pub fn force_reload(&mut self) -> Result<(), String> {
        if !self.path.exists() {
            return Ok(());
        }

        let data =
            fs::read_to_string(&self.path).map_err(|e| format!("failed to read store: {e}"))?;
        let mut file: CronStoreFile =
            serde_json::from_str(&data).map_err(|e| format!("failed to parse store: {e}"))?;

        if file.version < CURRENT_VERSION {
            migrate_store(&mut file);
        }

        self.last_mtime = fs::metadata(&self.path)
            .ok()
            .and_then(|m| m.modified().ok());
        self.file = file;
        self.dirty = false;

        Ok(())
    }

    /// Persist the store to disk if dirty.
    /// Uses atomic write (tmp+fsync+rename) for crash safety.
    pub fn persist(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }

        let data = serde_json::to_string_pretty(&self.file)
            .map_err(|e| format!("failed to serialize store: {e}"))?;

        atomic_write(&self.path, data.as_bytes())
            .map_err(|e| format!("failed to write store: {e}"))?;

        self.last_mtime = fs::metadata(&self.path)
            .ok()
            .and_then(|m| m.modified().ok());
        self.dirty = false;

        Ok(())
    }

    /// Mark the store as dirty (needs persistence).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    // ── Job accessors ──────────────────────────────────────────────────

    /// Get an immutable slice of all jobs.
    pub fn jobs(&self) -> &[CronJob] {
        &self.file.jobs
    }

    /// Get a mutable reference to the jobs vec. Auto-marks dirty.
    pub fn jobs_mut(&mut self) -> &mut Vec<CronJob> {
        self.dirty = true;
        &mut self.file.jobs
    }

    /// Find a job by ID.
    pub fn get_job(&self, id: &str) -> Option<&CronJob> {
        self.file.jobs.iter().find(|j| j.id == id)
    }

    /// Find a job by ID (mutable). Auto-marks dirty.
    pub fn get_job_mut(&mut self, id: &str) -> Option<&mut CronJob> {
        self.dirty = true;
        self.file.jobs.iter_mut().find(|j| j.id == id)
    }

    /// Add a job. Marks dirty.
    pub fn add_job(&mut self, job: CronJob) {
        self.file.jobs.push(job);
        self.dirty = true;
    }

    /// Remove a job by ID. Returns the removed job if found. Marks dirty.
    pub fn remove_job(&mut self, id: &str) -> Option<CronJob> {
        let pos = self.file.jobs.iter().position(|j| j.id == id)?;
        self.dirty = true;
        Some(self.file.jobs.remove(pos))
    }

    /// Number of jobs in the store.
    pub fn job_count(&self) -> usize {
        self.file.jobs.len()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::ScheduleKind;
    use tempfile::TempDir;

    fn make_test_job(name: &str) -> CronJob {
        CronJob::new(
            name,
            "agent-1",
            "test prompt",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        )
    }

    #[test]
    fn load_empty_creates_new_store() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");

        let store = CronStore::load(path).unwrap();
        assert_eq!(store.file.version, 1);
        assert_eq!(store.job_count(), 0);
    }

    #[test]
    fn add_persist_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");

        // Create, add job, persist
        {
            let mut store = CronStore::load(path.clone()).unwrap();
            let job = make_test_job("Daily Report");
            store.add_job(job);
            assert_eq!(store.job_count(), 1);
            store.persist().unwrap();
        }

        // Reload from disk
        {
            let store = CronStore::load(path).unwrap();
            assert_eq!(store.job_count(), 1);
            assert_eq!(store.jobs()[0].name, "Daily Report");
        }
    }

    #[test]
    fn remove_job() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");

        let mut store = CronStore::load(path).unwrap();
        let job1 = make_test_job("Job A");
        let job2 = make_test_job("Job B");
        let id1 = job1.id.clone();

        store.add_job(job1);
        store.add_job(job2);
        assert_eq!(store.job_count(), 2);

        let removed = store.remove_job(&id1);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name, "Job A");
        assert_eq!(store.job_count(), 1);
        assert_eq!(store.jobs()[0].name, "Job B");

        // Removing non-existent returns None
        assert!(store.remove_job("nonexistent").is_none());
    }

    #[test]
    fn persist_skips_when_not_dirty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");

        let mut store = CronStore::load(path.clone()).unwrap();
        // Not dirty, persist should be a no-op
        store.persist().unwrap();
        // File should not exist since we never wrote anything
        assert!(!path.exists());
    }

    #[test]
    fn bak_recovery() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");
        let bak_path = dir.path().join("cron.bak");

        // Write a .bak file with a job
        let store_file = CronStoreFile {
            version: 1,
            jobs: vec![make_test_job("Recovered Job")],
        };
        let data = serde_json::to_string_pretty(&store_file).unwrap();
        fs::write(&bak_path, data).unwrap();

        // Load should recover from .bak
        let store = CronStore::load(path.clone()).unwrap();
        assert_eq!(store.job_count(), 1);
        assert_eq!(store.jobs()[0].name, "Recovered Job");

        // .bak should have been renamed to the main path
        assert!(path.exists());
        assert!(!bak_path.exists());
    }

    #[test]
    fn force_reload_picks_up_external_changes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");

        // Create and persist initial store
        let mut store = CronStore::load(path.clone()).unwrap();
        store.add_job(make_test_job("Original"));
        store.persist().unwrap();
        assert_eq!(store.job_count(), 1);

        // Externally modify the file
        let external_file = CronStoreFile {
            version: 1,
            jobs: vec![make_test_job("External A"), make_test_job("External B")],
        };
        let data = serde_json::to_string_pretty(&external_file).unwrap();
        fs::write(&path, data).unwrap();

        // force_reload should pick up the external changes
        store.force_reload().unwrap();
        assert_eq!(store.job_count(), 2);
        assert_eq!(store.jobs()[0].name, "External A");
        assert_eq!(store.jobs()[1].name, "External B");
    }
}
