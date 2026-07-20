//! Cron Job Configuration
//!
//! Configuration types for the scheduled job system.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── CronConfig ──────────────────────────────────────────────────────────

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

    /// Job execution timeout in seconds
    #[serde(default = "default_job_timeout")]
    pub job_timeout_secs: u64,

    /// Retain job history for this many days
    #[serde(default = "default_history_retention")]
    pub history_retention_days: u32,

    /// Maximum concurrent agent sessions spawned by cron
    #[serde(default = "default_max_concurrent_agents")]
    pub max_concurrent_agents: Option<usize>,

    /// How many missed jobs to catch up on restart
    #[serde(default)]
    pub max_missed_jobs_per_restart: Option<usize>,

    /// Stagger interval (ms) between catchup jobs
    #[serde(default)]
    pub catchup_stagger_ms: Option<i64>,

    /// D4: when `true`, jobs that lack an explicit `failure_alert` config but
    /// do have a `source_channel_id` + `source_conversation_id` get a
    /// default alert (after=1, cooldown=1h, target=source channel) so
    /// timeouts / errors don't fail silently in the channel that triggered
    /// the job. Set to `false` to keep the legacy opt-in-only behaviour.
    #[serde(default = "default_true")]
    pub notify_on_failure_default: bool,

    /// D2: cron-default Think→Act iteration cap. Applies to every cron-driven
    /// run as a `RunRequest.max_iterations_override`, overriding the global
    /// `[execution] max_iterations` default (1000) for system-initiated jobs
    /// without affecting interactive sessions. `None` disables the cron
    /// cap and falls through to the global default. Default `Some(200)` —
    /// raised from `Some(40)` in 2026-05-27 because 40 was still too tight
    /// for real-world briefing tasks: a single web-research run that has to
    /// (a) climb the fallback ladder (`search` → `web_fetch` canonical →
    /// `web_fetch` alternate → browser tooling) AND (b) cross-reference
    /// 3-5 sources AND (c) assemble a structured report can easily burn
    /// 60-120 tool calls before producing useful output. `200` keeps a hard
    /// guardrail for runaway loops while honouring the project doctrine
    /// that Aleph is a 24/7 background daemon — scheduled jobs should
    /// silently persevere in the background rather than fold early.
    #[serde(default = "default_cron_max_iterations")]
    pub default_max_iterations: Option<u32>,
}

const fn default_true() -> bool {
    true
}

fn default_db_path() -> String {
    "~/.aleph/data/tasks.db".to_string()
}

const fn default_check_interval() -> u64 {
    60 // 1 minute
}

const fn default_job_timeout() -> u64 {
    // 15 minutes. Coupled to `default_cron_max_iterations = Some(200)`:
    // a 200-iter run averaging ~3s/turn lands around 10 min, so 15 min
    // gives ~50% headroom before wall-clock kills the job. Raised from
    // 300s (5 min) in 2026-05-27 alongside the iter-cap bump — 5 min
    // was the choke point that fired BEFORE the iter-cap during the
    // fallback-ladder + multi-source-synthesis runs that the iter-cap
    // bump targets. Per-job overrides via `CronJob.timeout_ms` remain
    // the escape hatch for either tighter (ping/heartbeat jobs) or
    // looser (deep research) budgets.
    900
}

const fn default_history_retention() -> u32 {
    30 // 30 days
}

const fn default_max_concurrent_agents() -> Option<usize> {
    Some(2)
}

const fn default_cron_max_iterations() -> Option<u32> {
    Some(200)
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            db_path: default_db_path(),
            check_interval_secs: 60,
            job_timeout_secs: 900,
            history_retention_days: 30,
            max_concurrent_agents: Some(2),
            max_missed_jobs_per_restart: None,
            catchup_stagger_ms: None,
            notify_on_failure_default: true,
            default_max_iterations: default_cron_max_iterations(),
        }
    }
}

