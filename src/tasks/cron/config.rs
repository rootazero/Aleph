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
    format!("{ALEPH_HOME_PREFIX}data/tasks.db")
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

/// Prefix that marks a `db_path` as living inside Aleph's own state root
/// rather than somewhere the user picked. Only this prefix is `ALEPH_HOME`-
/// aware; a bare `~/` means the operator's real home and stays there.
const ALEPH_HOME_PREFIX: &str = "~/.aleph/";

/// Expand a configured `db_path`.
///
/// `~/.aleph/…` is Aleph's own state root, so it resolves through
/// [`crate::utils::paths::get_config_dir`] — the single source that honours
/// `ALEPH_HOME`, and the same one `cron::carryover` already uses for the
/// sibling `data/cron_carryover/` directory. Expanding it by hand off the real
/// home (which this did until 2026-08-08) parks the scheduler's whole job
/// database outside the relocated home: every other subsystem moves, cron does
/// not, and nothing errors — the operator just boots into an empty scheduler.
///
/// `get_config_dir` is a pure lookup and creates nothing; the store's own
/// `load()` creates the parent directory when it opens the file.
fn expand_configured_path(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix(ALEPH_HOME_PREFIX) {
        if let Ok(root) = crate::utils::paths::get_config_dir() {
            return root.join(rest).to_string_lossy().to_string();
        }
    }
    // A user-written `~/` prefix means the real home directory.
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Ok(home) = crate::utils::paths::get_home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    raw.to_string()
}

impl CronConfig {
    /// Expand the database path (resolve `~`). See [`expand_configured_path`].
    #[must_use]
    pub fn expand_db_path(&self) -> String {
        expand_configured_path(&self.db_path)
    }

    /// Where this store lived before `ALEPH_HOME` was honoured, if anywhere.
    ///
    /// `Some` only for the `~/.aleph/`-rooted default — a user-supplied
    /// absolute or bare-`~/` path always resolved the same way, so it has
    /// nothing to migrate. Consumed once at boot by
    /// [`crate::tasks::cron::store::migrate_legacy_store`].
    #[must_use]
    pub fn legacy_db_path(&self) -> Option<std::path::PathBuf> {
        self.db_path
            .strip_prefix(ALEPH_HOME_PREFIX)
            .and_then(crate::utils::paths::legacy_home_aleph_path)
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

/// What triggered a job run.
///
/// Chain and Catchup were removed as orphan variants (no production constructor).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TriggerSource {
    #[default]
    Schedule,
    Manual,
}

impl TriggerSource {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Schedule => "schedule",
            Self::Manual => "manual",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "manual" => Self::Manual,
            _ => Self::Schedule,
        }
    }
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
    /// Set by `CronService::run_job`, consumed (and cleared) by
    /// `phase1_mark_due_jobs`, which turns it into `TriggerSource::Manual`.
    ///
    /// A manual trigger is expressed by pulling `next_run_at_ms` forward to
    /// *now*, which by itself is indistinguishable from the schedule coming
    /// due — so without this flag every run the timer picks up looks scheduled
    /// and `trigger_source` is a constant. Persisted so a restart between the
    /// click and the tick does not silently downgrade the run.
    #[serde(default)]
    pub manual_trigger_pending: bool,
}

// ── JobChain ────────────────────────────────────────────────────────────

/// The two chain successors of a job, carried as one settable unit.
///
/// The successors are persisted on `CronJob` as two flat `Option<String>`
/// fields (unchanged on the wire and on disk); this is the shape every *write*
/// surface takes, so both the `cron_manage` tool and the `cron.create` /
/// `cron.update` RPCs express "set the chain" with one field and one clear
/// flag rather than four independent knobs that can be set half-way.
///
/// Why this exists at all: the trigger machinery, its cycle detection and its
/// tests shipped complete, but until 2026-08-09 the only writers of the two
/// fields were `#[cfg(test)]`. `on_failure` in particular is not something the
/// model can arrange for itself — when a job times out or its provider dies,
/// no agent turn is running to call `cron_manage` — so this is capability the
/// harness owes the model, not reasoning taken away from it.
/// `deny_unknown_fields` because every field here is optional: without it a
/// caller who writes `{"onSuccess": "b"}` deserializes cleanly into an EMPTY
/// chain, which the update path reads as "clear the chain" and reports as
/// success. That is the same silent no-op `failure_alert` shipped with, and
/// all-optional structs are where it comes from. The cost is that adding a
/// field here is a breaking change for any client that pins the old shape —
/// acceptable while both ends of this wire ship from this tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobChain {
    /// Job ID to run immediately after this job finishes `Ok` (or `Skipped`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_success: Option<String>,
    /// Job ID to run immediately after this job finishes `Error` or `Timeout`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
}

