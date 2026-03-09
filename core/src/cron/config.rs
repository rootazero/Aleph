//! Cron Job Configuration
//!
//! Configuration types for the scheduled job system.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cron service configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    "~/.aleph/data/cron.db".to_string()
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

    /// Cron expression (e.g., "0 0 9 * * *" for 9am daily)
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

    // --- State-machine scheduling ---

    /// Pre-computed next run time (Unix seconds)
    #[serde(default)]
    pub next_run_at: Option<i64>,

    /// Timestamp when job started running (Unix seconds)
    #[serde(default)]
    pub running_at: Option<i64>,

    /// Timestamp of last completed run (Unix seconds)
    #[serde(default)]
    pub last_run_at: Option<i64>,

    // --- Resilience ---

    /// Number of consecutive failures
    #[serde(default)]
    pub consecutive_failures: u32,

    /// Maximum retries before disabling
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Job priority (1=highest, 10=lowest)
    #[serde(default = "default_priority")]
    pub priority: u32,

    // --- Schedule types ---

    /// Kind of schedule: cron, every, at
    #[serde(default)]
    pub schedule_kind: ScheduleKind,

    /// Interval in milliseconds (for ScheduleKind::Every)
    #[serde(default)]
    pub every_ms: Option<i64>,

    /// One-shot timestamp (for ScheduleKind::At)
    #[serde(default)]
    pub at_time: Option<i64>,

    /// Delete the job after a successful run (for one-shot jobs)
    #[serde(default)]
    pub delete_after_run: bool,

    // --- Job chaining ---

    /// Job ID to trigger on success
    #[serde(default)]
    pub next_job_id_on_success: Option<String>,

    /// Job ID to trigger on failure
    #[serde(default)]
    pub next_job_id_on_failure: Option<String>,

    // --- Delivery ---

    /// Delivery configuration for job results
    #[serde(default)]
    pub delivery_config: Option<DeliveryConfig>,

    // --- Dynamic prompt ---

    /// Template with {{var}} placeholders
    #[serde(default)]
    pub prompt_template: Option<String>,

    /// JSON-encoded context variables
    #[serde(default)]
    pub context_vars: Option<String>,

    // --- Optimistic locking ---

    /// Version for optimistic concurrency control
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_max_retries() -> u32 {
    3
}

fn default_priority() -> u32 {
    5
}

fn default_version() -> u32 {
    1
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
            // State-machine scheduling
            next_run_at: None,
            running_at: None,
            last_run_at: None,
            // Resilience
            consecutive_failures: 0,
            max_retries: default_max_retries(),
            priority: default_priority(),
            // Schedule types
            schedule_kind: ScheduleKind::default(),
            every_ms: None,
            at_time: None,
            delete_after_run: false,
            // Job chaining
            next_job_id_on_success: None,
            next_job_id_on_failure: None,
            // Delivery
            delivery_config: None,
            // Dynamic prompt
            prompt_template: None,
            context_vars: None,
            // Optimistic locking
            version: default_version(),
        }
    }

    /// Validate the cron expression
    pub fn validate_schedule(&self) -> Result<(), String> {
        use std::str::FromStr;
        cron::Schedule::from_str(&self.schedule)
            .map(|_| ())
            .map_err(|e| format!("Invalid cron expression '{}': {}", self.schedule, e))
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

// --- New types for extended scheduling ---

/// Kind of schedule expression
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleKind {
    #[default]
    Cron,
    Every,
    At,
}

impl ScheduleKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Cron => "cron",
            Self::Every => "every",
            Self::At => "at",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "every" => Self::Every,
            "at" => Self::At,
            _ => Self::Cron,
        }
    }
}

/// What triggered a job run
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerSource {
    #[default]
    Schedule,
    Chain,
    Manual,
    Catchup,
}