impl CronConfig {
    /// Expand the database path (resolve ~)
    #[must_use]
    pub fn expand_db_path(&self) -> String {
        if self.db_path.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(&self.db_path[2..]).to_string_lossy().to_string();
            }
        }
        self.db_path.clone()
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.check_interval_secs == 0 {
            return Err("check_interval_secs must be > 0".to_string());
        }
        if self.job_timeout_secs == 0 {
            return Err("job_timeout_secs must be > 0".to_string());
        }
        Ok(())
    }
}

// ── ScheduleKind ────────────────────────────────────────────────────────

/// Rich schedule kind: one-shot, interval, or cron expression
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleKind {
    /// One-shot: fire at a specific timestamp
    At {
        at: i64,
        #[serde(default)]
        delete_after_run: bool,
    },
    /// Interval: fire every N milliseconds
    Every {
        every_ms: i64,
        #[serde(default)]
        anchor_ms: Option<i64>,
    },
    /// Cron expression: standard 6-field cron
    Cron {
        expr: String,
        #[serde(default)]
        tz: Option<String>,
        #[serde(default)]
        stagger_ms: Option<i64>,
    },
}

impl Default for ScheduleKind {
    fn default() -> Self {
        Self::Cron {
            expr: "0 0 * * * *".to_string(),
            tz: None,
            stagger_ms: None,
        }
    }
}

// ── RunStatus ───────────────────────────────────────────────────────────

/// Outcome status of a completed job run
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    Error,
    Skipped,
    Timeout,
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => write!(f, "ok"),
            Self::Error => write!(f, "error"),
            Self::Skipped => write!(f, "skipped"),
            Self::Timeout => write!(f, "timeout"),
        }
    }
}

// ── ErrorReason ─────────────────────────────────────────────────────────

/// Categorized error reason for job failures
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ErrorReason {
    Transient(String),
    Permanent(String),
}

impl ErrorReason {
    /// Stable `snake_case` category token (`"transient"` / `"permanent"`).
    ///
    /// This is the wire form the `cron_runs.error_reason` column stores and
    /// that history consumers filter on — never the Rust `Debug` rendering
    /// (`Transient("…")`), which would make rows unqueryable by category.
    #[must_use]
    pub const fn category(&self) -> &'static str {
        match self {
            Self::Transient(_) => "transient",
            Self::Permanent(_) => "permanent",
        }
    }
}

// ── SessionTarget ───────────────────────────────────────────────────────

/// Where the cron job agent session runs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionTarget {
    Main,
    #[default]
    Isolated,
}

// ── TriggerSource ───────────────────────────────────────────────────────

/// What triggered a job run
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TriggerSource {
    #[default]
    Schedule,
    Chain,
    Manual,
    Catchup,
}

impl TriggerSource {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Schedule => "schedule",
            Self::Chain => "chain",
            Self::Manual => "manual",
            Self::Catchup => "catchup",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "chain" => Self::Chain,
            "manual" => Self::Manual,
            "catchup" => Self::Catchup,
            _ => Self::Schedule,
        }
    }
}

// ── FailureAlertConfig ──────────────────────────────────────────────────

const fn default_alert_after() -> u32 {
    2
}

const fn default_alert_cooldown() -> i64 {
    3_600_000 // 1 hour in ms
}

/// Configuration for alerting on repeated job failures
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FailureAlertConfig {
    /// Alert after this many consecutive failures
    #[serde(default = "default_alert_after")]
    pub after: u32,

    /// Cooldown between alerts in milliseconds
    #[serde(default = "default_alert_cooldown")]
    pub cooldown_ms: i64,

    /// Where to send the alert
    pub target: DeliveryTargetConfig,
}

// ── JobStateV2 ──────────────────────────────────────────────────────────