impl JobChain {
    /// `true` when neither successor is set — the same state as "no chain".
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.on_success.is_none() && self.on_failure.is_none()
    }

    /// Both successors, in a fixed order, skipping unset ones.
    pub fn targets(&self) -> impl Iterator<Item = &str> {
        self.on_success
            .iter()
            .chain(self.on_failure.iter())
            .map(String::as_str)
    }
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

    // A `timezone` field lived here until 2026-08-08. It had three writers
    // (RPC create, RPC update, the store round-trip) and **zero** readers in
    // the scheduler: `compute_next_run_for_job` only ever consults
    // `ScheduleKind::Cron { tz }`. Worse, the Panel editor rebuilt
    // `schedule_kind` without a `tz`, so editing any job silently discarded
    // the timezone `cron_manage` had written into the schedule. The single
    // source is `ScheduleKind::Cron { tz }`; `CronJobView::timezone` is
    // derived from it for display.
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

    /// Job ID to trigger on success. Read via [`CronJob::chain`]; set via
    /// [`CronJob::set_chain`] — do not assign directly outside test fixtures,
    /// or the write-time cycle/existence check in `CronService` is bypassed.
    #[serde(default)]
    pub next_job_id_on_success: Option<String>,

    /// Job ID to trigger on failure. Same accessors as the success link.
    #[serde(default)]
    pub next_job_id_on_failure: Option<String>,

    // A `delivery_config` field lived here until 2026-08-08. Nothing ever
    // wrote it (no RPC, no tool, no CLI accepted it) and the one place that
    // read it — `phase1_mark_due_jobs` → `JobSnapshot::delivery` — carried it
    // to the executor's door and dropped it: `execute_cron_job` routes results
    // through `source_channel_id` / `source_conversation_id` instead. The
    // whole chain was inert, so it is gone rather than reconnected.
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

    /// P1 data isolation: the user id that created this job, stamped once at
    /// `cron_manage(action='create')` time from `scope::current_scope()`
    /// inside the creating (admin-gated) run. `#[serde(default)]` → old
    /// (pre-P1) payloads read `None` — unscoped, legacy owner semantics;
    /// `skip_serializing_if` → a legacy job round-trips byte-identical
    /// instead of gaining an `"owner_user_id":null` key, matching
    /// `SessionMetadata`'s two fields exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
    /// The rendered scope boundary (`scope::ScopeId::render()`) paired with
    /// [`Self::owner_user_id`]. `#[serde(default)]` → old payloads read
    /// `None`; `skip_serializing_if` → they round-trip unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_id: Option<String>,
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
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            schedule_kind,
            session_target: SessionTarget::default(),
            next_job_id_on_success: None,
            next_job_id_on_failure: None,
            failure_alert: None,
            prompt_template: None,
            context_vars: None,
            timeout_ms: None,
            state: JobStateV2::default(),
            owner_user_id: None,
            scope_id: None,
        }
    }

    /// Stamp this job with the ambient scope of the request creating it.
    ///
    /// **The one derivation of "who owns a cron job".** Every production face
    /// that mints a `CronJob` calls exactly this — `cron.create` (RPC),
    /// `cron_manage(action='create')` (tool), and `loop_graph`'s two
    /// governance installers via `stamp_governance_job`. Three of those four
    /// carried their own inline copy of these four lines and the fourth
    /// carried none, so every job created from the Panel or the CLI reached
    /// the store with both columns NULL — and four readers then short-circuit
    /// on that NULL: the deactivation sweep (`pause_all_owned_by`) counts it
    /// as not-owned, `walled_owner_reason` skips it as "legacy"
    /// (byte-indistinguishable from a genuinely pre-P1 job),
    /// `executor::build_cron_metadata` has no `ScopeAttribution` to rehydrate
    /// so the run executes unscoped — the UNRESTRICTED arm of every visibility
    /// predicate — and its spend lands in `@unattributed`.
    ///
    /// `config::tests::every_production_cron_job_construction_stamps_the_scope`
    /// is keyed on this method's NAME, so a fifth construction face that skips
    /// it fails by name rather than by someone noticing.
    ///
    /// # Why not inside `CronService::add_job`
    ///
    /// `add_job` is reached from paths where the ambient scope is not the
    /// creator's. An unguarded stamp there would OVERWRITE an explicit owner
    /// rather than supply a missing one — a job silently changing hands, which
    /// is the expensive direction. The stamp belongs at the face that knows it
    /// is minting a NEW job for the caller in front of it.
    ///
    /// `current_scope()` being `None` (single-user install, or an internal
    /// path with no request around it) leaves both columns `None` — exactly
    /// the legacy shape `ScopeAttribution::from_persisted` already reads as
    /// "unscoped". No behaviour change there.
    pub fn stamp_current_scope(&mut self) {
        if let Some(attr) = crate::scope::current_scope() {
            self.owner_user_id = Some(attr.owner_user_id);
            self.scope_id = Some(attr.scope.render());
        }
    }

    /// This job's chain successors, or `None` when it chains to nothing.
    ///
    /// Projected from the two persisted fields — never stored separately, so
    /// a view can't disagree with what `phase3` actually reads.
    #[must_use]
    pub fn chain(&self) -> Option<JobChain> {
        let chain = JobChain {
            on_success: self.next_job_id_on_success.clone(),
            on_failure: self.next_job_id_on_failure.clone(),
        };
        (!chain.is_empty()).then_some(chain)
    }

    /// Replace both chain successors. `None` clears the chain entirely.
    ///
    /// Replace, not merge: a caller that sends `{on_success: "b"}` after
    /// `{on_success: "a", on_failure: "c"}` means "the chain is now just
    /// b", which is the same convention `failure_alert` uses on update. The
    /// caller is expected to have run `chain::validate` first — `CronService`
    /// is the enforcement point.
    pub fn set_chain(&mut self, chain: Option<JobChain>) {
        let chain = chain.unwrap_or_default();
        self.next_job_id_on_success = chain.on_success;
        self.next_job_id_on_failure = chain.on_failure;
    }

    /// The timezone this job's schedule is evaluated in, if any.
    ///
    /// Reads the single source (`ScheduleKind::Cron { tz }`) — the same field
    /// `compute_next_run_for_job` passes to `compute_next_cron`. Interval and
    /// one-shot schedules are absolute, so they have no timezone.
    #[must_use]
    pub fn schedule_timezone(&self) -> Option<&str> {
        match &self.schedule_kind {
            ScheduleKind::Cron { tz, .. } => tz.as_deref(),
            ScheduleKind::Every { .. } | ScheduleKind::At { .. } => None,
        }
    }

    /// Effective per-job execution timeout in milliseconds.
    ///
    /// Returns this job's `timeout_ms` override when set, else falls back to
    /// the provided configured default (`CronConfig::job_timeout_secs` in
    /// production). Used by the catchup stale-marker threshold; runtime
    /// execution gets the timeout via `JobSnapshot.timeout_ms` threaded by
    /// `phase1_mark_due_jobs`, which honours the per-job override against the
    /// *configured* default.
    #[must_use]
    pub fn effective_timeout_ms(&self, default_timeout_ms: i64) -> i64 {
        self.timeout_ms.unwrap_or(default_timeout_ms)
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
    // `model` and `delivery` lived here until 2026-08-08. Every construction
    // site wrote `model: None` (there was no `CronJob` field it could come
    // from) while the executor hard-wired `model_override: None`, and
    // `delivery` was the terminus of the inert `CronJob::delivery_config`
    // chain. Both were removed rather than given a source.
    pub timeout_ms: Option<i64>,
    pub session_target: SessionTarget,
    pub marked_at: i64,
    pub trigger_source: TriggerSource,
    /// P1 data isolation: copied from `CronJob::owner_user_id` at snapshot
    /// time (`phase1_mark_due_jobs`) so the
    /// fire path can rehydrate attribution with no completing run to inherit
    /// metadata from — see `executor::build_cron_metadata`.
    pub owner_user_id: Option<String>,
    /// Copied from `CronJob::scope_id` alongside [`Self::owner_user_id`].
    pub scope_id: Option<String>,
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
    /// Derived, read-only: the job is switched on but has nothing scheduled,
    /// so it will never fire again until an operator acts.
    ///
    /// `enabled` alone cannot express this. Phase 3 parks a permanently-failed
    /// job by clearing `next_run_at_ms` and leaves the switch on, so every
    /// surface that only shows `enabled` shows a healthy-looking job that is
    /// in fact dead. Not persisted — always recomputed from state, so it can
    /// never disagree with the two fields it is made of.
    #[serde(default)]
    pub parked: bool,
    pub schedule_kind: ScheduleKind,
    pub agent_id: String,
    pub source_channel_id: Option<String>,
    pub prompt: String,
    /// Derived, read-only: the timezone the schedule is evaluated in.
    ///
    /// Not persisted — projected from `ScheduleKind::Cron { tz }` (the single
    /// source) so it can never disagree with what the scheduler actually uses.
    /// `None` for interval / one-shot schedules, which have no timezone.
    pub timezone: Option<String>,
    pub tags: Vec<String>,
    pub session_target: SessionTarget,
    pub state: JobStateV2,
    pub failure_alert: Option<FailureAlertConfig>,
    /// Chain successors, `None` when this job chains to nothing.
    ///
    /// Every surface that can *set* the chain reads it back through this
    /// field. A settable field with no read-back is half a wire: the caller
    /// gets `success` and has no way to confirm what was stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<JobChain>,
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
            parked: job.enabled && job.state.next_run_at_ms.is_none(),
            schedule_kind: job.schedule_kind.clone(),
            agent_id: job.agent_id.clone(),
            source_channel_id: job.source_channel_id.clone(),
            prompt: job.prompt.clone(),
            timezone: job.schedule_timezone().map(str::to_string),
            tags: job.tags.clone(),
            session_target: job.session_target.clone(),
            state: job.state.clone(),
            failure_alert: job.failure_alert.clone(),
            chain: job.chain(),
            timeout_ms: job.timeout_ms,
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

// ── Delivery types ──────────────────────────────────────────────────────
// Re-exported from shared::delivery to avoid breaking existing consumers.

pub use crate::tasks::shared::alert::FailureAlertConfig;
pub use crate::tasks::shared::delivery::{
    DeliveryConfig, DeliveryMode, DeliveryOutcome, DeliveryStatus, DeliveryTargetConfig,
};

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// One production `fn`, brace-matched out of a source file.
    struct ProductionFn {
        file: String,
        name: String,
        body: String,
    }

    /// The identifier of the `fn` this line declares, if it declares one.
    fn declared_fn_name(line: &str) -> Option<String> {
        let mut t = line.trim_start();
        for prefix in [
            "pub(crate) ",
            "pub(super) ",
            "pub(self) ",
            "pub ",
            "default ",
            "const ",
            "async ",
            "unsafe ",
            "extern ",
        ] {
            while let Some(rest) = t.strip_prefix(prefix) {
                t = rest.trim_start();
            }
        }
        let rest = t.strip_prefix("fn ")?;
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    }

    /// Every `fn` in the crate's PRODUCTION source that could bear on this
    /// census, with its brace-matched body.
    ///
    /// Test items are removed by `production_prefix` (a whole-item walk, not a
    /// `split("#[cfg(test)]")` cut) and literal payloads are blanked by
    /// `code_text`, so neither a `{` inside a string nor this test's own needle
    /// constants can reach the scan.
    ///
    /// `needles` prefilters on RAW file text — a superset of what the stripped
    /// scan can find (raw text still carries comments and test modules), so a
    /// file dropped here could not have contained a production hit. It exists
    /// because brace-matching every `fn` in the crate took over a minute.
    fn production_fns(needles: &[&str]) -> Vec<ProductionFn> {
        use crate::utils::source_scan::{code_text, production_prefix, rust_sources_under};

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = rust_sources_under(&root);
        assert!(
            files.len() > 100,
            "the source scan found only {} files under src/ — it is measuring \
             nothing, so every verdict below would be vacuously green",
            files.len()
        );

        let mut out = Vec::new();
        for (file, text) in &files {
            if !needles.iter().any(|n| text.contains(n)) {
                continue;
            }
            let code = code_text(&production_prefix(text));
            let lines: Vec<&str> = code.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let Some(name) = declared_fn_name(line) else {
                    continue;
                };
                let (mut depth, mut opened, mut end) = (0i32, false, i);
                for (k, l) in lines.iter().enumerate().skip(i) {
                    depth += i32::try_from(l.matches('{').count()).unwrap_or(0);
                    depth -= i32::try_from(l.matches('}').count()).unwrap_or(0);
                    opened |= l.contains('{');
                    end = k;
                    if opened && depth <= 0 {
                        break;
                    }
                }
                out.push(ProductionFn {
                    file: file.clone(),
                    name,
                    body: lines[i..=end].join("\n"),
                });
            }
        }
        out
    }

    /// Every production site that mints a `CronJob` must stamp the ambient
    /// scope onto it, and this guard is keyed on the ONE derivation's NAME.
    ///
    /// The bug it exists to make impossible: `cron.create`'s RPC handler built
    /// a job and never touched `owner_user_id` / `scope_id`, so every job the
    /// Panel or the CLI created was ownerless — while three other faces each
    /// carried their own inline copy of the same four lines. A fifth face that
    /// forgets fails here, by name.
    ///
    /// **What it does NOT prove**: this is a name census, not a dataflow
    /// proof. A body that stamps a *different* job than the one it constructs
    /// passes. What it does prove is the shape that actually shipped — a
    /// creating face where the derivation is not named at all.
    ///
    /// The set of helper fns that count as stamping is DERIVED from the same
    /// method name — any production fn IN THE SAME FILE whose own body names
    /// it — so there is no hand-kept list of blessed helpers to rot.
    ///
    /// The relation is deliberately file-local rather than crate-wide. A
    /// crate-wide closure over bare fn NAMES was written first and was a false
    /// green: `stamp_governance_job` made its caller `execute` a stamper, and
    /// "any body that calls something named `execute(`" is most of the crate,
    /// so the guard approved a `handle_create` with the stamp deleted. A
    /// census whose reach grows by one hop per common name is a census that
    /// eventually approves everything.
    #[test]
    fn every_production_cron_job_construction_stamps_the_scope() {
        const STAMP: &str = "stamp_current_scope";
        const CONSTRUCT: &str = "CronJob::new(";

        let fns = production_fns(&[CONSTRUCT, STAMP]);

        // Fixed point, per file: a fn stamps if it names the derivation, or
        // calls a same-file helper that does.
        let mut stampers: Vec<(&str, &str)> = Vec::new();
        loop {
            let before = stampers.len();
            for f in &fns {
                let key = (f.file.as_str(), f.name.as_str());
                if stampers.contains(&key) {
                    continue;
                }
                let stamps = f.body.contains(STAMP)
                    || stampers.iter().any(|(file, helper)| {
                        *file == f.file
                            && *helper != f.name
                            && f.body.contains(&format!("{helper}("))
                    });
                if stamps {
                    stampers.push(key);
                }
            }
            if stampers.len() == before {
                break;
            }
        }
        assert!(
            stampers.iter().any(|(_, name)| *name == STAMP),
            "the derivation `CronJob::{STAMP}` was not found in production \
             source at all — it was renamed or deleted, and this census is \
             now guarding a name nothing has"
        );

        let sites: Vec<&ProductionFn> = fns.iter().filter(|f| f.body.contains(CONSTRUCT)).collect();
        // Self-protection (criterion #3): a census that finds nothing to judge
        // is green for the wrong reason. These four faces are the ones the
        // round measured; the assertion is that the scan still SEES them, not
        // an exclusion list — anything else it finds is judged the same way.
        for known in [
            "src/gateway/handlers/cron/real.rs",
            "src/builtin_tools/cron_manage.rs",
            "src/builtin_tools/loop_graph_manage.rs",
        ] {
            assert!(
                sites.iter().any(|s| s.file == known),
                "the scan no longer sees the known `{CONSTRUCT}` site in \
                 {known} — it is measuring less than it did, so its silence \
                 about the others means nothing"
            );
        }

        let offenders: Vec<String> = sites
            .iter()
            .filter(|s| {
                !(s.body.contains(STAMP)
                    || stampers.iter().any(|(file, helper)| {
                        *file == s.file
                            && *helper != s.name
                            && s.body.contains(&format!("{helper}("))
                    }))
            })
            .map(|s| format!("{}::{}", s.file, s.name))
            .collect();

        assert!(
            offenders.is_empty(),
            "these production sites construct a CronJob without calling \
             `CronJob::{STAMP}` (directly or through a helper that does): \
             {offenders:?}. A job created with neither owner_user_id nor \
             scope_id is invisible to `pause_all_owned_by`, reads as a legacy \
             job to `walled_owner_reason`, executes unscoped, and charges its \
             spend to @unattributed."
        );
    }

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

    /// The whole job database used to hang off `dirs::home_dir()`, so a
    /// relocated `ALEPH_HOME` left cron reading and writing a directory
    /// nothing else in the process touches — silently, since with the variable
    /// unset the two spellings are byte-identical.
    #[test]
    fn default_db_path_follows_aleph_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(dir.path());

        let expanded = CronConfig::default().expand_db_path();
        assert_eq!(
            std::path::Path::new(&expanded),
            dir.path().join("data").join("tasks.db"),
            "the cron store must live inside ALEPH_HOME like every other subsystem"
        );
    }

    /// A bare `~/` prefix is a user-chosen location and means the real home,
    /// not Aleph's state root.
    #[test]
    fn bare_tilde_path_is_not_redirected_into_aleph_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(dir.path());

        let config = CronConfig {
            db_path: "~/elsewhere/tasks.db".to_string(),
            ..CronConfig::default()
        };
        let expanded = config.expand_db_path();
        assert!(
            !std::path::Path::new(&expanded).starts_with(dir.path()),
            "a user-written ~/ path must not be captured by ALEPH_HOME, got {expanded}"
        );
        assert!(expanded.ends_with("elsewhere/tasks.db"));
    }

    /// Only the `~/.aleph/`-rooted default ever moved, so only it has a legacy
    /// location worth migrating from.
    #[test]
    fn legacy_db_path_is_only_offered_for_the_aleph_rooted_default() {
        assert!(CronConfig::default().legacy_db_path().is_some());

        for custom in ["/var/lib/aleph/tasks.db", "~/elsewhere/tasks.db"] {
            let config = CronConfig {
                db_path: custom.to_string(),
                ..CronConfig::default()
            };
            assert!(
                config.legacy_db_path().is_none(),
                "{custom} always resolved the same way; there is nothing to migrate"
            );
        }
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
        assert_eq!(job.effective_timeout_ms(900_000), 900_000);

        let mut overridden = job.clone();
        overridden.timeout_ms = Some(600_000);
        assert_eq!(overridden.effective_timeout_ms(900_000), 600_000);
        // Timestamps should be recent (within last second, in ms)
        let now = chrono::Utc::now().timestamp_millis();
        assert!((now - job.created_at).abs() < 1000);
        assert!((now - job.updated_at).abs() < 1000);
        // State defaults
        assert_eq!(job.state.consecutive_errors, 0);
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

    /// A job persisted BEFORE `timezone` / `delivery_config` were retired must
    /// still load. Unlike a config file, this is durable user data with no
    /// migration step, so "the field is gone" has to mean "silently ignored",
    /// never "this job fails to deserialize and disappears from the list".
    ///
    /// Also pins the reason no migration was needed: the scheduler
    /// (`compute_next_run_for_job`) has only ever read `ScheduleKind::Cron
    /// { tz }`. The retired top-level `timezone` was write-only, so a stored
    /// job's firing time is byte-identical before and after the cut — and
    /// `schedule_timezone()` reports the value that actually governs it.
    #[test]
    fn a_job_persisted_with_the_retired_keys_still_loads() {
        // Start from a real job so every *surviving* required field is present
        // exactly as the store writes it, then graft on only the retired keys.
        // Hand-listing the whole struct would make this test rot the next time
        // a field is added, and it would then rot silently green.
        let live = CronJob::new(
            "nightly",
            "main",
            "summarise",
            ScheduleKind::Cron {
                expr: "0 0 3 * * *".into(),
                tz: Some("Asia/Shanghai".into()),
                stagger_ms: None,
            },
        );
        let mut stored = serde_json::to_value(&live).unwrap();
        let obj = stored.as_object_mut().unwrap();
        // Retired 2026-08-08 — present in every job written before then.
        obj.insert("timezone".into(), serde_json::json!("America/New_York"));
        obj.insert(
            "delivery_config".into(),
            serde_json::json!({
                "mode": "primary",
                "targets": [{ "type": "webhook", "url": "https://x.example" }]
            }),
        );

        let job: CronJob =
            serde_json::from_value(stored).expect("a pre-cut job must still deserialize");
        assert_eq!(job.id, live.id);
        assert!(job.enabled);
        // The schedule's tz is the single source, and it is what survived —
        // NOT the stale top-level key that never reached the scheduler.
        assert_eq!(job.schedule_timezone(), Some("Asia/Shanghai"));
        assert!(
            crate::tasks::cron::service::ops::compute_next_run_for_job(
                &job,
                chrono::Utc::now().timestamp_millis(),
            )
            .is_some(),
            "a legacy job must still schedule after the cut"
        );
    }

    #[test]
    fn test_trigger_source_roundtrip() {
        for (s, v) in [
            ("schedule", TriggerSource::Schedule),
            ("manual", TriggerSource::Manual),
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
