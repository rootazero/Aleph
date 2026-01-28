//! Cron Job Configuration
//!
//! Configuration types for the scheduled job system.

use serde::{Deserialize, Serialize};

/// Cron service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronConfig {
    /// Whether the cron service is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Database path for job persistence
    #[serde(default = "default_db_path")]
    pub db_path: String,

    /// Check interval in seconds (how often to check for due jobs)
    #[serde(default = "default_check_interval")]
    pub check_interval_secs: u64,

    /// Maximum concurrent job executions
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_jobs: usize,

    /// Job execution timeout in seconds
    #[serde(default = "default_job_timeout")]
    pub job_timeout_secs: u64,

    /// Retain job history for this many days
    #[serde(default = "default_history_retention")]
    pub history_retention_days: u32,
}

fn default_true() -> bool {
    true
}

fn default_db_path() -> String {
    "~/.aether/cron.db".to_string()
}

fn default_check_interval() -> u64 {
    60 // 1 minute
}

fn default_max_concurrent() -> usize {
    5
}

fn default_job_timeout() -> u64 {
    300 // 5 minutes
}

fn default_history_retention() -> u32 {
    30 // 30 days
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            db_path: default_db_path(),
            check_interval_secs: 60,
            max_concurrent_jobs: 5,
            job_timeout_secs: 300,
            history_retention_days: 30,
        }
    }
}

impl CronConfig {
    /// Expand the database path (resolve ~)
    pub fn expand_db_path(&self) -> String {
        if self.db_path.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home
                    .join(&self.db_path[2..])
                    .to_string_lossy()
                    .to_string();
            }
        }
        self.db_path.clone()
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.check_interval_secs == 0 {
            return Err("check_interval_secs must be > 0".to_string());
        }
        if self.max_concurrent_jobs == 0 {
            return Err("max_concurrent_jobs must be > 0".to_string());
        }
        if self.job_timeout_secs == 0 {
            return Err("job_timeout_secs must be > 0".to_string());
        }
        Ok(())
    }
}

/// A scheduled job definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique job identifier
    pub id: String,

    /// Human-readable job name
    pub name: String,

    /// Cron expression (e.g., "0 9 * * *" for 9am daily)
    pub schedule: String,

    /// Agent ID to invoke
    pub agent_id: String,

    /// Message to send to the agent
    pub prompt: String,

    /// Whether the job is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Optional timezone (defaults to local)
    #[serde(default)]
    pub timezone: Option<String>,

    /// Tags for organization
    #[serde(default)]
    pub tags: Vec<String>,

    /// Created timestamp (Unix seconds)
    #[serde(default)]
    pub created_at: i64,

    /// Last modified timestamp (Unix seconds)
    #[serde(default)]
    pub updated_at: i64,
}

impl CronJob {
    /// Create a new cron job
    pub fn new(
        name: impl Into<String>,
        schedule: impl Into<String>,
        agent_id: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            schedule: schedule.into(),
            agent_id: agent_id.into(),
            prompt: prompt.into(),
            enabled: true,
            timezone: None,
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Validate the cron expression
    #[cfg(feature = "cron")]
    pub fn validate_schedule(&self) -> Result<(), String> {
        use std::str::FromStr;
        cron::Schedule::from_str(&self.schedule)
            .map(|_| ())
            .map_err(|e| format!("Invalid cron expression '{}': {}", self.schedule, e))
    }

    #[cfg(not(feature = "cron"))]
    pub fn validate_schedule(&self) -> Result<(), String> {
        // Basic validation without cron crate
        if self.schedule.is_empty() {
            return Err("Schedule cannot be empty".to_string());
        }
        Ok(())
    }
}

/// Job execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    /// Job is pending (never run)
    Pending,
    /// Job is currently running
    Running,
    /// Job completed successfully
    Success,
    /// Job failed
    Failed,
    /// Job was skipped (previous run still in progress)
    Skipped,
    /// Job timed out
    Timeout,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "pending"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Success => write!(f, "success"),
            JobStatus::Failed => write!(f, "failed"),
            JobStatus::Skipped => write!(f, "skipped"),
            JobStatus::Timeout => write!(f, "timeout"),
        }
    }
}

/// Job execution record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRun {
    /// Run identifier
    pub id: String,

    /// Job ID this run belongs to
    pub job_id: String,

    /// Run status
    pub status: JobStatus,

    /// Start timestamp (Unix seconds)
    pub started_at: i64,

    /// End timestamp (Unix seconds, 0 if still running)
    pub ended_at: i64,

    /// Duration in milliseconds
    pub duration_ms: u64,

    /// Error message if failed
    pub error: Option<String>,

    /// Agent response (truncated)
    pub response: Option<String>,
}

impl JobRun {
    /// Create a new job run
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_id: job_id.into(),
            status: JobStatus::Running,
            started_at: chrono::Utc::now().timestamp(),
            ended_at: 0,
            duration_ms: 0,
            error: None,
            response: None,
        }
    }

    /// Mark as success
    pub fn success(mut self, response: Option<String>) -> Self {
        let now = chrono::Utc::now().timestamp();
        self.status = JobStatus::Success;
        self.ended_at = now;
        self.duration_ms = ((now - self.started_at) * 1000) as u64;
        self.response = response.map(|r| truncate_string(&r, 1000));
        self
    }

    /// Mark as failed
    pub fn failed(mut self, error: String) -> Self {
        let now = chrono::Utc::now().timestamp();
        self.status = JobStatus::Failed;
        self.ended_at = now;
        self.duration_ms = ((now - self.started_at) * 1000) as u64;
        self.error = Some(truncate_string(&error, 500));
        self
    }

    /// Mark as timeout
    pub fn timeout(mut self) -> Self {
        let now = chrono::Utc::now().timestamp();
        self.status = JobStatus::Timeout;
        self.ended_at = now;
        self.duration_ms = ((now - self.started_at) * 1000) as u64;
        self.error = Some("Job execution timed out".to_string());
        self
    }
}

/// Truncate string to max length
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_config_default() {
        let config = CronConfig::default();
        assert!(config.enabled);
        assert_eq!(config.check_interval_secs, 60);
        assert_eq!(config.max_concurrent_jobs, 5);
    }

    #[test]
    fn test_cron_config_validate() {
        let mut config = CronConfig::default();
        assert!(config.validate().is_ok());

        config.check_interval_secs = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_cron_job_new() {
        let job = CronJob::new("Daily Report", "0 9 * * *", "main", "Generate daily report");
        assert_eq!(job.name, "Daily Report");
        assert_eq!(job.schedule, "0 9 * * *");
        assert!(job.enabled);
    }

    #[test]
    fn test_job_run_lifecycle() {
        let run = JobRun::new("job-1");
        assert_eq!(run.status, JobStatus::Running);
        assert!(run.ended_at == 0);

        let run = run.success(Some("Done!".to_string()));
        assert_eq!(run.status, JobStatus::Success);
        assert!(run.ended_at > 0);

        let run2 = JobRun::new("job-2").failed("Error occurred".to_string());
        assert_eq!(run2.status, JobStatus::Failed);
        assert!(run2.error.is_some());
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world!", 8), "hello...");
    }
}