/// Runtime state for a cron job (persisted alongside the job definition)
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct JobStateV2 {
    pub next_run_at_ms: Option<i64>,
    pub running_at_ms: Option<i64>,
    pub last_run_at_ms: Option<i64>,
    pub last_run_status: Option<RunStatus>,
    pub last_error: Option<String>,
    pub last_error_reason: Option<ErrorReason>,
    pub last_duration_ms: Option<i64>,
    #[serde(default)]
    pub consecutive_errors: u32,
    pub last_failure_alert_at_ms: Option<i64>,
    pub last_delivery_status: Option<DeliveryStatus>,
    /// Output text of the previous run — feeds the `{{last_output}}`
    /// template variable so a job can chain off its own last result.
    #[serde(default)]
    pub last_output: Option<String>,
    /// Total number of completed runs — feeds the `{{run_count}}` variable.
    #[serde(default)]
    pub run_count: u64,
}

// ── CronJob ─────────────────────────────────────────────────────────────

/// A scheduled job definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique job identifier
    pub id: String,

    /// Human-readable job name
    pub name: String,

    /// Agent ID to invoke
    pub agent_id: String,

    /// Channel where this cron job was created (for result delivery)
    #[serde(default)]
    pub source_channel_id: Option<String>,

    /// Conversation ID within the channel (e.g. Telegram `chat_id`) for delivery routing
    #[serde(default)]
    pub source_conversation_id: Option<String>,

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

    /// Created timestamp (ms since epoch)
    #[serde(default)]
    pub created_at: i64,

    /// Last modified timestamp (ms since epoch)
    #[serde(default)]
    pub updated_at: i64,

    /// Schedule definition
    pub schedule_kind: ScheduleKind,

    /// Where the agent session runs
    #[serde(default)]
    pub session_target: SessionTarget,

    /// Job ID to trigger on success
    #[serde(default)]
    pub next_job_id_on_success: Option<String>,

    /// Job ID to trigger on failure
    #[serde(default)]
    pub next_job_id_on_failure: Option<String>,

    /// Delivery configuration for job results
    #[serde(default)]
    pub delivery_config: Option<DeliveryConfig>,

    /// Failure alerting configuration
    #[serde(default)]
    pub failure_alert: Option<FailureAlertConfig>,

    /// Template with {{var}} placeholders
    #[serde(default)]
    pub prompt_template: Option<String>,

    /// JSON-encoded context variables
    #[serde(default)]
    pub context_vars: Option<String>,

    /// Per-job execution timeout override (in milliseconds).
    ///
    /// When `Some`, this overrides `CronConfig::job_timeout_secs` for this
    /// specific job. The override is threaded into `JobSnapshot.timeout_ms`
    /// by `phase1_mark_due_jobs` and reaches the executor's wall-clock
    /// `timeout_secs`. Use this for jobs whose normal completion window
    /// exceeds the global default (e.g. long-running research tasks).
    ///
    /// `None` falls back to the configured default.
    #[serde(default)]
    pub timeout_ms: Option<i64>,

    /// Runtime state
    #[serde(default)]
    pub state: JobStateV2,
}

impl CronJob {
    /// Create a new cron job with sensible defaults
    pub fn new(
        name: impl Into<String>,
        agent_id: impl Into<String>,
        prompt: impl Into<String>,
        schedule_kind: ScheduleKind,
    ) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            agent_id: agent_id.into(),
            source_channel_id: None,
            source_conversation_id: None,
            prompt: prompt.into(),
            enabled: true,
            timezone: None,
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            schedule_kind,
            session_target: SessionTarget::default(),
            next_job_id_on_success: None,
            next_job_id_on_failure: None,
            delivery_config: None,
            failure_alert: None,
            prompt_template: None,
            context_vars: None,
            timeout_ms: None,
            state: JobStateV2::default(),
        }
    }

    /// Effective per-job execution timeout in milliseconds.
    ///
    /// Returns this job's `timeout_ms` override when set, else falls back to
    /// the 15-minute baseline (matches `CronConfig::job_timeout_secs`'s default).
    /// Used by the catchup stale-marker threshold; runtime execution gets the
    /// timeout via `JobSnapshot.timeout_ms` threaded by `phase1_mark_due_jobs`,
    /// which honours the per-job override against the *configured* default.
    #[must_use]
    pub fn effective_timeout_ms(&self) -> i64 {
        self.timeout_ms.unwrap_or(900_000)
    }

    /// Construct a generic `DeliveryPayload` from this job and its output.
    ///
    /// Used when handing off results to the shared delivery pipeline.
    #[must_use]
    #[allow(dead_code)]
    fn to_delivery_payload(
        &self,
        output: String,
    ) -> crate::tasks::shared::delivery::DeliveryPayload {
        crate::tasks::shared::delivery::DeliveryPayload {
            source_type: "cron".to_string(),
            task_name: self.name.clone(),
            agent_id: self.agent_id.clone(),
            output,
            channel_id: self.source_channel_id.clone(),
            metadata: serde_json::Value::Null,
        }
    }
}

