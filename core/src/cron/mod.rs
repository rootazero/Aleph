//! Cron Job Scheduling Service
//!
//! Provides scheduled job execution for automating agent tasks.
//!
//! # Features
//!
//! - Standard cron expressions (5-field: min hour day month weekday)
//! - Job persistence in SQLite
//! - Concurrent job execution limits
//! - Job history and run logs
//! - Enable/disable jobs without deletion
//!
//! # Cron Expression Format
//!
//! ```text
//! ┌───────────── minute (0 - 59)
//! │ ┌───────────── hour (0 - 23)
//! │ │ ┌───────────── day of month (1 - 31)
//! │ │ │ ┌───────────── month (1 - 12)
//! │ │ │ │ ┌───────────── day of week (0 - 6, Sunday = 0)
//! │ │ │ │ │
//! * * * * *
//! ```
//!
//! # Examples
//!
//! - `0 9 * * *` - Every day at 9:00 AM
//! - `*/15 * * * *` - Every 15 minutes
//! - `0 0 * * 0` - Every Sunday at midnight
//! - `0 9 1 * *` - First day of every month at 9:00 AM
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::cron::{CronService, CronConfig, CronJob};
//!
//! let config = CronConfig::default();
//! let mut service = CronService::new(config)?;
//!
//! // Add a job
//! let job = CronJob::new(
//!     "Daily Summary",
//!     "0 18 * * *",
//!     "main",
//!     "Summarize today's activities"
//! );
//! service.add_job(job).await?;
//!
//! // Start the scheduler
//! service.start().await?;
//! ```

pub mod config;
pub mod chain;
pub mod delivery;
pub mod resource;
pub mod scheduler;
pub mod template;
pub mod webhook_target;

pub use config::{
    CronConfig, CronJob, DeliveryConfig, DeliveryMode, DeliveryOutcome,
    DeliveryTargetConfig, JobRun, JobStatus, ScheduleKind, TriggerSource,
};
pub use delivery::{DeliveryEngine, DeliveryTarget};
pub use scheduler::{compute_backoff_ms, compute_next_run_at};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::sync_primitives::Arc;
use tokio::sync::{oneshot, RwLock, Semaphore};

use cron::Schedule;
use std::str::FromStr;

/// Result type for cron operations
pub type CronResult<T> = Result<T, CronError>;

