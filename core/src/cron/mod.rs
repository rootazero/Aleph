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

pub use config::{CronConfig, CronJob, JobRun, JobStatus, ScheduleKind, TriggerSource};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{oneshot, RwLock, Semaphore};

#[cfg(feature = "cron")]
use cron::Schedule;
#[cfg(feature = "cron")]
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
            // Ignore "duplicate column name" errors
            let _ = conn.execute_batch(&sql);
        }

        let run_columns = [
            ("retry_count", "INTEGER DEFAULT 0"),
            ("trigger_source", "TEXT DEFAULT 'schedule'"),
            ("delivery_status", "TEXT"),
            ("delivery_error", "TEXT"),
        ];

        for (col, col_type) in &run_columns {
            let sql = format!("ALTER TABLE cron_runs ADD COLUMN {} {}", col, col_type);
            let _ = conn.execute_batch(&sql);
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

    /// Add a new job
    pub async fn add_job(&self, job: CronJob) -> CronResult<String> {
        // Validate schedule
        #[cfg(feature = "cron")]
        {
            Schedule::from_str(&job.schedule)
                .map_err(|e| CronError::InvalidSchedule(format!("{}", e)))?;
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
        // Validate schedule
        #[cfg(feature = "cron")]
        {
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

        tokio::task::spawn_blocking(move || {
            let conn = Connection::open(&db_path)?;
            let now = Utc::now().timestamp();
            let rows = conn.execute(
                "UPDATE cron_jobs SET enabled = 1, updated_at = ?2 WHERE id = ?1",
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

    /// Get next run time for a job
    #[cfg(feature = "cron")]
    pub fn get_next_run(&self, job: &CronJob) -> Option<DateTime<Utc>> {
        Schedule::from_str(&job.schedule)
            .ok()
            .and_then(|schedule| schedule.upcoming(Utc).next())
    }

    #[cfg(not(feature = "cron"))]
    pub fn get_next_run(&self, _job: &CronJob) -> Option<DateTime<Utc>> {
        None
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

        tracing::info!(
            "Starting cron service (check interval: {}s, max concurrent: {})",
            check_interval,
            self.config.max_concurrent_jobs
        );

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(check_interval));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Check for due jobs
                        if let Err(e) = Self::check_and_run_jobs(
                            db_path.clone(),
                            executor.clone(),
                            running_jobs.clone(),
                            semaphore.clone(),
                            job_timeout,
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

    /// Check for due jobs and run them
    #[cfg(feature = "cron")]
    async fn check_and_run_jobs(
        db_path: PathBuf,
        executor: JobExecutor,
        running_jobs: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
        semaphore: Arc<Semaphore>,
        job_timeout: u64,
    ) -> CronResult<()> {
        let now = Utc::now();

        // Get enabled jobs (in blocking context)
        let jobs = {
            let db_path = db_path.clone();
            tokio::task::spawn_blocking(move || {
                let conn = Connection::open(&db_path)?;
                let sql = format!(
                    "SELECT {} FROM cron_jobs WHERE enabled = 1",
                    Self::JOBS_SELECT
                );
                let mut stmt = conn.prepare(&sql)?;

                let jobs: Vec<CronJob> = stmt
                    .query_map([], Self::row_to_cron_job)?
                    .collect::<Result<Vec<_>, _>>()?;

                Ok::<_, CronError>(jobs)
            })
            .await
            .map_err(|e| CronError::Internal(format!("Task join error: {}", e)))??
        };

        for job in jobs {
            // Parse schedule
            let schedule = match Schedule::from_str(&job.schedule) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Invalid schedule for job {}: {}", job.id, e);
                    continue;
                }
            };

            // Check if job should run now (within the check interval)
            let should_run = schedule
                .upcoming(Utc)
                .take(1)
                .any(|next| {
                    let diff = (next - now).num_seconds().abs();
                    diff < 60 // Within 1 minute window
                });

            if !should_run {
                continue;
            }

            // Check if already running
            {
                let running = running_jobs.read().await;
                if running.contains_key(&job.id) {
                    tracing::debug!("Job {} is already running, skipping", job.id);
                    continue;
                }
            }

            // Try to acquire semaphore
            let permit = match semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    tracing::debug!("Max concurrent jobs reached, deferring {}", job.id);
                    continue;
                }
            };

            tracing::info!("Running cron job: {} ({})", job.name, job.id);

            // Spawn job execution
            let job_id = job.id.clone();
            let job_id_for_track = job_id.clone();
            let job_name = job.name.clone();
            let agent_id = job.agent_id.clone();
            let prompt = job.prompt.clone();
            let executor = executor.clone();
            let db_path_for_task = db_path.clone();
            let running_jobs_for_task = running_jobs.clone();

            let handle = tokio::spawn(async move {
                let _permit = permit; // Hold permit until done
                let mut run = JobRun::new(&job_id);

                // Save initial run state
                {
                    let db_path = db_path_for_task.clone();
                    let run_clone = run.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        Self::save_run_sync(&db_path, &run_clone)
                    }).await;
                }

                // Execute with timeout
                let result = tokio::time::timeout(
                    tokio::time::Duration::from_secs(job_timeout),
                    executor(job_id.clone(), agent_id, prompt),
                )
                .await;

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

                // Update run state
                {
                    let db_path = db_path_for_task.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        Self::save_run_sync(&db_path, &run)
                    }).await;
                }

                // Remove from running jobs
                running_jobs_for_task.write().await.remove(&job_id);
            });

            // Track running job
            running_jobs.write().await.insert(job_id_for_track, handle);
        }

        Ok(())
    }

    #[cfg(not(feature = "cron"))]
    async fn check_and_run_jobs(
        _db_path: PathBuf,
        _executor: JobExecutor,
        _running_jobs: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
        _semaphore: Arc<Semaphore>,
        _job_timeout: u64,
    ) -> CronResult<()> {
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
}