// ── JobSnapshot ─────────────────────────────────────────────────────────

/// A frozen snapshot of a job ready for execution
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub id: String,
    pub agent_id: Option<String>,
    /// Channel to deliver results to
    pub source_channel_id: Option<String>,
    /// Conversation ID within the channel (e.g. Telegram `chat_id`)
    pub source_conversation_id: Option<String>,
    /// Template-rendered prompt
    pub prompt: String,
    pub model: Option<String>,
    pub timeout_ms: Option<i64>,
    pub delivery: Option<DeliveryConfig>,
    pub session_target: SessionTarget,
    pub marked_at: i64,
    pub trigger_source: TriggerSource,
}

// ── ExecutionResult ─────────────────────────────────────────────────────

/// Result of executing a job snapshot
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: i64,
    pub status: RunStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub error_reason: Option<ErrorReason>,
    pub delivery_status: Option<DeliveryStatus>,
    pub agent_used_messaging_tool: bool,
    pub trigger_source: TriggerSource,
    /// Classification of `error` produced by [`crate::tasks::shared::retry_hint::classify`].
    /// `None` on success; `Some(RetryHint::permanent())` for a classified non-transient
    /// failure (the writeback path uses this to stop scheduling further retries and
    /// to bypass the failure-alert cooldown).
    pub retry_hint: Option<crate::tasks::shared::retry_hint::RetryHint>,
}

// ── CronJobView ─────────────────────────────────────────────────────────