/// Errors that can occur in cron operations
#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Job not found: {0}")]
    NotFound(String),

    #[error("Invalid cron expression: {0}")]
    InvalidSchedule(String),

    #[error("Job already exists: {0}")]
    AlreadyExists(String),

    #[error("Service not running")]
    NotRunning,

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Callback for job execution
pub type JobExecutor = Arc<
    dyn Fn(String, String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;

/// Cron service for scheduled job execution
pub struct CronService {
    /// Configuration
    config: CronConfig,
    /// Database path (connections created per-operation)
    db_path: PathBuf,
    /// Job executor callback
    executor: Option<JobExecutor>,
    /// Shutdown signal
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Currently running jobs
    running_jobs: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// Semaphore for concurrent job limits
    semaphore: Arc<Semaphore>,
}

impl CronService {
    /// Create a new cron service
    pub fn new(config: CronConfig) -> CronResult<Self> {
        config.validate().map_err(CronError::Internal)?;

        // Expand and create database path
        let db_path_str = config.expand_db_path();
        let db_path = PathBuf::from(&db_path_str);

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CronError::Internal(format!("Failed to create db directory: {}", e)))?;
        }

        // Initialize database schema
        {
            let conn = Connection::open(&db_path)?;
            Self::init_schema(&conn)?;
            Self::migrate_schema(&conn)?;
        }

        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_jobs));

        Ok(Self {
            config,
            db_path,
            executor: None,
            shutdown_tx: None,
            running_jobs: Arc::new(RwLock::new(HashMap::new())),
            semaphore,
        })
    }

    /// Initialize database schema
    fn init_schema(conn: &Connection) -> CronResult<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS cron_jobs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                schedule TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                prompt TEXT NOT NULL,
                enabled INTEGER DEFAULT 1,
                timezone TEXT,
                tags TEXT DEFAULT '[]',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                -- State-machine scheduling
                next_run_at INTEGER,
                running_at INTEGER,
                last_run_at INTEGER,
                -- Resilience
                consecutive_failures INTEGER DEFAULT 0,
                max_retries INTEGER DEFAULT 3,
                priority INTEGER DEFAULT 5,
                -- Schedule types
                schedule_kind TEXT DEFAULT 'cron',
                every_ms INTEGER,
                at_time INTEGER,
                delete_after_run INTEGER DEFAULT 0,
                -- Job chaining
                next_job_id_on_success TEXT,
                next_job_id_on_failure TEXT,
                -- Delivery
                delivery_config TEXT,
                -- Dynamic prompt
                prompt_template TEXT,
                context_vars TEXT,
                -- Optimistic locking
                version INTEGER DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS cron_runs (
                id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                status TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                ended_at INTEGER DEFAULT 0,
                duration_ms INTEGER DEFAULT 0,
                error TEXT,
                response TEXT,
                -- Extended fields
                retry_count INTEGER DEFAULT 0,
                trigger_source TEXT DEFAULT 'schedule',
                delivery_status TEXT,
                delivery_error TEXT,
                FOREIGN KEY (job_id) REFERENCES cron_jobs(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_runs_job_id ON cron_runs(job_id);
            CREATE INDEX IF NOT EXISTS idx_runs_started_at ON cron_runs(started_at);
            CREATE INDEX IF NOT EXISTS idx_jobs_next_run ON cron_jobs(next_run_at) WHERE enabled = 1;
            CREATE INDEX IF NOT EXISTS idx_jobs_running ON cron_jobs(running_at);
            "#,
        )?;
        Ok(())
    }

    /// Migrate existing databases: add new columns if they don't exist.
    /// Each ALTER TABLE ADD COLUMN will fail silently if the column already exists.
    fn migrate_schema(conn: &Connection) -> CronResult<()> {
        let job_columns = [
            ("next_run_at", "INTEGER"),
            ("running_at", "INTEGER"),
            ("last_run_at", "INTEGER"),
            ("consecutive_failures", "INTEGER DEFAULT 0"),
            ("max_retries", "INTEGER DEFAULT 3"),
            ("priority", "INTEGER DEFAULT 5"),
            ("schedule_kind", "TEXT DEFAULT 'cron'"),
            ("every_ms", "INTEGER"),
            ("at_time", "INTEGER"),
            ("delete_after_run", "INTEGER DEFAULT 0"),
            ("next_job_id_on_success", "TEXT"),
            ("next_job_id_on_failure", "TEXT"),
            ("delivery_config", "TEXT"),
            ("prompt_template", "TEXT"),
            ("context_vars", "TEXT"),
            ("version", "INTEGER DEFAULT 1"),
        ];

        for (col, col_type) in &job_columns {
            let sql = format!("ALTER TABLE cron_jobs ADD COLUMN {} {}", col, col_type);
            // Only suppress "duplicate column name" errors; propagate real failures
            if let Err(e) = conn.execute_batch(&sql) {
                let msg = e.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(CronError::Database(e));
                }
            }
        }

        let run_columns = [
            ("retry_count", "INTEGER DEFAULT 0"),
            ("trigger_source", "TEXT DEFAULT 'schedule'"),
            ("delivery_status", "TEXT"),
            ("delivery_error", "TEXT"),
        ];

        for (col, col_type) in &run_columns {
            let sql = format!("ALTER TABLE cron_runs ADD COLUMN {} {}", col, col_type);
            if let Err(e) = conn.execute_batch(&sql) {
                let msg = e.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(CronError::Database(e));
                }
            }
        }

        // Create new indexes (IF NOT EXISTS handles idempotency)
        let _ = conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_jobs_next_run ON cron_jobs(next_run_at) WHERE enabled = 1;
             CREATE INDEX IF NOT EXISTS idx_jobs_running ON cron_jobs(running_at);",
        );

        Ok(())
    }

    /// Set the job executor callback
    pub fn set_executor(&mut self, executor: JobExecutor) {
        self.executor = Some(executor);
    }

    /// Get a reference to the executor (for manual job triggering via RPC)
    pub fn executor_ref(&self) -> Option<&JobExecutor> {
        self.executor.as_ref()
    }

    /// Add a new job
    pub async fn add_job(&self, job: CronJob) -> CronResult<String> {
        // Validate schedule (only for Cron kind)
        if job.schedule_kind == ScheduleKind::Cron {
            Schedule::from_str(&job.schedule)
                .map_err(|e| CronError::InvalidSchedule(format!("{}", e)))?;
        }

        // Compute initial next_run_at
        let mut job = job;
        if job.next_run_at.is_none() && job.enabled {
            job.next_run_at = scheduler::compute_next_run_at(&job, Utc::now());
        }

        let db_path = self.db_path.clone();
        let job_id = job.id.clone();
        let job_name = job.name.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let tags_json = serde_json::to_string(&job.tags).unwrap_or_else(|_| "[]".to_string());
            let delivery_json = job.delivery_config.as_ref().and_then(|d| serde_json::to_string(d).ok());

            conn.execute(
                r#"
                INSERT INTO cron_jobs (
                    id, name, schedule, agent_id, prompt, enabled, timezone, tags,
                    created_at, updated_at,
                    next_run_at, running_at, last_run_at,
                    consecutive_failures, max_retries, priority,
                    schedule_kind, every_ms, at_time, delete_after_run,
                    next_job_id_on_success, next_job_id_on_failure,
                    delivery_config, prompt_template, context_vars, version
                )
                VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    ?9, ?10,
                    ?11, ?12, ?13,
                    ?14, ?15, ?16,
                    ?17, ?18, ?19, ?20,
                    ?21, ?22,
                    ?23, ?24, ?25, ?26
                )
                "#,
                params![
                    job.id,
                    job.name,
                    job.schedule,
                    job.agent_id,
                    job.prompt,
                    job.enabled as i32,
                    job.timezone,
                    tags_json,
                    job.created_at,
                    job.updated_at,
                    job.next_run_at,
                    job.running_at,
                    job.last_run_at,
                    job.consecutive_failures,
                    job.max_retries,
                    job.priority,
                    job.schedule_kind.as_str(),
                    job.every_ms,
                    job.at_time,
                    job.delete_after_run as i32,
                    job.next_job_id_on_success,
                    job.next_job_id_on_failure,
                    delivery_json,
                    job.prompt_template,
                    job.context_vars,
                    job.version,
                ],
            ).map_err(|e| {
                if e.to_string().contains("UNIQUE constraint failed") {
                    CronError::AlreadyExists(job.id.clone())
                } else {
                    CronError::Database(e)
                }
            })?;

            Ok::<_, CronError>(())
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??;

        tracing::info!("Added cron job: {} ({})", job_name, job_id);
        Ok(job_id)
    }

    /// Update an existing job
    pub async fn update_job(&self, job: CronJob) -> CronResult<()> {
        // Validate schedule (only for Cron kind)
        if job.schedule_kind == ScheduleKind::Cron {
            Schedule::from_str(&job.schedule)
                .map_err(|e| CronError::InvalidSchedule(format!("{}", e)))?;
        }

        let db_path = self.db_path.clone();
        let job_id = job.id.clone();
        let job_name = job.name.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let tags_json = serde_json::to_string(&job.tags).unwrap_or_else(|_| "[]".to_string());
            let now = Utc::now().timestamp();
            let delivery_json = job.delivery_config.as_ref().and_then(|d| serde_json::to_string(d).ok());

            let rows = conn.execute(
                r#"
                UPDATE cron_jobs
                SET name = ?2, schedule = ?3, agent_id = ?4, prompt = ?5,
                    enabled = ?6, timezone = ?7, tags = ?8, updated_at = ?9,
                    next_run_at = ?10, running_at = ?11, last_run_at = ?12,
                    consecutive_failures = ?13, max_retries = ?14, priority = ?15,
                    schedule_kind = ?16, every_ms = ?17, at_time = ?18, delete_after_run = ?19,
                    next_job_id_on_success = ?20, next_job_id_on_failure = ?21,
                    delivery_config = ?22, prompt_template = ?23, context_vars = ?24, version = ?25
                WHERE id = ?1
                "#,
                params![
                    job.id,
                    job.name,
                    job.schedule,
                    job.agent_id,
                    job.prompt,
                    job.enabled as i32,
                    job.timezone,
                    tags_json,
                    now,
                    job.next_run_at,
                    job.running_at,
                    job.last_run_at,
                    job.consecutive_failures,
                    job.max_retries,
                    job.priority,
                    job.schedule_kind.as_str(),
                    job.every_ms,
                    job.at_time,
                    job.delete_after_run as i32,
                    job.next_job_id_on_success,
                    job.next_job_id_on_failure,
                    delivery_json,
                    job.prompt_template,
                    job.context_vars,
                    job.version,
                ],
            )?;

            if rows == 0 {
                return Err(CronError::NotFound(job.id));
            }

            Ok::<_, CronError>(())
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??;

        tracing::info!("Updated cron job: {} ({})", job_name, job_id);
        Ok(())
    }

    /// Delete a job
    pub async fn delete_job(&self, job_id: &str) -> CronResult<()> {
        let db_path = self.db_path.clone();
        let job_id = job_id.to_string();
        let job_id_for_log = job_id.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let rows = conn.execute("DELETE FROM cron_jobs WHERE id = ?1", params![job_id])?;

            if rows == 0 {
                return Err(CronError::NotFound(job_id));
            }

            Ok::<_, CronError>(())
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??;

        tracing::info!("Deleted cron job: {}", job_id_for_log);
        Ok(())
    }

    /// Enable a job
    pub async fn enable_job(&self, job_id: &str) -> CronResult<()> {
        let db_path = self.db_path.clone();
        let job_id = job_id.to_string();
        let jobs_select = Self::JOBS_SELECT;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let now = Utc::now();

            // Enable the job
            let rows = conn.execute(
                "UPDATE cron_jobs SET enabled = 1, updated_at = ?2 WHERE id = ?1",
                params![job_id, now.timestamp()],
            )?;

            if rows == 0 {
                return Err(CronError::NotFound(job_id));
            }

            // Recompute next_run_at for fresh scheduling
            let sql = format!("SELECT {} FROM cron_jobs WHERE id = ?1", jobs_select);
            let mut stmt = conn.prepare(&sql)?;
            if let Ok(job) = stmt.query_row(params![job_id], CronService::row_to_cron_job) {
                let next = scheduler::compute_next_run_at(&job, now);
                conn.execute(
                    "UPDATE cron_jobs SET next_run_at = ?1 WHERE id = ?2",
                    params![next, job.id],
                )?;
            }

            Ok::<_, CronError>(())
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??;

        Ok(())
    }

    /// Disable a job
    pub async fn disable_job(&self, job_id: &str) -> CronResult<()> {
        let db_path = self.db_path.clone();
        let job_id = job_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let now = Utc::now().timestamp();
            let rows = conn.execute(
                "UPDATE cron_jobs SET enabled = 0, updated_at = ?2 WHERE id = ?1",
                params![job_id, now],
            )?;

            if rows == 0 {
                return Err(CronError::NotFound(job_id));
            }

            Ok::<_, CronError>(())
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??;

        Ok(())
    }

    /// Helper: map a row to CronJob. Columns must be selected in the standard order.
    fn row_to_cron_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<CronJob> {
        let tags_json: String = row.get(7)?;
        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

        let schedule_kind_str: String = row.get::<_, Option<String>>(16)?.unwrap_or_else(|| "cron".to_string());
        let delivery_json: Option<String> = row.get(22)?;
        let delivery_config = delivery_json.and_then(|j| serde_json::from_str(&j).ok());

        Ok(CronJob {
            id: row.get(0)?,
            name: row.get(1)?,
            schedule: row.get(2)?,
            agent_id: row.get(3)?,
            prompt: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            timezone: row.get(6)?,
            tags,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
            next_run_at: row.get(10)?,
            running_at: row.get(11)?,
            last_run_at: row.get(12)?,
            consecutive_failures: row.get::<_, Option<u32>>(13)?.unwrap_or(0),
            max_retries: row.get::<_, Option<u32>>(14)?.unwrap_or(3),
            priority: row.get::<_, Option<u32>>(15)?.unwrap_or(5),
            schedule_kind: ScheduleKind::from_str(&schedule_kind_str),
            every_ms: row.get(17)?,
            at_time: row.get(18)?,
            delete_after_run: row.get::<_, Option<i32>>(19)?.unwrap_or(0) != 0,
            next_job_id_on_success: row.get(20)?,
            next_job_id_on_failure: row.get(21)?,
            delivery_config,
            prompt_template: row.get(23)?,
            context_vars: row.get(24)?,
            version: row.get::<_, Option<u32>>(25)?.unwrap_or(1),
        })
    }

    /// The standard SELECT columns for cron_jobs
    const JOBS_SELECT: &'static str =
        "id, name, schedule, agent_id, prompt, enabled, timezone, tags, \
         created_at, updated_at, \
         next_run_at, running_at, last_run_at, \
         consecutive_failures, max_retries, priority, \
         schedule_kind, every_ms, at_time, delete_after_run, \
         next_job_id_on_success, next_job_id_on_failure, \
         delivery_config, prompt_template, context_vars, version";

    /// Get a job by ID
    pub async fn get_job(&self, job_id: &str) -> CronResult<CronJob> {
        let db_path = self.db_path.clone();
        let job_id = job_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let sql = format!(
                "SELECT {} FROM cron_jobs WHERE id = ?1",
                Self::JOBS_SELECT
            );
            let mut stmt = conn.prepare(&sql)?;

            let job = stmt
                .query_row(params![job_id], Self::row_to_cron_job)
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => CronError::NotFound(job_id.clone()),
                    _ => CronError::Database(e),
                })?;

            Ok::<_, CronError>(job)
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))?
    }

    /// List all jobs
    pub async fn list_jobs(&self) -> CronResult<Vec<CronJob>> {
        let db_path = self.db_path.clone();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let sql = format!(
                "SELECT {} FROM cron_jobs ORDER BY created_at DESC",
                Self::JOBS_SELECT
            );
            let mut stmt = conn.prepare(&sql)?;

            let jobs: Vec<CronJob> = stmt
                .query_map([], Self::row_to_cron_job)?
                .collect::<Result<Vec<_>, _>>()?;

            Ok::<_, CronError>(jobs)
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))?
    }

    /// Helper: map a row to JobRun
    fn row_to_job_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRun> {
        let status_str: String = row.get(2)?;
        let status = match status_str.as_str() {
            "pending" => JobStatus::Pending,
            "running" => JobStatus::Running,
            "success" => JobStatus::Success,
            "failed" => JobStatus::Failed,
            "skipped" => JobStatus::Skipped,
            "timeout" => JobStatus::Timeout,
            _ => JobStatus::Failed,
        };
        let trigger_str: String = row.get::<_, Option<String>>(9)?.unwrap_or_else(|| "schedule".to_string());

        Ok(JobRun {
            id: row.get(0)?,
            job_id: row.get(1)?,
            status,
            started_at: row.get(3)?,
            ended_at: row.get(4)?,
            duration_ms: row.get(5)?,
            error: row.get(6)?,
            response: row.get(7)?,
            retry_count: row.get::<_, Option<u32>>(8)?.unwrap_or(0),
            trigger_source: TriggerSource::from_str(&trigger_str),
            delivery_status: row.get(10)?,
            delivery_error: row.get(11)?,
        })
    }

    /// The standard SELECT columns for cron_runs
    const RUNS_SELECT: &'static str =
        "id, job_id, status, started_at, ended_at, duration_ms, error, response, \
         retry_count, trigger_source, delivery_status, delivery_error";

    /// Get job run history
    pub async fn get_job_runs(&self, job_id: &str, limit: usize) -> CronResult<Vec<JobRun>> {
        let db_path = self.db_path.clone();
        let job_id = job_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let sql = format!(
                "SELECT {} FROM cron_runs WHERE job_id = ?1 ORDER BY started_at DESC LIMIT ?2",
                Self::RUNS_SELECT
            );
            let mut stmt = conn.prepare(&sql)?;

            let runs: Vec<JobRun> = stmt
                .query_map(params![job_id, limit as i64], Self::row_to_job_run)?
                .collect::<Result<Vec<_>, _>>()?;

            Ok::<_, CronError>(runs)
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))?
    }

    /// Save a job run (blocking, for use within spawn_blocking)
    fn save_run_sync(db_path: &Path, run: &JobRun) -> CronResult<()> {
        let conn = Connection::open(db_path)?;
        conn.execute(
            r#"
            INSERT OR REPLACE INTO cron_runs (
                id, job_id, status, started_at, ended_at, duration_ms, error, response,
                retry_count, trigger_source, delivery_status, delivery_error
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![
                run.id,
                run.job_id,
                run.status.to_string(),
                run.started_at,
                run.ended_at,
                run.duration_ms as i64,
                run.error,
                run.response,
                run.retry_count,
                run.trigger_source.as_str(),
                run.delivery_status,
                run.delivery_error,
            ],
        )?;
        Ok(())
    }

    /// Finalize a job after execution: update state, handle backoff, trigger chains.
    fn finalize_job_sync(db_path: &Path, job_id: &str, job: &CronJob, success: bool) -> CronResult<()> {
        let conn = Connection::open(db_path)?;
        let now = Utc::now();
        let now_ms = now.timestamp_millis();

        if success {
            // Reset failures, compute next run time
            let next_run = scheduler::compute_next_run_at(job, now);
            conn.execute(
                "UPDATE cron_jobs SET running_at = NULL, last_run_at = ?1, consecutive_failures = 0, next_run_at = ?2 WHERE id = ?3",
                params![now_ms, next_run, job_id],
            )?;

            // Trigger on_success chain
            if let Some(ref next_id) = job.next_job_id_on_success {
                let _ = chain::trigger_chain_job_sync(&conn, next_id, now_ms);
            }

            // Delete completed one-shot if configured
            if job.delete_after_run && job.schedule_kind == ScheduleKind::At {
                conn.execute("DELETE FROM cron_jobs WHERE id = ?1", params![job_id])?;
            }
        } else {
            let failures = job.consecutive_failures + 1;

            if failures <= job.max_retries {
                // Retry with backoff
                let backoff = scheduler::compute_backoff_ms(failures);
                let next_run = Some(now_ms + backoff as i64);
                conn.execute(
                    "UPDATE cron_jobs SET running_at = NULL, last_run_at = ?1, consecutive_failures = ?2, next_run_at = ?3 WHERE id = ?4",
                    params![now_ms, failures, next_run, job_id],
                )?;
            } else {
                // Max retries exceeded: disable the job and trigger failure chain.
                // Keep consecutive_failures at current value (not reset to 0) to prevent
                // infinite retry cycles where the counter resets after exhausting retries.
                let next_run = scheduler::compute_next_run_at(job, now);
                conn.execute(
                    "UPDATE cron_jobs SET running_at = NULL, last_run_at = ?1, consecutive_failures = ?2, next_run_at = ?3, enabled = 0 WHERE id = ?4",
                    params![now_ms, failures, next_run, job_id],
                )?;

                if let Some(ref next_id) = job.next_job_id_on_failure {
                    let _ = chain::trigger_chain_job_sync(&conn, next_id, now_ms);
                }
            }
        }

        Ok(())
    }

    /// Get next run time for a job
    pub fn get_next_run(&self, job: &CronJob) -> Option<DateTime<Utc>> {
        Schedule::from_str(&job.schedule)
            .ok()
            .and_then(|schedule| schedule.upcoming(Utc).next())
    }

    /// Start the cron scheduler
    pub async fn start(&mut self) -> CronResult<()> {
        if !self.config.enabled {
            tracing::info!("Cron service is disabled");
            return Ok(());
        }

        if self.executor.is_none() {
            return Err(CronError::Internal("No executor set".to_string()));
        }

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        let db_path = self.db_path.clone();
        let executor = self.executor.clone().unwrap();
        let running_jobs = self.running_jobs.clone();
        let semaphore = self.semaphore.clone();
        let check_interval = self.config.check_interval_secs;
        let job_timeout = self.config.job_timeout_secs;
        let max_concurrent = self.config.max_concurrent_jobs;

        tracing::info!(
            "Starting cron service (check interval: {}s, max concurrent: {})",
            check_interval,
            max_concurrent
        );

        // Run startup catch-up before entering main loop
        if let Err(e) = Self::startup_catchup(&self.db_path).await {
            tracing::error!("Startup catch-up failed: {}", e);
        }

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(check_interval));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = Self::check_and_run_jobs(
                            db_path.clone(),
                            executor.clone(),
                            running_jobs.clone(),
                            semaphore.clone(),
                            job_timeout,
                            max_concurrent,
                        ).await {
                            tracing::error!("Error checking cron jobs: {}", e);
                        }
                    }
                    _ = &mut shutdown_rx => {
                        tracing::info!("Cron service shutdown requested");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Stop the cron scheduler
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Wait for running jobs to complete
        let jobs = self.running_jobs.write().await;
        for (job_id, handle) in jobs.iter() {
            tracing::info!("Waiting for job {} to complete...", job_id);
            handle.abort();
        }
    }

    /// Scheduler tick: acquire due jobs and execute them.
    ///
    /// Uses state-machine approach: jobs with `next_run_at <= now` and `running_at IS NULL`
    /// are atomically acquired via BEGIN IMMEDIATE transaction.
    async fn check_and_run_jobs(
        db_path: PathBuf,
        executor: JobExecutor,
        running_jobs: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
        semaphore: Arc<Semaphore>,
        job_timeout: u64,
        max_concurrent: usize,
    ) -> CronResult<()> {
        let now_ms = Utc::now().timestamp_millis();

        // Phase 1: Clear stuck jobs (running for > STUCK_THRESHOLD)
        {
            let db = db_path.clone();
            let stuck_cutoff = now_ms - scheduler::STUCK_THRESHOLD_MS;
            tokio::task::spawn_blocking(move || {
                let conn = Connection::open(&db)?;
                let cleared = conn.execute(
                    "UPDATE cron_jobs SET running_at = NULL WHERE running_at IS NOT NULL AND running_at < ?1",
                    params![stuck_cutoff],
                )?;
                if cleared > 0 {
                    tracing::warn!("Cleared {} stuck cron jobs", cleared);
                }
                Ok::<_, CronError>(())
            })
            .await
            .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??;
        }

        // Phase 2: Determine effective concurrency based on CPU load
        // Runs in spawn_blocking because get_cpu_usage() sleeps 200ms for sysinfo
        let available_permits = semaphore.available_permits();
        let effective_max = tokio::task::spawn_blocking(move || {
            resource::resolve_effective_concurrency(max_concurrent, available_permits)
        })
        .await
        .unwrap_or(max_concurrent);

        // Phase 3: Atomic acquire due jobs
        let jobs_select = Self::JOBS_SELECT;
        let acquired = {
            let db = db_path.clone();
            tokio::task::spawn_blocking(move || {
                let conn = Connection::open(&db)?;
                conn.execute_batch("BEGIN IMMEDIATE")?;

                let sql = format!(
                    "SELECT {} FROM cron_jobs WHERE next_run_at <= ?1 AND running_at IS NULL AND enabled = 1 ORDER BY priority ASC LIMIT ?2",
                    jobs_select
                );
                let mut stmt = conn.prepare(&sql)?;
                let jobs: Vec<CronJob> = stmt
                    .query_map(params![now_ms, effective_max as i64], Self::row_to_cron_job)?
                    .collect::<Result<Vec<_>, _>>()?;

                // Mark acquired jobs as running
                for job in &jobs {
                    conn.execute(
                        "UPDATE cron_jobs SET running_at = ?1 WHERE id = ?2",
                        params![now_ms, job.id],
                    )?;
                }

                conn.execute_batch("COMMIT")?;
                Ok::<_, CronError>(jobs)
            })
            .await
            .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??
        };

        // Phase 4: Spawn execution for each acquired job
        for job in acquired {
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    // Release the running_at marker since we can't run it now
                    let db = db_path.clone();
                    let jid = job.id.clone();
                    let _ = tokio::task::spawn_blocking(move || -> Option<()> {
                        let conn = Connection::open(&db).ok()?;
                        conn.execute(
                            "UPDATE cron_jobs SET running_at = NULL WHERE id = ?1",
                            params![jid],
                        ).ok()?;
                        Some(())
                    }).await;
                    continue;
                }
            };

            tracing::info!("Running cron job: {} ({})", job.name, job.id);

            let job_id = job.id.clone();
            let job_id_for_track = job_id.clone();
            let job_name = job.name.clone();
            let agent_id = job.agent_id.clone();

            // Resolve prompt: use template if available, otherwise use raw prompt
            let runs_select = Self::RUNS_SELECT;
            let prompt = if let Some(ref tpl) = job.prompt_template {
                let db = db_path.clone();
                let jid = job.id.clone();
                let tpl = tpl.clone();
                let job_for_tpl = job.clone();
                let last_run = tokio::task::spawn_blocking(move || -> Option<JobRun> {
                    let conn = Connection::open(&db).ok()?;
                    let sql = format!(
                        "SELECT {} FROM cron_runs WHERE job_id = ?1 ORDER BY started_at DESC LIMIT 1",
                        runs_select
                    );
                    let mut stmt = conn.prepare(&sql).ok()?;
                    stmt.query_row(params![jid], Self::row_to_job_run).ok()
                })
                .await
                .unwrap_or(None);
                template::render_template(&tpl, &job_for_tpl, last_run.as_ref(), 0)
            } else {
                job.prompt.clone()
            };

            let executor = executor.clone();
            let db_path_for_task = db_path.clone();
            let running_jobs_for_task = running_jobs.clone();
            let job_clone = job.clone();

            let handle = tokio::spawn(async move {
                let _permit = permit;
                let mut run = JobRun::new(&job_id);

                // Save initial run state
                {
                    let db = db_path_for_task.clone();
                    let run_clone = run.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        Self::save_run_sync(&db, &run_clone)
                    }).await;
                }

                // Execute with timeout
                let result = tokio::time::timeout(
                    tokio::time::Duration::from_secs(job_timeout),
                    executor(job_id.clone(), agent_id, prompt),
                )
                .await;

                let success = matches!(&result, Ok(Ok(_)));
                run = match result {
                    Ok(Ok(response)) => {
                        tracing::info!("Job {} completed successfully", job_name);
                        run.success(Some(response))
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Job {} failed: {}", job_name, e);
                        run.failed(e)
                    }
                    Err(_) => {
                        tracing::error!("Job {} timed out", job_name);
                        run.timeout()
                    }
                };

                // Save final run state
                {
                    let db = db_path_for_task.clone();
                    let run_clone = run.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        Self::save_run_sync(&db, &run_clone)
                    }).await;
                }

                // Finalize: update job state (next_run_at, failures, chains)
                {
                    let db = db_path_for_task.clone();
                    let jid = job_id.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        Self::finalize_job_sync(&db, &jid, &job_clone, success)
                    }).await;
                }

                // Remove from running jobs map
                running_jobs_for_task.write().await.remove(&job_id);
            });

            running_jobs.write().await.insert(job_id_for_track, handle);
        }

        Ok(())
    }

    /// Cleanup old run history
    pub async fn cleanup_history(&self) -> CronResult<u64> {
        let db_path = self.db_path.clone();
        let retention_days = self.config.history_retention_days;

        let rows = tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let cutoff = Utc::now().timestamp() - (retention_days as i64 * 86400);

            let rows = conn.execute(
                "DELETE FROM cron_runs WHERE started_at < ?1",
                params![cutoff],
            )?;

            Ok::<_, CronError>(rows as u64)
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??;

        if rows > 0 {
            tracing::info!("Cleaned up {} old cron run records", rows);
        }

        Ok(rows)
    }

    /// Run startup catch-up: clear stale markers and recompute schedules.
    async fn startup_catchup(db_path: &Path) -> CronResult<()> {
        let db_path = db_path.to_path_buf();
        let jobs_select = Self::JOBS_SELECT;

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let now = Utc::now();
            let now_ms = now.timestamp_millis();

            // Phase 1: Clear all running_at markers (stale from previous shutdown)
            let cleared = conn.execute(
                "UPDATE cron_jobs SET running_at = NULL WHERE running_at IS NOT NULL",
                [],
            )?;
            if cleared > 0 {
                tracing::info!("Startup: cleared {} stale running markers", cleared);
            }

            // Phase 2: Recompute next_run_at for all enabled jobs
            let sql = format!(
                "SELECT {} FROM cron_jobs WHERE enabled = 1",
                jobs_select
            );
            let mut stmt = conn.prepare(&sql)?;
            let jobs: Vec<CronJob> = stmt
                .query_map([], CronService::row_to_cron_job)?
                .collect::<Result<Vec<_>, _>>()?;

            let mut catchup_count = 0;
            for job in &jobs {
                if scheduler::is_completed_oneshot(job) {
                    continue;
                }

                let next = scheduler::compute_next_run_at(job, now);

                // If next_run_at is in the past (missed), set it to now for immediate catchup
                let effective_next = match next {
                    Some(t) if t < now_ms => Some(now_ms),
                    other => other,
                };

                conn.execute(
                    "UPDATE cron_jobs SET next_run_at = ?1 WHERE id = ?2",
                    params![effective_next, job.id],
                )?;
                catchup_count += 1;
            }

            tracing::info!("Startup: recomputed next_run_at for {} jobs", catchup_count);
            Ok::<_, CronError>(())
        })
        .await
        .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_config() -> CronConfig {
        let dir = tempdir().unwrap();
        CronConfig {
            db_path: dir.path().join("test_cron.db").to_string_lossy().to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_add_and_get_job() {
        let config = test_config();
        let service = CronService::new(config).unwrap();

        let job = CronJob::new("Test Job", "0 0 * * * *", "main", "Test prompt");
        let job_id = job.id.clone();

        service.add_job(job).await.unwrap();

        let retrieved = service.get_job(&job_id).await.unwrap();
        assert_eq!(retrieved.name, "Test Job");
        assert_eq!(retrieved.schedule, "0 0 * * * *");
    }

    #[tokio::test]
    async fn test_list_jobs() {
        let config = test_config();
        let service = CronService::new(config).unwrap();

        let job1 = CronJob::new("Job 1", "0 0 * * * *", "main", "Prompt 1");
        let job2 = CronJob::new("Job 2", "0 30 * * * *", "main", "Prompt 2");

        service.add_job(job1).await.unwrap();
        service.add_job(job2).await.unwrap();

        let jobs = service.list_jobs().await.unwrap();
        assert_eq!(jobs.len(), 2);
    }

    #[tokio::test]
    async fn test_enable_disable_job() {
        let config = test_config();
        let service = CronService::new(config).unwrap();

        let job = CronJob::new("Test Job", "0 0 * * * *", "main", "Test");
        let job_id = job.id.clone();
        service.add_job(job).await.unwrap();

        // Disable
        service.disable_job(&job_id).await.unwrap();
        let job = service.get_job(&job_id).await.unwrap();
        assert!(!job.enabled);

        // Enable
        service.enable_job(&job_id).await.unwrap();
        let job = service.get_job(&job_id).await.unwrap();
        assert!(job.enabled);
    }

    #[tokio::test]
    async fn test_delete_job() {
        let config = test_config();
        let service = CronService::new(config).unwrap();

        let job = CronJob::new("Test Job", "0 0 * * * *", "main", "Test");
        let job_id = job.id.clone();
        service.add_job(job).await.unwrap();

        service.delete_job(&job_id).await.unwrap();

        let result = service.get_job(&job_id).await;
        assert!(matches!(result, Err(CronError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_schema_migration_new_fields() {
        let config = test_config();
        let service = CronService::new(config).unwrap();

        let mut job = CronJob::new("Migration Test", "0 0 * * * *", "main", "test");
        job.priority = 1;
        job.schedule_kind = ScheduleKind::Every;
        job.every_ms = Some(60_000);
        let job_id = job.id.clone();

        service.add_job(job).await.unwrap();

        let ret = service.get_job(&job_id).await.unwrap();
        assert_eq!(ret.priority, 1);
        assert_eq!(ret.schedule_kind, ScheduleKind::Every);
        assert_eq!(ret.every_ms, Some(60_000));
        assert_eq!(ret.version, 1);
    }

    #[tokio::test]
    async fn test_scheduler_startup_computes_next_run() {
        let config = test_config();
        let service = CronService::new(config).unwrap();

        let job = CronJob::new("Hourly Job", "0 0 * * * *", "main", "Do work");
        let job_id = job.id.clone();
        service.add_job(job).await.unwrap();

        let retrieved = service.get_job(&job_id).await.unwrap();
        assert!(retrieved.next_run_at.is_some(), "next_run_at should be set after add");
        assert!(retrieved.next_run_at.unwrap() > chrono::Utc::now().timestamp_millis());
    }

    #[tokio::test]
    async fn test_add_every_job() {
        let config = test_config();
        let service = CronService::new(config).unwrap();

        let mut job = CronJob::new("Interval Job", "unused", "main", "Do work");
        job.schedule_kind = ScheduleKind::Every;
        job.every_ms = Some(60_000);

        let job_id = job.id.clone();
        service.add_job(job).await.unwrap();

        let retrieved = service.get_job(&job_id).await.unwrap();
        assert!(retrieved.next_run_at.is_some());
        assert_eq!(retrieved.schedule_kind, ScheduleKind::Every);
    }

    #[tokio::test]
    async fn test_full_job_lifecycle() {
        use crate::cron::config::DeliveryConfig;

        let config = test_config();
        let service = CronService::new(config).unwrap();

        // 1. Add a job with delivery config
        let mut job = CronJob::new("Lifecycle Test", "0 0 * * * *", "main", "Do work");
        job.priority = 2;
        job.max_retries = 5;
        job.delivery_config = Some(DeliveryConfig {
            mode: crate::cron::config::DeliveryMode::None,
            targets: vec![],
            fallback_target: None,
        });
        let job_id = job.id.clone();
        service.add_job(job).await.unwrap();

        // 2. Verify next_run_at computed and delivery config stored
        let ret = service.get_job(&job_id).await.unwrap();
        assert!(ret.next_run_at.is_some());
        assert_eq!(ret.priority, 2);
        assert_eq!(ret.max_retries, 5);
        assert!(ret.delivery_config.is_some());

        // 3. Update the job
        let mut updated = ret;
        updated.prompt = "Updated prompt".to_string();
        service.update_job(updated).await.unwrap();
        let ret = service.get_job(&job_id).await.unwrap();
        assert_eq!(ret.prompt, "Updated prompt");

        // 4. Verify template rendering works
        let rendered = crate::cron::template::render_template(
            "Job {{job_name}} run #{{run_count}}",
            &ret,
            None,
            42,
        );
        assert_eq!(rendered, "Job Lifecycle Test run #42");

        // 5. Verify chain cycle detection (no chain configured, so no cycle)
        let conn = rusqlite::Connection::open(&service.db_path).unwrap();
        let has_cycle = crate::cron::chain::detect_cycle_sync(&conn, &job_id, "nonexistent").unwrap();
        assert!(!has_cycle);

        // 6. Disable and re-enable
        service.disable_job(&job_id).await.unwrap();
        let ret = service.get_job(&job_id).await.unwrap();
        assert!(!ret.enabled);

        service.enable_job(&job_id).await.unwrap();
        let ret = service.get_job(&job_id).await.unwrap();
        assert!(ret.enabled);

        // 7. Delete
        service.delete_job(&job_id).await.unwrap();
        assert!(matches!(service.get_job(&job_id).await, Err(CronError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_add_oneshot_job() {
        let config = test_config();
        let service = CronService::new(config).unwrap();

        let mut job = CronJob::new("One-Shot", "unused", "main", "Do once");
        job.schedule_kind = ScheduleKind::At;
        job.at_time = Some(Utc::now().timestamp_millis() + 3_600_000);
        job.delete_after_run = true;

        let job_id = job.id.clone();
        service.add_job(job).await.unwrap();

        let ret = service.get_job(&job_id).await.unwrap();
        assert_eq!(ret.schedule_kind, ScheduleKind::At);
        assert!(ret.next_run_at.is_some());
        assert!(ret.delete_after_run);
    }
}
