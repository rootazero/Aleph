//! `SQLite` Store for Cron Jobs
//!
//! Persistent storage using `SQLite` with in-memory cache.
//! The `CronStore` maintains a `Vec<CronJob>` in memory for fast access,
//! and writes changes to `SQLite` on `persist()`. This keeps the same
//! API as the previous JSON store while gaining query capabilities
//! and consistency with other Aleph subsystems.
//!
//! The `CronStore` is designed to be wrapped in a `tokio::sync::Mutex`
//! by the service layer.

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use tracing::{info, warn};

use crate::tasks::cron::config::CronJob;
use crate::tasks::cron::history::{self, CronRunRecord};

/// Current schema version
const CURRENT_VERSION: u32 = 1;

// ── CronStore ────────────────────────────────────────────────────────

/// SQLite-backed cron job store with in-memory cache.
pub struct CronStore {
    conn: Connection,
    /// In-memory cache of all jobs (the authoritative working copy)
    jobs: Vec<CronJob>,
    /// Dirty flag: true when in-memory state differs from DB
    dirty: bool,
}

impl CronStore {
    /// Open (or create) the `SQLite` store at the given path.
    ///
    /// Creates the schema if needed, runs migrations, and loads all
    /// jobs into memory.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create store directory: {e}"))?;
        }

        let conn = crate::utils::sqlite_open::open_sqlite_safe(&path)
            .map_err(|e| format!("failed to open cron DB at {}: {e}", path.display()))?;

        init_schema(&conn)?;
        migrate_schema(&conn)?;
        history::init_schema(&conn)?;

        // Load all jobs into memory
        let jobs = load_all_jobs(&conn)?;
        info!(count = jobs.len(), path = %path.display(), "Cron store loaded");

        Ok(Self {
            conn,
            jobs,
            dirty: false,
        })
    }

    /// Reload all jobs from the database, discarding in-memory changes.
    /// Returns true if the data actually changed.
    pub fn reload_if_changed(&mut self) -> Result<bool, String> {
        // SQLite is always authoritative; reload unconditionally
        self.force_reload()?;
        Ok(true)
    }

    /// Always reload from database, discarding in-memory state.
    pub fn force_reload(&mut self) -> Result<(), String> {
        self.jobs = load_all_jobs(&self.conn)?;
        self.dirty = false;
        Ok(())
    }

    /// Persist in-memory changes to the database.
    ///
    /// Uses a transaction to atomically replace all rows.
    /// Only writes when the dirty flag is set.
    pub fn persist(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }

        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("failed to begin transaction: {e}"))?;

        // Delete all existing rows and re-insert from memory.
        // For a small number of jobs (< 1000) this is simpler and safer
        // than tracking individual row changes.
        tx.execute("DELETE FROM cron_jobs", [])
            .map_err(|e| format!("failed to clear jobs: {e}"))?;

        for job in &self.jobs {
            let json = serde_json::to_string(job)
                .map_err(|e| format!("failed to serialize job '{}': {e}", job.id))?;
            tx.execute(
                "INSERT INTO cron_jobs (id, name, agent_id, enabled, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![job.id, job.name, job.agent_id, job.enabled, json],
            ).map_err(|e| format!("failed to insert job '{}': {e}", job.id))?;
        }

        tx.commit()
            .map_err(|e| format!("failed to commit transaction: {e}"))?;

        self.dirty = false;
        Ok(())
    }

    /// Mark the store as dirty (needs persistence).
    pub const fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    // ── Job accessors ──────────────────────────────────────────────────

    /// Get an immutable slice of all jobs.
    pub fn jobs(&self) -> &[CronJob] {
        &self.jobs
    }

    /// Get a mutable reference to the jobs vec. Auto-marks dirty.
    pub const fn jobs_mut(&mut self) -> &mut Vec<CronJob> {
        self.dirty = true;
        &mut self.jobs
    }

    /// Find a job by ID.
    pub fn get_job(&self, id: &str) -> Option<&CronJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Find a job by ID (mutable). Auto-marks dirty.
    pub fn get_job_mut(&mut self, id: &str) -> Option<&mut CronJob> {
        self.dirty = true;
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// Add a job. Marks dirty.
    pub fn add_job(&mut self, job: CronJob) {
        self.jobs.push(job);
        self.dirty = true;
    }

    /// Remove a job by ID. Returns the removed job if found. Marks dirty.
    pub fn remove_job(&mut self, id: &str) -> Option<CronJob> {
        let pos = self.jobs.iter().position(|j| j.id == id)?;
        self.dirty = true;
        Some(self.jobs.remove(pos))
    }

    /// Number of jobs in the store.
    pub const fn job_count(&self) -> usize {
        self.jobs.len()
    }

    // ── History ───────────────────────────────────────────────────────

    /// Insert a cron run record into the history table.
    pub fn insert_run(&self, record: &CronRunRecord) -> Result<(), String> {
        history::insert_cron_run(&self.conn, record)
    }

    /// Get execution history for a specific job.
    pub fn get_runs(&self, job_id: &str, limit: usize) -> Result<Vec<CronRunRecord>, String> {
        history::get_cron_runs(&self.conn, job_id, limit)
    }

    /// Cleanup old run records beyond retention period.
    pub fn cleanup_old_runs(&self, retention_days: u32, now_ms: i64) -> Result<u64, String> {
        history::cleanup_old_cron_runs(&self.conn, retention_days, now_ms)
    }
}