impl TriggerSource {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Schedule => "schedule",
            Self::Chain => "chain",
            Self::Manual => "manual",
            Self::Catchup => "catchup",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "chain" => Self::Chain,
            "manual" => Self::Manual,
            "catchup" => Self::Catchup,
            _ => Self::Schedule,
        }
    }
}

/// Configuration for delivering job results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryConfig {
    pub mode: DeliveryMode,
    pub targets: Vec<DeliveryTargetConfig>,
    #[serde(default)]
    pub fallback_target: Option<DeliveryTargetConfig>,
}

/// Delivery mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryMode {
    None,
    Primary,
    Broadcast,
}

/// Delivery target configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum DeliveryTargetConfig {
    Gateway {
        channel: String,
        chat_id: String,
        #[serde(default)]
        format: Option<String>,
    },
    Memory {
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        importance: Option<f32>,
    },
    Webhook {
        url: String,
        #[serde(default)]
        method: Option<String>,
        #[serde(default)]
        headers: Option<std::collections::HashMap<String, String>>,
    },
}

/// Outcome of a delivery attempt
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeliveryOutcome {
    pub target_kind: String,
    pub success: bool,
    pub message: Option<String>,
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

    /// Number of retry attempts for this run
    #[serde(default)]
    pub retry_count: u32,

    /// What triggered this run
    #[serde(default)]
    pub trigger_source: TriggerSource,

    /// JSON summary of delivery outcomes
    #[serde(default)]
    pub delivery_status: Option<String>,

    /// Delivery error message
    #[serde(default)]
    pub delivery_error: Option<String>,
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
            retry_count: 0,
            trigger_source: TriggerSource::default(),
            delivery_status: None,
            delivery_error: None,
        }
    }

    /// Set the trigger source
    pub fn with_trigger(mut self, source: TriggerSource) -> Self {
        self.trigger_source = source;
        self
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

/// Truncate string to max length (UTF-8 safe)
fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let target = max_len.saturating_sub(3);
        let boundary = s
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= target)
            .last()
            .unwrap_or(0);
        format!("{}...", &s[..boundary])
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
        let job = CronJob::new("Daily Report", "0 0 9 * * *", "main", "Generate daily report");
        assert_eq!(job.name, "Daily Report");
        assert_eq!(job.schedule, "0 0 9 * * *");
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

    #[test]
    fn test_schedule_kind_default() {
        assert_eq!(ScheduleKind::default(), ScheduleKind::Cron);
    }

    #[test]
    fn test_schedule_kind_roundtrip() {
        assert_eq!(ScheduleKind::parse("cron"), ScheduleKind::Cron);
        assert_eq!(ScheduleKind::parse("every"), ScheduleKind::Every);
        assert_eq!(ScheduleKind::parse("at"), ScheduleKind::At);
        assert_eq!(ScheduleKind::parse("invalid"), ScheduleKind::Cron);
    }

    #[test]
    fn test_delivery_config_serde() {
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Webhook {
                url: "https://x.com".into(),
                method: None,
                headers: None,
            }],
            fallback_target: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: DeliveryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.targets.len(), 1);
    }

    #[test]
    fn test_trigger_source_roundtrip() {
        for (s, v) in [
            ("schedule", TriggerSource::Schedule),
            ("chain", TriggerSource::Chain),
            ("manual", TriggerSource::Manual),
            ("catchup", TriggerSource::Catchup),
        ] {
            assert_eq!(TriggerSource::parse(s), v);
            assert_eq!(v.as_str(), s);
        }
    }

    #[test]
    fn test_cron_job_extended_defaults() {
        let job = CronJob::new("T", "0 0 * * * *", "main", "p");
        assert_eq!(job.schedule_kind, ScheduleKind::Cron);
        assert_eq!(job.priority, 5);
        assert_eq!(job.max_retries, 3);
        assert_eq!(job.consecutive_failures, 0);
        assert!(job.next_run_at.is_none());
        assert!(job.running_at.is_none());
        assert_eq!(job.version, 1);
    }
}