/// Read-only view of a `CronJob` for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobView {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule_kind: ScheduleKind,
    pub agent_id: String,
    pub source_channel_id: Option<String>,
    pub prompt: String,
    pub timezone: Option<String>,
    pub tags: Vec<String>,
    pub session_target: SessionTarget,
    pub state: JobStateV2,
    pub delivery_config: Option<DeliveryConfig>,
    pub failure_alert: Option<FailureAlertConfig>,
    pub timeout_ms: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&CronJob> for CronJobView {
    fn from(job: &CronJob) -> Self {
        Self {
            id: job.id.clone(),
            name: job.name.clone(),
            enabled: job.enabled,
            schedule_kind: job.schedule_kind.clone(),
            agent_id: job.agent_id.clone(),
            source_channel_id: job.source_channel_id.clone(),
            prompt: job.prompt.clone(),
            timezone: job.timezone.clone(),
            tags: job.tags.clone(),
            session_target: job.session_target.clone(),
            state: job.state.clone(),
            delivery_config: job.delivery_config.clone(),
            failure_alert: job.failure_alert.clone(),
            timeout_ms: job.timeout_ms,
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

// ── Delivery types ──────────────────────────────────────────────────────
// Re-exported from shared::delivery to avoid breaking existing consumers.

pub use crate::tasks::shared::delivery::{
    DeliveryConfig, DeliveryMode, DeliveryOutcome, DeliveryStatus, DeliveryTargetConfig,
};

// ── JobRun ──────────────────────────────────────────────────────────────

/// Job execution record (kept for history)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRun {
    /// Run identifier
    pub id: String,

    /// Job ID this run belongs to
    pub job_id: String,

    /// Run status
    pub status: RunStatus,

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
    /// Create a new job run.
    /// All timestamps use milliseconds since epoch for consistency with the rest of the cron system.
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_id: job_id.into(),
            status: RunStatus::Ok, // Will be updated on completion
            started_at: chrono::Utc::now().timestamp_millis(),
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
    #[must_use]
    pub const fn with_trigger(mut self, source: TriggerSource) -> Self {
        self.trigger_source = source;
        self
    }

    /// Mark as success
    #[must_use]
    pub fn success(mut self, response: Option<String>) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        self.status = RunStatus::Ok;
        self.ended_at = now;
        self.duration_ms = now.saturating_sub(self.started_at) as u64;
        self.response = response.map(|r| truncate_string(&r, 1000));
        self
    }

    /// Mark as failed
    #[must_use]
    pub fn failed(mut self, error: String) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        self.status = RunStatus::Error;
        self.ended_at = now;
        self.duration_ms = now.saturating_sub(self.started_at) as u64;
        self.error = Some(truncate_string(&error, 500));
        self
    }

    /// Mark as timeout
    #[must_use]
    pub fn timeout(mut self) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        self.status = RunStatus::Timeout;
        self.ended_at = now;
        self.duration_ms = now.saturating_sub(self.started_at) as u64;
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
        format!("{}...", s.get(..boundary).unwrap_or(s))
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_config_default() {
        let config = CronConfig::default();
        assert!(config.enabled);
        assert_eq!(config.check_interval_secs, 60);
        assert_eq!(config.max_concurrent_agents, Some(2));
        // The 24/7-daemon perseverance contract: scheduled jobs get a
        // generous iter cap AND wall-clock budget so the prompt-driven
        // fallback ladder (search → web_fetch alt → browser) can actually
        // play out before the job is killed. Lowering either of these
        // below the values asserted here re-introduces the early-fold
        // bug fixed in 2026-05-27 — touch with intent.
        assert_eq!(
            config.default_max_iterations,
            Some(200),
            "cron iter cap is the 24/7-daemon perseverance contract"
        );
        assert_eq!(
            config.job_timeout_secs, 900,
            "cron wall-clock must outlive the iter cap"
        );
    }

    #[test]
    fn test_cron_config_validate() {
        let mut config = CronConfig::default();
        assert!(config.validate().is_ok());

        config.check_interval_secs = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn run_status_serde_roundtrip() {
        for status in [
            RunStatus::Ok,
            RunStatus::Error,
            RunStatus::Skipped,
            RunStatus::Timeout,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: RunStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
        // Verify snake_case
        assert_eq!(serde_json::to_string(&RunStatus::Ok).unwrap(), "\"ok\"");
        assert_eq!(
            serde_json::to_string(&RunStatus::Timeout).unwrap(),
            "\"timeout\""
        );
    }

    #[test]
    fn error_reason_category_is_snake_case() {
        // The history `error_reason` column stores this token, not the Debug
        // form (`Transient("…")`) — consumers filter on `"transient"`.
        assert_eq!(
            ErrorReason::Transient("x".to_string()).category(),
            "transient"
        );
        assert_eq!(
            ErrorReason::Permanent("y".to_string()).category(),
            "permanent"
        );
    }

    #[test]
    fn error_reason_serde_roundtrip() {
        let transient = ErrorReason::Transient("network timeout".to_string());
        let json = serde_json::to_string(&transient).unwrap();
        let parsed: ErrorReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, transient);

        let permanent = ErrorReason::Permanent("invalid config".to_string());
        let json = serde_json::to_string(&permanent).unwrap();
        let parsed: ErrorReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, permanent);

        // Verify tagged format
        let json = serde_json::to_string(&transient).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], "transient");
        assert_eq!(v["message"], "network timeout");
    }

    #[test]
    fn job_state_v2_defaults() {
        let state = JobStateV2::default();
        assert!(state.next_run_at_ms.is_none());
        assert!(state.running_at_ms.is_none());
        assert!(state.last_run_at_ms.is_none());
        assert!(state.last_run_status.is_none());
        assert!(state.last_error.is_none());
        assert!(state.last_error_reason.is_none());
        assert!(state.last_duration_ms.is_none());
        assert_eq!(state.consecutive_errors, 0);
        assert!(state.last_failure_alert_at_ms.is_none());
        assert!(state.last_delivery_status.is_none());
        assert!(state.last_output.is_none());
        assert_eq!(state.run_count, 0);
    }

    #[test]
    fn session_target_default_is_isolated() {
        assert_eq!(SessionTarget::default(), SessionTarget::Isolated);
    }

    #[test]
    fn failure_alert_config_defaults() {
        let json = r#"{"target":{"kind":"Webhook","url":"https://example.com"}}"#;
        let config: FailureAlertConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.after, 2);
        assert_eq!(config.cooldown_ms, 3_600_000);
        match &config.target {
            DeliveryTargetConfig::Webhook { url, .. } => {
                assert_eq!(url, "https://example.com");
            }
            _ => panic!("expected Webhook target"),
        }
    }

    #[test]
    fn schedule_kind_every_serde() {
        let kind = ScheduleKind::Every {
            every_ms: 60_000,
            anchor_ms: Some(1000),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: ScheduleKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);

        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], "every");
        assert_eq!(v["every_ms"], 60_000);
    }

    #[test]
    fn schedule_kind_cron_serde() {
        let kind = ScheduleKind::Cron {
            expr: "0 9 * * *".to_string(),
            tz: Some("Asia/Shanghai".to_string()),
            stagger_ms: None,
        };
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: ScheduleKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);

        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], "cron");
        assert_eq!(v["expr"], "0 9 * * *");
    }

    #[test]
    fn schedule_kind_at_serde() {
        let kind = ScheduleKind::At {
            at: 1700000000000,
            delete_after_run: true,
        };
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: ScheduleKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);

        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], "at");
        assert_eq!(v["at"], 1700000000000_i64);
        assert_eq!(v["delete_after_run"], true);
    }

    #[test]
    fn cron_job_new_sets_defaults() {
        let kind = ScheduleKind::Every {
            every_ms: 30_000,
            anchor_ms: None,
        };
        let job = CronJob::new("Test Job", "agent-1", "do something", kind.clone());

        // UUID format
        assert!(uuid::Uuid::parse_str(&job.id).is_ok());
        assert_eq!(job.name, "Test Job");
        assert_eq!(job.agent_id, "agent-1");
        assert_eq!(job.prompt, "do something");
        assert!(job.enabled);
        assert_eq!(job.schedule_kind, kind);
        assert_eq!(job.session_target, SessionTarget::Isolated);
        assert!(
            job.timeout_ms.is_none(),
            "new jobs have no timeout override"
        );
        assert_eq!(job.effective_timeout_ms(), 900_000);

        let mut overridden = job.clone();
        overridden.timeout_ms = Some(600_000);
        assert_eq!(overridden.effective_timeout_ms(), 600_000);
        // Timestamps should be recent (within last second, in ms)
        let now = chrono::Utc::now().timestamp_millis();
        assert!((now - job.created_at).abs() < 1000);
        assert!((now - job.updated_at).abs() < 1000);
        // State defaults
        assert_eq!(job.state.consecutive_errors, 0);
    }

    #[test]
    fn test_job_run_lifecycle() {
        let run = JobRun::new("job-1");
        assert_eq!(run.status, RunStatus::Ok);
        assert!(run.ended_at == 0);

        let run = run.success(Some("Done!".to_string()));
        assert_eq!(run.status, RunStatus::Ok);
        assert!(run.ended_at > 0);

        let run2 = JobRun::new("job-2").failed("Error occurred".to_string());
        assert_eq!(run2.status, RunStatus::Error);
        assert!(run2.error.is_some());
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world!", 8), "hello...");
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
    fn test_cron_job_view_from() {
        let job = CronJob::new(
            "View Test",
            "agent-1",
            "test prompt",
            ScheduleKind::default(),
        );
        let view = CronJobView::from(&job);
        assert_eq!(view.id, job.id);
        assert_eq!(view.name, "View Test");
        assert!(view.enabled);
        assert_eq!(view.agent_id, "agent-1");
    }
}