// ── Schema management ────────────────────────────────────────────────

/// Create the `cron_jobs` table if it doesn't exist.
fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cron_jobs (
            id      TEXT PRIMARY KEY,
            name    TEXT NOT NULL,
            agent_id TEXT NOT NULL DEFAULT 'main',
            enabled INTEGER NOT NULL DEFAULT 1,
            data    TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS cron_meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
    .map_err(|e| format!("failed to create cron schema: {e}"))?;

    // Set initial version if not present
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM cron_meta WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("failed to read schema version: {e}"))?;

    if version.is_none() {
        // `INSERT OR IGNORE`: idempotent on the version PRIMARY KEY so a
        // concurrent first-run init can't fail on a duplicate-key error.
        conn.execute(
            "INSERT OR IGNORE INTO cron_meta (key, value) VALUES ('version', ?1)",
            params![CURRENT_VERSION.to_string()],
        )
        .map_err(|e| format!("failed to set schema version: {e}"))?;
    }

    Ok(())
}

/// Run schema migrations.
fn migrate_schema(conn: &Connection) -> Result<(), String> {
    let version_str: String = conn
        .query_row(
            "SELECT value FROM cron_meta WHERE key = 'version'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("failed to read schema version: {e}"))?;

    let version: u32 = version_str.parse().map_err(|e| {
        format!(
            "schema version '{version_str}' is not a valid number: {e}. \
             Manual intervention may be required to repair the cron database."
        )
    })?;

    if version < CURRENT_VERSION {
        // Future migrations go here
        conn.execute(
            "UPDATE cron_meta SET value = ?1 WHERE key = 'version'",
            params![CURRENT_VERSION.to_string()],
        )
        .map_err(|e| format!("failed to update schema version: {e}"))?;
        info!(from = version, to = CURRENT_VERSION, "Cron schema migrated");
    }

    Ok(())
}

/// Load all jobs from the database.
fn load_all_jobs(conn: &Connection) -> Result<Vec<CronJob>, String> {
    let mut stmt = conn
        .prepare("SELECT data FROM cron_jobs ORDER BY rowid")
        .map_err(|e| format!("failed to prepare query: {e}"))?;

    let jobs: Vec<CronJob> = stmt
        .query_map([], |row| {
            let json: String = row.get(0)?;
            Ok(json)
        })
        .map_err(|e| format!("failed to query jobs: {e}"))?
        .filter_map(|r| match r {
            Ok(json) => match serde_json::from_str::<CronJob>(&json) {
                Ok(job) => Some(job),
                Err(e) => {
                    warn!(error = %e, "Skipping corrupt cron job row");
                    None
                }
            },
            Err(e) => {
                warn!(error = %e, "Failed to read cron job row");
                None
            }
        })
        .collect();

    Ok(jobs)
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::cron::config::ScheduleKind;
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
        let path = dir.path().join("cron.db");

        let store = CronStore::load(path).unwrap();
        assert_eq!(store.job_count(), 0);
    }

    #[test]
    fn add_persist_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.db");

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
        let path = dir.path().join("cron.db");

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
        let path = dir.path().join("cron.db");

        let mut store = CronStore::load(path).unwrap();
        // Not dirty, persist should be a no-op (returns Ok)
        store.persist().unwrap();
    }

    #[test]
    fn force_reload_from_db() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.db");

        let mut store = CronStore::load(path).unwrap();
        store.add_job(make_test_job("Original"));
        store.persist().unwrap();
        assert_eq!(store.job_count(), 1);

        // Add in-memory but don't persist
        store.add_job(make_test_job("Ephemeral"));
        assert_eq!(store.job_count(), 2);

        // force_reload should discard unpersisted changes
        store.force_reload().unwrap();
        assert_eq!(store.job_count(), 1);
        assert_eq!(store.jobs()[0].name, "Original");
    }
}
