# Cron Module Redesign Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the cron module's internals into an infrastructure-grade task scheduling system with anchor-aligned scheduling, three-phase concurrency, crash recovery, and panel UI sync.

**Architecture:** Module rebuild — replace the monolithic `mod.rs` (1346 lines) + SQLite-only persistence with a layered service architecture: pure scheduling functions → JSON atomic persistence → three-phase concurrent service → dual-path execution. Preserve Gateway RPC interface signatures where possible.

**Tech Stack:** Rust, tokio, serde_json, sha2, chrono, cron crate, rusqlite (for history only), Leptos 0.8 (panel UI)

**Spec:** `docs/superpowers/specs/2026-03-14-cron-module-redesign.md`

---

## File Map

### New Files to Create
| File | Responsibility |
|------|---------------|
| `core/src/cron/clock.rs` | Clock trait + SystemClock + FakeClock |
| `core/src/cron/schedule.rs` | Pure scheduling computation (anchor-aligned, cron-next, MIN_REFIRE_GAP) |
| `core/src/cron/stagger.rs` | SHA256 hash-based stagger spreading |
| `core/src/cron/store.rs` | JSON atomic persistence (tmp+fsync+rename), load, reload, migration |
| `core/src/cron/alert.rs` | Failure alerting + cooldown |
| `core/src/cron/service/mod.rs` | Service facade, re-exports |
| `core/src/cron/service/state.rs` | ServiceState<C: Clock> runtime container |
| `core/src/cron/service/ops.rs` | CRUD operations + manual trigger (zero side-effect reads) |
| `core/src/cron/service/concurrency.rs` | Three-phase execution model |
| `core/src/cron/service/timer.rs` | Core scheduling loop + worker pool |
| `core/src/cron/service/catchup.rs` | Restart catchup strategy |
| `core/src/cron/execution/mod.rs` | Execution module facade |
| `core/src/cron/execution/lightweight.rs` | Main-loop event injection |
| `core/src/cron/execution/isolated.rs` | Isolated agent session execution |

### Files to Modify
| File | Changes |
|------|---------|
| `core/src/cron/config.rs` | Enhanced ScheduleKind (rich enum), JobState (new fields), new types (RunStatus, ErrorReason, DeliveryStatus, FailureAlertConfig, JobSnapshot, ExecutionResult, SessionTarget, CronJobView) |
| `core/src/cron/mod.rs` | Gut the monolith — becomes thin facade re-exporting submodules. CronService becomes a wrapper around service::CronServiceInner |
| `core/src/cron/delivery.rs` | Add dedup logic (AlreadySentByAgent) |
| `core/src/cron/chain.rs` | Migrate from SQLite queries to CronStore operations |
| `core/src/cron/template.rs` | Inject Clock trait instead of direct Utc::now() |
| `core/src/gateway/handlers/cron.rs` | Update to use new service API, add new fields to JSON serialization |
| `apps/panel/src/api/cron.rs` | Add new DTO fields |
| `apps/panel/src/views/cron.rs` | Three-state indicator, anchor/stagger inputs, failure alert section, delivery column |

### Files to Delete
| File | Reason |
|------|--------|
| `core/src/cron/scheduler.rs` | Replaced by `schedule.rs` |
| `core/src/cron/resource.rs` | CPU-aware concurrency removed; fixed limits used instead |

---

## Chunk 1: Foundation — Clock, Config Types, Schedule, Stagger

### Task 1: Clock Trait

**Files:**
- Create: `core/src/cron/clock.rs`
- Modify: `core/src/cron/mod.rs` (add `pub mod clock;`)

- [ ] **Step 1: Write Clock trait and SystemClock**

```rust
// core/src/cron/clock.rs
use chrono::{DateTime, Utc};

/// Abstraction over system time for testability.
/// All time-dependent cron logic receives this instead of calling Utc::now() directly.
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
    fn now_utc(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.now_ms())
            .unwrap_or_else(|| Utc::now())
    }
}

/// Production clock — delegates to system time.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        Utc::now().timestamp_millis()
    }
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
```

- [ ] **Step 2: Write FakeClock under test-helpers feature**

```rust
// Append to core/src/cron/clock.rs
#[cfg(any(test, feature = "test-helpers"))]
pub mod testing {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// Test clock with manual time control.
    pub struct FakeClock {
        current_ms: AtomicI64,
    }

    impl FakeClock {
        pub fn new(initial_ms: i64) -> Self {
            Self { current_ms: AtomicI64::new(initial_ms) }
        }

        pub fn advance(&self, ms: i64) {
            self.current_ms.fetch_add(ms, Ordering::SeqCst);
        }

        pub fn set(&self, ms: i64) {
            self.current_ms.store(ms, Ordering::SeqCst);
        }
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> i64 {
            self.current_ms.load(Ordering::SeqCst)
        }
    }
}
```

- [ ] **Step 3: Write tests for FakeClock**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::testing::FakeClock;

    #[test]
    fn system_clock_returns_reasonable_time() {
        let clock = SystemClock;
        let now = clock.now_ms();
        // Should be after 2025-01-01
        assert!(now > 1735689600000);
    }

    #[test]
    fn fake_clock_initial_value() {
        let clock = FakeClock::new(1_000_000);
        assert_eq!(clock.now_ms(), 1_000_000);
    }

    #[test]
    fn fake_clock_advance() {
        let clock = FakeClock::new(1_000_000);
        clock.advance(500);
        assert_eq!(clock.now_ms(), 1_000_500);
        clock.advance(500);
        assert_eq!(clock.now_ms(), 1_001_000);
    }

    #[test]
    fn fake_clock_set() {
        let clock = FakeClock::new(1_000_000);
        clock.set(2_000_000);
        assert_eq!(clock.now_ms(), 2_000_000);
    }

    #[test]
    fn fake_clock_now_utc() {
        let clock = FakeClock::new(1710000000000); // 2024-03-09T16:00:00Z
        let dt = clock.now_utc();
        assert_eq!(dt.timestamp_millis(), 1710000000000);
    }
}
```

- [ ] **Step 4: Register module in mod.rs**

Add `pub mod clock;` to `core/src/cron/mod.rs` near the top module declarations.

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib cron::clock`
Expected: All 5 tests pass.

- [ ] **Step 6: Commit**

```bash
git add core/src/cron/clock.rs core/src/cron/mod.rs
git commit -m "cron: add Clock trait with SystemClock and FakeClock"
```

---

### Task 2: Enhanced Config Types

**Files:**
- Modify: `core/src/cron/config.rs`

This task adds the new types from the spec alongside existing types. Existing types are preserved for now (they'll be migrated in Chunk 3).

- [ ] **Step 1: Add new enums and structs**

Append to `core/src/cron/config.rs`:

```rust
// === New types for cron module redesign ===

/// Execution status of a job run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    Error,
    Skipped,
    Timeout,
}

/// Classification of execution errors for retry decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ErrorReason {
    /// Network, rate_limit, 5xx, timeout — worth retrying
    Transient(String),
    /// Auth, bad config, invalid prompt — don't retry
    Permanent(String),
}

/// Delivery outcome tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Delivered,
    NotDelivered,
    AlreadySentByAgent,
    NotRequested,
}

/// Execution target for a cron job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionTarget {
    /// Inject event into main agent session (lightweight, <1ms)
    Main,
    /// Execute in isolated agent session (heavyweight, LLM cost)
    #[default]
    Isolated,
}

/// Failure alert configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FailureAlertConfig {
    /// Alert after N consecutive failures (default: 2)
    #[serde(default = "default_alert_after")]
    pub after: u32,
    /// Cooldown between alerts in ms (default: 3600000 = 1h)
    #[serde(default = "default_alert_cooldown")]
    pub cooldown_ms: i64,
    /// Alert delivery target
    pub target: DeliveryTargetConfig,
}

fn default_alert_after() -> u32 { 2 }
fn default_alert_cooldown() -> i64 { 3_600_000 }

/// Enhanced job runtime state with crash recovery and alert tracking.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct JobStateV2 {
    /// Computed next execution time (ms since epoch)
    pub next_run_at_ms: Option<i64>,
    /// Running marker — set in Phase 1, cleared in Phase 3. Used for crash recovery.
    pub running_at_ms: Option<i64>,
    /// Last execution timestamp
    pub last_run_at_ms: Option<i64>,
    /// Last execution status
    pub last_run_status: Option<RunStatus>,
    /// Last error message
    pub last_error: Option<String>,
    /// Classified error reason (transient vs permanent)
    pub last_error_reason: Option<ErrorReason>,
    /// Last execution duration (ms)
    pub last_duration_ms: Option<i64>,
    /// Consecutive error counter (for backoff calculation)
    #[serde(default)]
    pub consecutive_errors: u32,
    /// Schedule computation error counter (auto-disable threshold)
    #[serde(default)]
    pub schedule_error_count: u32,
    /// Last failure alert timestamp (for cooldown)
    pub last_failure_alert_at_ms: Option<i64>,
    /// Last delivery status
    pub last_delivery_status: Option<DeliveryStatus>,
}

/// Minimal snapshot for Phase 2 execution (no store access needed).
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub id: String,
    pub agent_id: Option<String>,
    /// Template-rendered prompt (rendered in Phase 1)
    pub prompt: String,
    pub model: Option<String>,
    pub timeout_ms: Option<i64>,
    pub delivery: Option<DeliveryConfig>,
    pub session_target: SessionTarget,
    pub marked_at: i64,
    pub trigger_source: TriggerSource,
}

/// Result of a job execution (produced in Phase 2, consumed in Phase 3).
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
}

/// Read-only projection of CronJob for list/get operations.
/// Ensures read operations cannot mutate state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobView {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule_kind: ScheduleKind,
    pub agent_id: String,
    pub prompt: String,
    pub timezone: Option<String>,
    pub tags: Vec<String>,
    pub session_target: SessionTarget,
    pub state: JobStateV2,
    pub delivery_config: Option<DeliveryConfig>,
    pub failure_alert: Option<FailureAlertConfig>,
    pub created_at: i64,
    pub updated_at: i64,
}
```

- [ ] **Step 2: Add `From<&CronJob> for CronJobView` implementation**

```rust
impl From<&CronJob> for CronJobView {
    fn from(job: &CronJob) -> Self {
        Self {
            id: job.id.clone(),
            name: job.name.clone(),
            enabled: job.enabled,
            schedule_kind: job.schedule_kind.clone(),
            agent_id: job.agent_id.clone(),
            prompt: job.prompt.clone(),
            timezone: job.timezone.clone(),
            tags: job.tags.clone(),
            session_target: job.session_target.clone(),
            state: job.state.clone(),
            delivery_config: job.delivery_config.clone(),
            failure_alert: job.failure_alert.clone(),
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}
```

- [ ] **Step 3: Write basic tests for serde round-trip**

```rust
#[cfg(test)]
mod new_type_tests {
    use super::*;

    #[test]
    fn run_status_serde_roundtrip() {
        let status = RunStatus::Error;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"error\"");
        let back: RunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RunStatus::Error);
    }

    #[test]
    fn error_reason_serde_roundtrip() {
        let reason = ErrorReason::Transient("rate_limit".to_string());
        let json = serde_json::to_string(&reason).unwrap();
        let back: ErrorReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reason);
    }

    #[test]
    fn job_state_v2_defaults() {
        let state = JobStateV2::default();
        assert_eq!(state.consecutive_errors, 0);
        assert!(state.next_run_at_ms.is_none());
        assert!(state.running_at_ms.is_none());
    }

    #[test]
    fn session_target_default_is_isolated() {
        let target = SessionTarget::default();
        assert_eq!(target, SessionTarget::Isolated);
    }

    #[test]
    fn failure_alert_config_defaults() {
        // Use internally-tagged serde format matching DeliveryTargetConfig
        let json = r#"{"target":{"kind":"Webhook","url":"https://example.com"}}"#;
        let config: FailureAlertConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.after, 2);
        assert_eq!(config.cooldown_ms, 3_600_000);
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib cron::config::new_type_tests`
Expected: All 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add core/src/cron/config.rs
git commit -m "cron: add enhanced config types for module redesign"
```

---

### Task 2b: Rewrite CronJob Struct and ScheduleKind Enum

**Files:**
- Modify: `core/src/cron/config.rs`

This task transforms the existing `CronJob` and `ScheduleKind` to use the new type system. Since this is a greenfield rebuild, we rewrite in-place.

- [ ] **Step 1: Rewrite ScheduleKind as rich enum**

Replace the existing simple `ScheduleKind` enum (lines ~318-343) with the rich enum:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleKind {
    At {
        at: i64,
        #[serde(default)]
        delete_after_run: bool,
    },
    Every {
        every_ms: i64,
        #[serde(default)]
        anchor_ms: Option<i64>,
    },
    Cron {
        expr: String,
        #[serde(default)]
        tz: Option<String>,
        #[serde(default)]
        stagger_ms: Option<i64>,
    },
}
```

Remove the old `as_str()` and `parse()` methods. Remove the flat schedule fields from CronJob (`every_ms`, `at_time`, `delete_after_run` as top-level fields).

- [ ] **Step 2: Rewrite CronJob to use new type system**

Replace the existing CronJob struct with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub agent_id: String,
    pub prompt: String,
    pub enabled: bool,
    pub timezone: Option<String>,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,

    // Schedule
    pub schedule_kind: ScheduleKind,

    // Execution target
    #[serde(default)]
    pub session_target: SessionTarget,

    // Resilience
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    // Job chaining
    pub next_job_id_on_success: Option<String>,
    pub next_job_id_on_failure: Option<String>,

    // Delivery
    pub delivery_config: Option<DeliveryConfig>,

    // Failure alerting
    pub failure_alert: Option<FailureAlertConfig>,

    // Dynamic prompt
    pub prompt_template: Option<String>,
    pub context_vars: Option<String>,

    // Runtime state
    #[serde(default)]
    pub state: JobStateV2,
}

fn default_max_retries() -> u32 { 3 }

impl CronJob {
    pub fn new(name: String, agent_id: String, prompt: String, schedule_kind: ScheduleKind) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            agent_id,
            prompt,
            enabled: true,
            timezone: None,
            tags: vec![],
            created_at: now,
            updated_at: now,
            schedule_kind,
            session_target: SessionTarget::default(),
            max_retries: 3,
            next_job_id_on_success: None,
            next_job_id_on_failure: None,
            delivery_config: None,
            failure_alert: None,
            prompt_template: None,
            context_vars: None,
            state: JobStateV2::default(),
        }
    }

    /// Get job timeout in milliseconds.
    pub fn timeout_ms(&self) -> i64 {
        300_000 // Default 5 minutes; can be made configurable later
    }
}
```

- [ ] **Step 3: Update CronConfig to add max_concurrent_agents**

```rust
// Add to CronConfig struct:
/// Maximum concurrent isolated agent jobs (default: 2).
#[serde(default = "default_max_concurrent_agents")]
pub max_concurrent_agents: Option<usize>,

/// Maximum missed jobs to execute immediately on restart (default: 5).
pub max_missed_jobs_per_restart: Option<usize>,

/// Stagger interval for deferred missed jobs in ms (default: 5000).
pub catchup_stagger_ms: Option<i64>,

fn default_max_concurrent_agents() -> Option<usize> { Some(2) }
```

- [ ] **Step 4: Remove old flat fields and fix compilation**

Remove from old CronJob: `schedule`, `every_ms`, `at_time`, `delete_after_run`, `consecutive_failures`, `priority`, `next_run_at`, `running_at`, `last_run_at`, `version` (optimistic locking — replaced by three-phase model).

Fix all compilation errors in existing code that referenced these fields. Since this is greenfield, any code referencing old fields will be rewritten in later tasks anyway, but we need it to compile.

- [ ] **Step 5: Remove old JobStatus enum** (replaced by RunStatus)

Map any remaining references: `JobStatus::Success` → `RunStatus::Ok`, etc.

- [ ] **Step 6: Update CronJob::new() tests and fix compilation**

Run: `cargo check -p alephcore`
Fix any remaining compilation errors in config.rs and its dependents.

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib cron::config`
Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add core/src/cron/config.rs
git commit -m "cron: rewrite CronJob and ScheduleKind to new type system"
```

---

### Task 3: Pure Schedule Computation

**Files:**
- Create: `core/src/cron/schedule.rs`
- Modify: `core/src/cron/mod.rs` (add `pub mod schedule;`)

- [ ] **Step 1: Write anchor-aligned interval computation**

```rust
// core/src/cron/schedule.rs

/// Minimum gap between consecutive runs to prevent spin-loops.
/// Protects against cron library edge cases (timezone boundaries, DST).
pub const MIN_REFIRE_GAP_MS: i64 = 2_000;

/// Compute next run time for interval schedule with anchor alignment.
///
/// Aligns to `anchor + N * interval` grid points, preventing drift.
/// Example: anchor=10:00, interval=30min, run finishes 10:07 → next is 10:30.
pub fn compute_next_every(
    now_ms: i64,
    every_ms: i64,
    anchor_ms: i64,
    last_run_at_ms: Option<i64>,
) -> Option<i64> {
    // Guard: future manual trigger
    if let Some(last) = last_run_at_ms {
        if last > now_ms {
            return Some(last + every_ms);
        }
    }
    // Guard: future anchor
    if now_ms <= anchor_ms {
        return Some(anchor_ms);
    }
    // Guard: invalid interval
    if every_ms <= 0 {
        return None; // caller should increment schedule_error_count
    }

    // Core formula: align to anchor grid
    let elapsed = now_ms - anchor_ms;
    let periods = (elapsed + every_ms - 1) / every_ms; // ceil division (safe: both positive)
    Some(anchor_ms + periods * every_ms)
}

/// Apply minimum gap safety net to prevent spin-loops.
pub fn apply_min_gap(next_run_ms: i64, last_ended_ms: Option<i64>) -> i64 {
    match last_ended_ms {
        Some(ended) => {
            let min_next = ended + MIN_REFIRE_GAP_MS;
            next_run_ms.max(min_next)
        }
        None => next_run_ms,
    }
}

/// Resolve anchor for Every schedule: explicit > created_at.
pub fn resolve_anchor(explicit_anchor_ms: Option<i64>, created_at_ms: i64) -> i64 {
    explicit_anchor_ms.unwrap_or(created_at_ms)
}
```

- [ ] **Step 2: Write cron expression computation**

```rust
// Append to core/src/cron/schedule.rs
use chrono::{DateTime, Utc, TimeZone};
use cron::Schedule;
use std::str::FromStr;

/// Compute next run time from cron expression with timezone support.
pub fn compute_next_cron(
    expr: &str,
    tz: Option<&str>,
    from: DateTime<Utc>,
) -> Result<Option<i64>, String> {
    let schedule = Schedule::from_str(expr)
        .map_err(|e| format!("invalid cron expression '{}': {}", expr, e))?;

    let next = if let Some(tz_str) = tz {
        let tz: chrono_tz::Tz = tz_str.parse()
            .map_err(|_| format!("invalid timezone: {}", tz_str))?;
        let local_from = from.with_timezone(&tz);
        schedule.after(&local_from).next().map(|dt| dt.with_timezone(&Utc).timestamp_millis())
    } else {
        schedule.after(&from).next().map(|dt| dt.timestamp_millis())
    };

    Ok(next)
}

/// Backoff tiers for failed job retry (ms).
pub const BACKOFF_TIERS_MS: &[i64] = &[
    30_000,     // 30s
    60_000,     // 1m
    300_000,    // 5m
    900_000,    // 15m
    3_600_000,  // 1h
];

/// Compute backoff delay based on consecutive error count.
pub fn compute_backoff_ms(consecutive_errors: u32) -> i64 {
    if consecutive_errors == 0 {
        return 0;
    }
    let index = (consecutive_errors as usize).saturating_sub(1);
    BACKOFF_TIERS_MS[index.min(BACKOFF_TIERS_MS.len() - 1)]
}
```

- [ ] **Step 3: Write comprehensive tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // === Anchor-aligned interval tests ===

    #[test]
    fn anchor_aligned_basic() {
        let anchor = 0;
        let every = 30 * 60 * 1000; // 30min
        let now = 10 * 60 * 1000; // 10min after anchor
        let next = compute_next_every(now, every, anchor, None).unwrap();
        assert_eq!(next, 30 * 60 * 1000); // Should be 30min mark
    }

    #[test]
    fn anchor_aligned_no_drift_after_slow_execution() {
        let anchor = 0;
        let every = 30 * 60 * 1000;
        // 10:07 — execution of 10:00 slot just finished
        let now = (10 * 60 + 7 * 60) * 1000;
        let next = compute_next_every(now, every, anchor, Some(0)).unwrap();
        // Should be 10:30, not 10:37
        assert_eq!(next, 30 * 60 * 1000);
    }

    #[test]
    fn anchor_aligned_exactly_on_anchor() {
        let anchor = 1_000_000;
        let every = 60_000;
        let now = anchor; // Exactly at anchor
        let next = compute_next_every(now, every, anchor, None).unwrap();
        assert_eq!(next, anchor); // Fire at anchor point
    }

    #[test]
    fn anchor_aligned_future_anchor() {
        let anchor = 2_000_000;
        let now = 1_000_000; // Before anchor
        let next = compute_next_every(now, 60_000, anchor, None).unwrap();
        assert_eq!(next, anchor);
    }

    #[test]
    fn anchor_aligned_future_manual_trigger() {
        let now = 1_000_000;
        let last_run = 2_000_000; // In the future (manual trigger)
        let every = 60_000;
        let next = compute_next_every(now, every, 0, Some(last_run)).unwrap();
        assert_eq!(next, 2_060_000);
    }

    #[test]
    fn anchor_aligned_zero_interval() {
        let next = compute_next_every(1_000_000, 0, 0, None);
        assert!(next.is_none()); // Config error
    }

    #[test]
    fn anchor_aligned_negative_interval() {
        let next = compute_next_every(1_000_000, -1, 0, None);
        assert!(next.is_none());
    }

    // === Anti-spin safety net ===

    #[test]
    fn min_gap_prevents_spin() {
        let ended_at = 1_000_000;
        let computed_next = 1_000_500; // 500ms later — within MIN_REFIRE_GAP
        let safe_next = apply_min_gap(computed_next, Some(ended_at));
        assert!(safe_next >= ended_at + MIN_REFIRE_GAP_MS);
        assert_eq!(safe_next, ended_at + MIN_REFIRE_GAP_MS);
    }

    #[test]
    fn min_gap_no_effect_when_far_enough() {
        let ended_at = 1_000_000;
        let computed_next = 1_060_000; // 60s later — well past gap
        let safe_next = apply_min_gap(computed_next, Some(ended_at));
        assert_eq!(safe_next, computed_next); // No change
    }

    #[test]
    fn min_gap_no_last_ended() {
        let safe_next = apply_min_gap(1_000_000, None);
        assert_eq!(safe_next, 1_000_000); // No adjustment
    }

    // === Anchor resolution ===

    #[test]
    fn resolve_anchor_explicit() {
        assert_eq!(resolve_anchor(Some(42), 100), 42);
    }

    #[test]
    fn resolve_anchor_fallback_to_created() {
        assert_eq!(resolve_anchor(None, 100), 100);
    }

    // === Backoff ===

    #[test]
    fn backoff_zero_errors() {
        assert_eq!(compute_backoff_ms(0), 0);
    }

    #[test]
    fn backoff_tiers() {
        assert_eq!(compute_backoff_ms(1), 30_000);
        assert_eq!(compute_backoff_ms(2), 60_000);
        assert_eq!(compute_backoff_ms(3), 300_000);
        assert_eq!(compute_backoff_ms(4), 900_000);
        assert_eq!(compute_backoff_ms(5), 3_600_000);
    }

    #[test]
    fn backoff_clamps_at_max() {
        assert_eq!(compute_backoff_ms(100), 3_600_000);
    }

    // === Cron expression ===

    #[test]
    fn cron_next_basic() {
        let from = DateTime::parse_from_rfc3339("2026-03-14T10:00:00Z")
            .unwrap().with_timezone(&Utc);
        let next = compute_next_cron("0 0 11 * * *", None, from).unwrap();
        assert!(next.is_some());
        assert!(next.unwrap() > from.timestamp_millis());
    }

    #[test]
    fn cron_invalid_expression() {
        let from = Utc::now();
        let result = compute_next_cron("invalid", None, from);
        assert!(result.is_err());
    }

    #[test]
    fn cron_invalid_timezone() {
        let from = Utc::now();
        let result = compute_next_cron("0 0 * * * *", Some("Invalid/TZ"), from);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 4: Register module and run tests**

Add `pub mod schedule;` to `core/src/cron/mod.rs`.

Run: `cargo test -p alephcore --lib cron::schedule`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add core/src/cron/schedule.rs core/src/cron/mod.rs
git commit -m "cron: add pure schedule computation with anchor alignment"
```

---

### Task 4: Hash-Based Stagger

**Files:**
- Create: `core/src/cron/stagger.rs`
- Modify: `core/src/cron/mod.rs`

- [ ] **Step 1: Write stagger computation**

```rust
// core/src/cron/stagger.rs
use sha2::{Sha256, Digest};

/// Compute deterministic stagger offset from job ID using SHA-256.
///
/// Same job ID always gets same offset → stable schedule across restarts.
/// Distributes jobs uniformly across `[0, stagger_ms)` window.
pub fn compute_stagger_offset(job_id: &str, stagger_ms: i64) -> i64 {
    if stagger_ms <= 0 {
        return 0;
    }

    let mut hasher = Sha256::new();
    hasher.update(job_id.as_bytes());
    let digest = hasher.finalize();

    let hash_val = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    (hash_val as i64) % stagger_ms
}

/// Compute staggered next run time for cron schedule.
///
/// 1. Take natural cron-computed next time
/// 2. Add deterministic offset based on job ID hash
/// 3. If result is in the past, advance to next window
pub fn compute_staggered_next(
    job_id: &str,
    cron_next_ms: i64,
    stagger_ms: i64,
    now_ms: i64,
) -> i64 {
    if stagger_ms <= 0 {
        return cron_next_ms;
    }

    let offset = compute_stagger_offset(job_id, stagger_ms);
    let staggered = cron_next_ms + offset;

    if staggered > now_ms {
        staggered
    } else {
        // Current window passed; stagger into next natural cron window
        cron_next_ms + stagger_ms + offset
    }
}
```

- [ ] **Step 2: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stagger_deterministic() {
        let a = compute_stagger_offset("job-abc", 300_000);
        let b = compute_stagger_offset("job-abc", 300_000);
        assert_eq!(a, b);
    }

    #[test]
    fn stagger_within_range() {
        for id in ["a", "b", "c", "long-job-id-12345", "🦀"] {
            let offset = compute_stagger_offset(id, 300_000);
            assert!(offset >= 0 && offset < 300_000, "offset {} out of range for {}", offset, id);
        }
    }

    #[test]
    fn stagger_different_ids_likely_differ() {
        let a = compute_stagger_offset("job-aaa", 300_000);
        let b = compute_stagger_offset("job-zzz", 300_000);
        // Not guaranteed but extremely likely with SHA256
        assert_ne!(a, b);
    }

    #[test]
    fn stagger_zero_window() {
        assert_eq!(compute_stagger_offset("any", 0), 0);
    }

    #[test]
    fn stagger_negative_window() {
        assert_eq!(compute_stagger_offset("any", -1), 0);
    }

    #[test]
    fn staggered_next_future() {
        let now = 1_000_000;
        let cron_next = 1_100_000; // 100s in future
        let result = compute_staggered_next("job-1", cron_next, 60_000, now);
        assert!(result > now);
        assert!(result >= cron_next);
        assert!(result < cron_next + 60_000);
    }

    #[test]
    fn staggered_next_past_advances_window() {
        let now = 1_200_000;
        let cron_next = 1_100_000; // Already past
        let stagger_ms = 60_000;
        let result = compute_staggered_next("job-1", cron_next, stagger_ms, now);
        assert!(result > now, "staggered result {} should be > now {}", result, now);
    }

    #[test]
    fn staggered_next_zero_stagger_passthrough() {
        let cron_next = 1_100_000;
        assert_eq!(compute_staggered_next("any", cron_next, 0, 1_000_000), cron_next);
    }
}
```

- [ ] **Step 3: Register, check sha2 dependency, run tests**

Add `pub mod stagger;` to `core/src/cron/mod.rs`.

Check if `sha2` is already in `Cargo.toml`. If not, add it:
```bash
# Check existing deps
grep 'sha2' core/Cargo.toml
# If missing:
# Add sha2 = "0.10" to [dependencies] in core/Cargo.toml
```

Run: `cargo test -p alephcore --lib cron::stagger`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add core/src/cron/stagger.rs core/src/cron/mod.rs
git commit -m "cron: add SHA256 hash-based stagger spreading"
```

---

## Chunk 2: Persistence — JSON Atomic Store

### Task 5: JSON Atomic Store

**Files:**
- Create: `core/src/cron/store.rs`
- Modify: `core/src/cron/mod.rs`

- [ ] **Step 1: Write CronStoreFile and CronStore types**

```rust
// core/src/cron/store.rs
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::io::Write;
use serde::{Serialize, Deserialize};
use crate::cron::config::CronJob;

const CURRENT_VERSION: u32 = 1;

/// On-disk format for the cron job store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronStoreFile {
    pub version: u32,
    pub jobs: Vec<CronJob>,
}

/// In-memory cron store with dirty tracking and mtime-based reload.
pub struct CronStore {
    path: PathBuf,
    file: CronStoreFile,
    last_mtime: Option<SystemTime>,
    dirty: bool,
}
```

- [ ] **Step 2: Write atomic_write function**

```rust
// Append to store.rs
use rand::Rng; // rand 0.8 API

/// Atomic file write: tmp → fsync → rename.
/// Guarantees: any reader sees either old complete file or new complete file.
fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent directory")
    })?;

    // Ensure directory exists
    std::fs::create_dir_all(dir)?;

    // Step 1: Write to temp file
    let tmp_name = format!(
        "{}.{}.{:08x}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        rand::thread_rng().gen::<u32>()
    );
    let tmp_path = dir.join(&tmp_name);

    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(data)?;
        f.sync_all()?;
    }

    // Step 2: Backup existing file (best-effort)
    if path.exists() {
        let bak_path = path.with_extension("bak");
        let _ = std::fs::rename(path, &bak_path);
    }

    // Step 3: Atomic rename
    std::fs::rename(&tmp_path, path)
}
```

- [ ] **Step 3: Write CronStore implementation**

```rust
// Append to store.rs
impl CronStore {
    /// Load store from disk or create empty.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        if path.exists() {
            let data = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
            let mut file: CronStoreFile = serde_json::from_str(&data)
                .map_err(|e| format!("failed to parse {}: {}", path.display(), e))?;

            // Apply migrations
            if file.version < CURRENT_VERSION {
                migrate_store(&mut file);
                // Persist migrated version
                let data = serde_json::to_string_pretty(&file)
                    .map_err(|e| format!("failed to serialize: {}", e))?;
                atomic_write(&path, data.as_bytes())
                    .map_err(|e| format!("failed to write migrated store: {}", e))?;
            }

            let mtime = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok());

            Ok(Self { path, file, last_mtime: mtime, dirty: false })
        } else {
            // Check for .bak recovery
            let bak_path = path.with_extension("bak");
            if bak_path.exists() {
                tracing::warn!("recovering cron store from backup: {}", bak_path.display());
                std::fs::rename(&bak_path, &path)
                    .map_err(|e| format!("failed to recover from backup: {}", e))?;
                return Self::load(path);
            }

            Ok(Self {
                path,
                file: CronStoreFile { version: CURRENT_VERSION, jobs: vec![] },
                last_mtime: None,
                dirty: false,
            })
        }
    }

    /// Reload from disk if file has been modified externally.
    pub fn reload_if_changed(&mut self) -> Result<bool, String> {
        let current_mtime = std::fs::metadata(&self.path)
            .ok()
            .and_then(|m| m.modified().ok());

        if current_mtime == self.last_mtime {
            return Ok(false); // No change
        }

        if !self.path.exists() {
            return Ok(false);
        }

        let data = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("reload failed: {}", e))?;
        self.file = serde_json::from_str(&data)
            .map_err(|e| format!("reload parse failed: {}", e))?;
        self.last_mtime = current_mtime;
        self.dirty = false;

        Ok(true)
    }

    /// Force reload from disk (used in Phase 3 writeback).
    pub fn force_reload(&mut self) -> Result<(), String> {
        if !self.path.exists() {
            return Ok(());
        }
        let data = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("force reload failed: {}", e))?;
        self.file = serde_json::from_str(&data)
            .map_err(|e| format!("force reload parse failed: {}", e))?;
        self.last_mtime = std::fs::metadata(&self.path).ok().and_then(|m| m.modified().ok());
        self.dirty = false;
        Ok(())
    }

    /// Persist to disk if dirty.
    pub fn persist(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }

        let data = serde_json::to_string_pretty(&self.file)
            .map_err(|e| format!("serialize failed: {}", e))?;
        atomic_write(&self.path, data.as_bytes())
            .map_err(|e| format!("persist failed: {}", e))?;

        self.last_mtime = std::fs::metadata(&self.path).ok().and_then(|m| m.modified().ok());
        self.dirty = false;
        Ok(())
    }

    /// Mark store as dirty (needs persist).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    // --- Job accessors ---

    pub fn jobs(&self) -> &[CronJob] {
        &self.file.jobs
    }

    pub fn jobs_mut(&mut self) -> &mut Vec<CronJob> {
        self.dirty = true;
        &mut self.file.jobs
    }

    pub fn get_job(&self, id: &str) -> Option<&CronJob> {
        self.file.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_job_mut(&mut self, id: &str) -> Option<&mut CronJob> {
        self.dirty = true;
        self.file.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn add_job(&mut self, job: CronJob) {
        self.dirty = true;
        self.file.jobs.push(job);
    }

    pub fn remove_job(&mut self, id: &str) -> Option<CronJob> {
        self.dirty = true;
        if let Some(pos) = self.file.jobs.iter().position(|j| j.id == id) {
            Some(self.file.jobs.remove(pos))
        } else {
            None
        }
    }

    pub fn job_count(&self) -> usize {
        self.file.jobs.len()
    }
}

/// Idempotent store migration.
fn migrate_store(file: &mut CronStoreFile) {
    // Version 0 → 1: normalize legacy fields (if any)
    if file.version == 0 {
        file.version = 1;
    }
    // Future migrations go here
}
```

- [ ] **Step 4: Write store tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store_path(dir: &TempDir) -> PathBuf {
        dir.path().join("cron").join("jobs.json")
    }

    fn make_test_job(id: &str) -> CronJob {
        let mut job = CronJob::new(id.to_string(), "test".to_string(), "agent".to_string(), "prompt".to_string());
        job.id = id.to_string();
        job
    }

    #[test]
    fn load_empty_creates_new_store() {
        let dir = TempDir::new().unwrap();
        let store = CronStore::load(test_store_path(&dir)).unwrap();
        assert_eq!(store.job_count(), 0);
        assert_eq!(store.file.version, CURRENT_VERSION);
    }

    #[test]
    fn add_persist_reload() {
        let dir = TempDir::new().unwrap();
        let path = test_store_path(&dir);

        // Add and persist
        {
            let mut store = CronStore::load(path.clone()).unwrap();
            store.add_job(make_test_job("j1"));
            store.persist().unwrap();
        }

        // Reload
        {
            let store = CronStore::load(path).unwrap();
            assert_eq!(store.job_count(), 1);
            assert_eq!(store.get_job("j1").unwrap().id, "j1");
        }
    }

    #[test]
    fn remove_job() {
        let dir = TempDir::new().unwrap();
        let mut store = CronStore::load(test_store_path(&dir)).unwrap();
        store.add_job(make_test_job("j1"));
        store.add_job(make_test_job("j2"));
        let removed = store.remove_job("j1");
        assert!(removed.is_some());
        assert_eq!(store.job_count(), 1);
        assert!(store.get_job("j1").is_none());
    }

    #[test]
    fn persist_skips_when_not_dirty() {
        let dir = TempDir::new().unwrap();
        let mut store = CronStore::load(test_store_path(&dir)).unwrap();
        // Not dirty — persist is a no-op (file won't exist)
        store.persist().unwrap();
        assert!(!store.path.exists());
    }

    #[test]
    fn bak_recovery() {
        let dir = TempDir::new().unwrap();
        let path = test_store_path(&dir);
        let bak_path = path.with_extension("bak");

        // Create a .bak file
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let data = serde_json::to_string(&CronStoreFile {
            version: 1,
            jobs: vec![make_test_job("recovered")],
        }).unwrap();
        std::fs::write(&bak_path, data).unwrap();

        // Load should recover from bak
        let store = CronStore::load(path.clone()).unwrap();
        assert_eq!(store.job_count(), 1);
        assert_eq!(store.get_job("recovered").unwrap().id, "recovered");
        // Original path should now exist, bak removed
        assert!(path.exists());
    }

    #[test]
    fn force_reload_picks_up_external_changes() {
        let dir = TempDir::new().unwrap();
        let path = test_store_path(&dir);

        let mut store = CronStore::load(path.clone()).unwrap();
        store.add_job(make_test_job("j1"));
        store.persist().unwrap();

        // Externally modify file
        let mut file: CronStoreFile = serde_json::from_str(
            &std::fs::read_to_string(&path).unwrap()
        ).unwrap();
        file.jobs.push(make_test_job("j2"));
        std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();

        // Force reload
        store.force_reload().unwrap();
        assert_eq!(store.job_count(), 2);
    }
}
```

- [ ] **Step 5: Register module, add tempfile dev-dependency if needed, run tests**

Add `pub mod store;` to `core/src/cron/mod.rs`.

Check/add `tempfile` dev-dependency:
```bash
grep 'tempfile' core/Cargo.toml
# If missing, add: tempfile = "3" under [dev-dependencies]
```

Run: `cargo test -p alephcore --lib cron::store`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add core/src/cron/store.rs core/src/cron/mod.rs
git commit -m "cron: add JSON atomic persistence with crash recovery"
```

---

## Chunk 3: Service Layer

### Task 6: Service State Container

**Files:**
- Create: `core/src/cron/service/mod.rs`
- Create: `core/src/cron/service/state.rs`
- Modify: `core/src/cron/mod.rs`

- [ ] **Step 1: Create service module facade**

```rust
// core/src/cron/service/mod.rs
// Only declare state initially. Other submodules are added by their respective tasks.
pub mod state;

pub use state::ServiceState;
```

Note: Task 7 adds `pub mod ops;`, Task 8 adds `pub mod concurrency;`, Task 9 adds `pub mod catchup;`, Task 10 adds `pub mod timer;`. Each task adds its own module declaration to prevent compilation failures.

- [ ] **Step 2: Write ServiceState**

```rust
// core/src/cron/service/state.rs
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use crate::cron::clock::Clock;
use crate::cron::config::CronConfig;
use crate::cron::store::CronStore;

/// Runtime state container for the cron service.
/// Generic over Clock for testability.
pub struct ServiceState<C: Clock> {
    pub store: Arc<Mutex<CronStore>>,
    pub clock: Arc<C>,
    pub config: CronConfig,
    is_running: AtomicBool,
    shutdown: AtomicBool,
}

impl<C: Clock> ServiceState<C> {
    pub fn new(store: CronStore, clock: C, config: CronConfig) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            clock: Arc::new(clock),
            config,
            is_running: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    pub fn set_running(&self, running: bool) {
        self.is_running.store(running, Ordering::SeqCst);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}
```

- [ ] **Step 3: Register service module**

Add `pub mod service;` to `core/src/cron/mod.rs`.

- [ ] **Step 4: Run compile check**

Run: `cargo check -p alephcore`
Expected: Compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add core/src/cron/service/
git commit -m "cron: add ServiceState container for cron service"
```

---

### Task 7: Service Operations (CRUD + Zero Side-Effect Reads)

**Files:**
- Create: `core/src/cron/service/ops.rs`

- [ ] **Step 1: Write CRUD operations with zero side-effect reads**

```rust
// core/src/cron/service/ops.rs
use crate::cron::clock::Clock;
use crate::cron::config::{CronJob, CronJobView, JobStateV2, ScheduleKind};
use crate::cron::schedule::{compute_next_every, compute_next_cron, resolve_anchor, apply_min_gap};
use crate::cron::stagger::compute_staggered_next;
use crate::cron::store::CronStore;
use chrono::{DateTime, Utc};

/// Full recompute: advance next_run_at_ms to future.
/// Called by: add, update, toggle.
pub fn recompute_next_run_full(job: &mut CronJob, clock: &dyn Clock) {
    let now = clock.now_ms();
    job.state.next_run_at_ms = compute_next_run_for_job(job, now);
}

/// Maintenance recompute: only fill missing next_run_at_ms.
/// Called by: timer tick, Phase 3 writeback.
/// NEVER modifies existing values — prevents silently skipping past-due jobs.
pub fn recompute_next_run_maintenance(job: &mut CronJob, clock: &dyn Clock) {
    if job.state.next_run_at_ms.is_some() {
        return; // Don't touch existing schedule
    }
    let now = clock.now_ms();
    job.state.next_run_at_ms = compute_next_run_for_job(job, now);
}

/// Compute next run time based on job's schedule kind.
fn compute_next_run_for_job(job: &CronJob, now_ms: i64) -> Option<i64> {
    match &job.schedule_kind {
        ScheduleKind::Every { every_ms, anchor_ms } => {
            let anchor = resolve_anchor(*anchor_ms, job.created_at);
            compute_next_every(now_ms, *every_ms, anchor, job.state.last_run_at_ms)
        }
        ScheduleKind::Cron { expr, tz, stagger_ms } => {
            let from = DateTime::from_timestamp_millis(now_ms)
                .unwrap_or_else(|| Utc::now());
            match compute_next_cron(expr, tz.as_deref(), from) {
                Ok(Some(next)) => {
                    let staggered = match stagger_ms {
                        Some(s) if *s > 0 => compute_staggered_next(&job.id, next, *s, now_ms),
                        _ => next,
                    };
                    // Use last_ended_at_ms (not last_run_at_ms which is start time)
                    let last_ended = job.state.last_run_at_ms
                        .zip(job.state.last_duration_ms)
                        .map(|(start, dur)| start + dur);
                    Some(apply_min_gap(staggered, last_ended))
                }
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(job_id = %job.id, error = %e, "schedule computation error");
                    None
                }
            }
        }
        ScheduleKind::At { at, .. } => {
            if *at > now_ms && job.state.last_run_at_ms.is_none() {
                Some(*at)
            } else {
                None // Already ran or past
            }
        }
    }
}

// --- Read operations (ZERO side effects) ---

/// List all jobs as read-only views. Never mutates state.
pub fn list_jobs(store: &CronStore) -> Vec<CronJobView> {
    store.jobs().iter().map(CronJobView::from).collect()
}

/// Get a single job as read-only view. Never mutates state.
pub fn get_job(store: &CronStore, id: &str) -> Option<CronJobView> {
    store.get_job(id).map(CronJobView::from)
}

// --- Write operations ---

/// Add a new job. Computes initial next_run_at_ms.
pub fn add_job(store: &mut CronStore, mut job: CronJob, clock: &dyn Clock) -> String {
    let id = job.id.clone();
    job.state = JobStateV2::default();
    recompute_next_run_full(&mut job, clock);
    store.add_job(job);
    id
}

/// Update an existing job. Recomputes next_run_at_ms.
pub fn update_job(store: &mut CronStore, id: &str, updates: CronJobUpdates, clock: &dyn Clock) -> Result<(), String> {
    let job = store.get_job_mut(id)
        .ok_or_else(|| format!("job not found: {}", id))?;

    // Apply updates
    if let Some(name) = updates.name { job.name = name; }
    if let Some(schedule) = updates.schedule_kind { job.schedule_kind = schedule; }
    if let Some(prompt) = updates.prompt { job.prompt = prompt; }
    if let Some(agent_id) = updates.agent_id { job.agent_id = agent_id; }
    if let Some(enabled) = updates.enabled { job.enabled = enabled; }
    if let Some(timezone) = updates.timezone { job.timezone = timezone; }
    if let Some(delivery) = updates.delivery_config { job.delivery_config = delivery; }
    if let Some(alert) = updates.failure_alert { job.failure_alert = alert; }
    job.updated_at = clock.now_ms();

    recompute_next_run_full(job, clock);
    Ok(())
}

/// Toggle job enabled state.
pub fn toggle_job(store: &mut CronStore, id: &str, clock: &dyn Clock) -> Result<bool, String> {
    let job = store.get_job_mut(id)
        .ok_or_else(|| format!("job not found: {}", id))?;
    job.enabled = !job.enabled;
    job.updated_at = clock.now_ms();
    if job.enabled {
        recompute_next_run_full(job, clock);
    }
    Ok(job.enabled)
}

/// Delete a job.
pub fn delete_job(store: &mut CronStore, id: &str) -> Result<(), String> {
    store.remove_job(id)
        .ok_or_else(|| format!("job not found: {}", id))?;
    Ok(())
}

/// Partial update struct for job updates.
#[derive(Debug, Default)]
pub struct CronJobUpdates {
    pub name: Option<String>,
    pub schedule_kind: Option<ScheduleKind>,
    pub prompt: Option<String>,
    pub agent_id: Option<String>,
    pub enabled: Option<bool>,
    pub timezone: Option<Option<String>>,
    pub delivery_config: Option<Option<DeliveryConfig>>,
    pub failure_alert: Option<Option<FailureAlertConfig>>,
}
```

- [ ] **Step 2: Write tests for zero side-effect reads**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;
    use crate::cron::store::CronStore;
    use tempfile::TempDir;

    fn setup() -> (TempDir, CronStore, FakeClock) {
        let dir = TempDir::new().unwrap();
        let store = CronStore::load(dir.path().join("jobs.json")).unwrap();
        let clock = FakeClock::new(1_000_000_000);
        (dir, store, clock)
    }

    #[test]
    fn list_jobs_zero_side_effects() {
        let (_dir, mut store, clock) = setup();
        let mut job = CronJob::new(/*...*/);
        job.state.next_run_at_ms = Some(500_000); // Past due
        store.add_job(job);

        // Snapshot before
        let before = store.jobs()[0].state.next_run_at_ms;

        // Read operation
        let _views = list_jobs(&store);

        // State unchanged
        let after = store.jobs()[0].state.next_run_at_ms;
        assert_eq!(before, after, "list_jobs must not modify state");
    }

    #[test]
    fn get_job_zero_side_effects() {
        let (_dir, mut store, _clock) = setup();
        let mut job = CronJob::new(/*...*/);
        job.id = "test".to_string();
        job.state.next_run_at_ms = Some(500_000);
        store.add_job(job);

        let before = store.get_job("test").unwrap().state.next_run_at_ms;
        let _view = get_job(&store, "test");
        let after = store.get_job("test").unwrap().state.next_run_at_ms;
        assert_eq!(before, after, "get_job must not modify state");
    }

    #[test]
    fn add_job_computes_next_run() {
        let (_dir, mut store, clock) = setup();
        let mut job = CronJob::new(/*...*/);
        job.schedule_kind = ScheduleKind::Every { every_ms: 60_000, anchor_ms: None };
        add_job(&mut store, job, &clock);
        assert!(store.jobs()[0].state.next_run_at_ms.is_some());
    }

    #[test]
    fn maintenance_recompute_preserves_past_due() {
        let (_dir, _store, clock) = setup();
        let mut job = CronJob::new(/*...*/);
        job.schedule_kind = ScheduleKind::Every { every_ms: 60_000, anchor_ms: None };
        job.state.next_run_at_ms = Some(500_000); // Past due

        recompute_next_run_maintenance(&mut job, &clock);
        assert_eq!(job.state.next_run_at_ms, Some(500_000), "maintenance must not advance past-due");
    }

    #[test]
    fn full_recompute_advances_past_due() {
        let (_dir, _store, clock) = setup();
        let mut job = CronJob::new(/*...*/);
        job.schedule_kind = ScheduleKind::Every { every_ms: 60_000, anchor_ms: None };
        job.state.next_run_at_ms = Some(500_000); // Past due

        recompute_next_run_full(&mut job, &clock);
        assert!(job.state.next_run_at_ms.unwrap() >= clock.now_ms(), "full recompute must advance to future");
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib cron::service::ops`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add core/src/cron/service/ops.rs
git commit -m "cron: add service operations with zero side-effect reads"
```

---

### Task 8: Three-Phase Concurrency Model

**Files:**
- Create: `core/src/cron/service/concurrency.rs`

- [ ] **Step 1: Write three-phase execution model**

```rust
// core/src/cron/service/concurrency.rs
use crate::cron::clock::Clock;
use crate::cron::config::{
    CronJob, ExecutionResult, JobSnapshot, RunStatus, SessionTarget, TriggerSource,
};
use crate::cron::service::ops::recompute_next_run_maintenance;
use crate::cron::store::CronStore;
use tokio::sync::Mutex;
use std::sync::Arc;

/// Phase 1: Under lock — check, mark, snapshot.
/// Returns snapshots of runnable jobs, or empty vec if none.
pub async fn phase1_mark_due_jobs<C: Clock>(
    store: &Arc<Mutex<CronStore>>,
    clock: &C,
) -> Result<Vec<JobSnapshot>, String> {
    let mut guard = store.lock().await;
    guard.reload_if_changed()?;

    let now = clock.now_ms();
    let mut snapshots = Vec::new();

    for job in guard.jobs_mut() {
        if !job.enabled { continue; }
        if job.state.running_at_ms.is_some() { continue; }

        if let Some(next) = job.state.next_run_at_ms {
            if next <= now {
                // Mark running
                job.state.running_at_ms = Some(now);

                snapshots.push(JobSnapshot {
                    id: job.id.clone(),
                    agent_id: Some(job.agent_id.clone()),
                    prompt: job.prompt.clone(), // TODO: template rendering
                    model: None, // TODO: from job config
                    timeout_ms: Some(job.timeout_ms()),
                    delivery: job.delivery_config.clone(),
                    session_target: job.session_target.clone(),
                    marked_at: now,
                    trigger_source: TriggerSource::Schedule,
                });
            }
        }
    }

    if !snapshots.is_empty() {
        guard.persist()?;
    }

    Ok(snapshots)
}

/// Phase 3: Under lock — reload, merge results, maintenance recompute, persist.
pub async fn phase3_writeback<C: Clock>(
    store: &Arc<Mutex<CronStore>>,
    clock: &C,
    results: &[(String, ExecutionResult)],
) -> Result<(), String> {
    if results.is_empty() {
        return Ok(());
    }

    let mut guard = store.lock().await;

    // Force reload to capture concurrent edits
    guard.force_reload()?;

    for (job_id, result) in results {
        let job = match guard.get_job_mut(job_id) {
            Some(j) => j,
            None => {
                // Job deleted during execution — discard result, log warning
                tracing::warn!(job_id = %job_id, "job deleted during execution, discarding result");
                continue;
            }
        };

        // Clear running marker
        job.state.running_at_ms = None;

        // Write execution results (always — these are facts)
        job.state.last_run_at_ms = Some(result.started_at);
        job.state.last_run_status = Some(result.status.clone());
        job.state.last_duration_ms = Some(result.duration_ms);
        job.state.last_error = result.error.clone();
        job.state.last_error_reason = result.error_reason.clone();
        job.state.last_delivery_status = result.delivery_status.clone();

        // Update consecutive errors
        match &result.status {
            RunStatus::Ok => {
                job.state.consecutive_errors = 0;
            }
            RunStatus::Error | RunStatus::Timeout => {
                job.state.consecutive_errors += 1;
            }
            RunStatus::Skipped => {} // Don't change counter
        }

        // Clear next_run_at_ms so maintenance recompute fills it
        job.state.next_run_at_ms = None;
    }

    // Maintenance recompute for ALL jobs (don't advance past-due)
    for job in guard.jobs_mut() {
        recompute_next_run_maintenance(job, clock);
    }

    guard.persist()?;
    Ok(())
}

/// Phase 1 for manual trigger: mark a specific job as running.
pub async fn phase1_mark_manual<C: Clock>(
    store: &Arc<Mutex<CronStore>>,
    clock: &C,
    job_id: &str,
) -> Result<Option<JobSnapshot>, String> {
    let mut guard = store.lock().await;
    guard.reload_if_changed()?;

    let job = match guard.get_job_mut(job_id) {
        Some(j) => j,
        None => return Err(format!("job not found: {}", job_id)),
    };

    if job.state.running_at_ms.is_some() {
        return Ok(None); // Already running
    }

    let now = clock.now_ms();
    job.state.running_at_ms = Some(now);

    let snapshot = JobSnapshot {
        id: job.id.clone(),
        agent_id: Some(job.agent_id.clone()),
        prompt: job.prompt.clone(),
        model: None,
        timeout_ms: Some(job.timeout_ms()),
        delivery: job.delivery_config.clone(),
        session_target: job.session_target.clone(),
        marked_at: now,
        trigger_source: TriggerSource::Manual,
    };

    guard.persist()?;
    Ok(Some(snapshot))
}
```

- [ ] **Step 2: Write concurrency tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;
    use crate::cron::store::CronStore;
    use tempfile::TempDir;

    async fn setup() -> (TempDir, Arc<Mutex<CronStore>>, Arc<FakeClock>) {
        let dir = TempDir::new().unwrap();
        let store = CronStore::load(dir.path().join("jobs.json")).unwrap();
        let clock = FakeClock::new(1_000_000_000);
        (dir, Arc::new(Mutex::new(store)), Arc::new(clock))
    }

    #[tokio::test]
    async fn phase1_marks_due_jobs() {
        let (_dir, store, clock) = setup().await;
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("j1");
            job.enabled = true;
            job.state.next_run_at_ms = Some(clock.now_ms() - 1000); // Past due
            guard.add_job(job);
        }

        let snapshots = phase1_mark_due_jobs(&store, clock.as_ref()).await.unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, "j1");

        // Verify running_at_ms is set
        let guard = store.lock().await;
        assert!(guard.get_job("j1").unwrap().state.running_at_ms.is_some());
    }

    #[tokio::test]
    async fn phase1_skips_already_running() {
        let (_dir, store, clock) = setup().await;
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("j1");
            job.enabled = true;
            job.state.next_run_at_ms = Some(clock.now_ms() - 1000);
            job.state.running_at_ms = Some(clock.now_ms()); // Already running
            guard.add_job(job);
        }

        let snapshots = phase1_mark_due_jobs(&store, clock.as_ref()).await.unwrap();
        assert!(snapshots.is_empty());
    }

    #[tokio::test]
    async fn phase3_merges_results() {
        let (_dir, store, clock) = setup().await;
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("j1");
            job.state.running_at_ms = Some(clock.now_ms());
            guard.add_job(job);
            guard.persist().unwrap();
        }

        let results = vec![(
            "j1".to_string(),
            ExecutionResult {
                started_at: clock.now_ms(),
                ended_at: clock.now_ms() + 1000,
                duration_ms: 1000,
                status: RunStatus::Ok,
                output: Some("done".to_string()),
                error: None,
                error_reason: None,
                delivery_status: None,
                agent_used_messaging_tool: false,
            },
        )];

        phase3_writeback(&store, clock.as_ref(), &results).await.unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("j1").unwrap();
        assert!(job.state.running_at_ms.is_none()); // Cleared
        assert_eq!(job.state.consecutive_errors, 0);
        assert_eq!(job.state.last_run_status, Some(RunStatus::Ok));
    }

    #[tokio::test]
    async fn phase3_handles_deleted_job() {
        let (_dir, store, clock) = setup().await;
        // Job exists in memory but gets removed from disk during "execution"
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("j1");
            job.state.running_at_ms = Some(clock.now_ms());
            guard.add_job(job);
            guard.persist().unwrap();
            // Simulate deletion: remove from disk
            guard.remove_job("j1");
            guard.persist().unwrap();
        }

        let results = vec![(
            "j1".to_string(),
            ExecutionResult {
                started_at: clock.now_ms(),
                ended_at: clock.now_ms() + 1000,
                duration_ms: 1000,
                status: RunStatus::Ok,
                output: None, error: None, error_reason: None,
                delivery_status: None, agent_used_messaging_tool: false,
            },
        )];

        // Should not panic — gracefully discards result
        phase3_writeback(&store, clock.as_ref(), &results).await.unwrap();
    }

    #[tokio::test]
    async fn phase3_increments_consecutive_errors() {
        let (_dir, store, clock) = setup().await;
        {
            let mut guard = store.lock().await;
            let mut job = make_test_job("j1");
            job.state.running_at_ms = Some(clock.now_ms());
            job.state.consecutive_errors = 2;
            guard.add_job(job);
            guard.persist().unwrap();
        }

        let results = vec![(
            "j1".to_string(),
            ExecutionResult {
                started_at: clock.now_ms(),
                ended_at: clock.now_ms() + 1000,
                duration_ms: 1000,
                status: RunStatus::Error,
                output: None,
                error: Some("timeout".to_string()),
                error_reason: Some(ErrorReason::Transient("timeout".to_string())),
                delivery_status: None, agent_used_messaging_tool: false,
            },
        )];

        phase3_writeback(&store, clock.as_ref(), &results).await.unwrap();

        let guard = store.lock().await;
        let job = guard.get_job("j1").unwrap();
        assert_eq!(job.state.consecutive_errors, 3);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib cron::service::concurrency`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add core/src/cron/service/concurrency.rs
git commit -m "cron: add three-phase concurrency model with MVCC merge"
```

---

### Task 9: Restart Catchup

**Files:**
- Create: `core/src/cron/service/catchup.rs`

- [ ] **Step 1: Write catchup implementation**

```rust
// core/src/cron/service/catchup.rs
use crate::cron::clock::Clock;
use crate::cron::store::CronStore;
use tokio::sync::Mutex;
use std::sync::Arc;

/// Default maximum missed jobs to execute immediately on restart.
const DEFAULT_MAX_MISSED_PER_RESTART: usize = 5;
/// Default stagger interval for deferred missed jobs (ms).
const DEFAULT_CATCHUP_STAGGER_MS: i64 = 5_000;
/// Stale running marker threshold (ms). Markers older than this are cleared.
const DEFAULT_STALE_THRESHOLD_MS: i64 = 7_200_000; // 2 hours

/// Run startup catchup: clear stale markers and stagger missed jobs.
/// Called once at service startup, before the timer loop begins.
pub async fn run_startup_catchup<C: Clock>(
    store: &Arc<Mutex<CronStore>>,
    clock: &C,
    max_missed: Option<usize>,
    stagger_ms: Option<i64>,
) -> Result<CatchupReport, String> {
    let max_immediate = max_missed.unwrap_or(DEFAULT_MAX_MISSED_PER_RESTART);
    let stagger_interval = stagger_ms.unwrap_or(DEFAULT_CATCHUP_STAGGER_MS);
    let now = clock.now_ms();

    let mut guard = store.lock().await;
    guard.reload_if_changed()?;

    let mut report = CatchupReport::default();

    // Step 1: Clear stale running markers
    for job in guard.jobs_mut() {
        if let Some(running_at) = job.state.running_at_ms {
            let timeout_ms = job.timeout_ms().unwrap_or(0);
            let threshold = DEFAULT_STALE_THRESHOLD_MS.max(timeout_ms * 2);
            if now - running_at > threshold {
                job.state.running_at_ms = None;
                report.stale_markers_cleared += 1;
                tracing::warn!(
                    job_id = %job.id,
                    running_since_ms = running_at,
                    "cleared stale running marker on startup"
                );
            }
        }
    }

    // Step 2: Collect missed jobs
    let mut missed: Vec<(String, i64)> = Vec::new();
    for job in guard.jobs() {
        if !job.enabled { continue; }
        if job.state.running_at_ms.is_some() { continue; }
        if let Some(next) = job.state.next_run_at_ms {
            if next <= now {
                missed.push((job.id.clone(), next));
            }
        }
    }

    // Step 3: Sort by overdue time (most overdue first)
    missed.sort_by_key(|(_, next)| *next);

    // Step 4: Split into immediate and deferred
    let split_at = max_immediate.min(missed.len());
    report.immediate_count = split_at;
    report.deferred_count = missed.len() - split_at;

    // Immediate: keep next_run_at_ms as-is (timer will pick up on first tick)
    // Deferred: stagger
    for (i, (job_id, _)) in missed.iter().enumerate().skip(split_at) {
        if let Some(job) = guard.get_job_mut(job_id) {
            let offset = (i - split_at + 1) as i64;
            job.state.next_run_at_ms = Some(now + offset * stagger_interval);
        }
    }

    if report.has_changes() {
        guard.persist()?;
    }

    tracing::info!(
        immediate = report.immediate_count,
        deferred = report.deferred_count,
        stale_cleared = report.stale_markers_cleared,
        "startup catchup complete"
    );

    Ok(report)
}

#[derive(Debug, Default)]
pub struct CatchupReport {
    pub stale_markers_cleared: usize,
    pub immediate_count: usize,
    pub deferred_count: usize,
}

impl CatchupReport {
    fn has_changes(&self) -> bool {
        self.stale_markers_cleared > 0 || self.deferred_count > 0
    }
}
```

- [ ] **Step 2: Write catchup tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;

    #[tokio::test]
    async fn clears_stale_running_markers() {
        // Setup: job with running_at_ms 3 hours ago
        let clock = FakeClock::new(10_000_000);
        // ... create store with stale job ...
        let report = run_startup_catchup(&store, &clock, None, None).await.unwrap();
        assert_eq!(report.stale_markers_cleared, 1);
    }

    #[tokio::test]
    async fn staggers_deferred_missed_jobs() {
        // Setup: 8 missed jobs
        // ... create store with 8 past-due jobs ...
        let report = run_startup_catchup(&store, &clock, Some(3), Some(5000)).await.unwrap();
        assert_eq!(report.immediate_count, 3);
        assert_eq!(report.deferred_count, 5);
        // Verify stagger: deferred[0] at now+5s, deferred[1] at now+10s, etc.
    }

    #[tokio::test]
    async fn no_changes_when_nothing_missed() {
        // Setup: all jobs in future
        let report = run_startup_catchup(&store, &clock, None, None).await.unwrap();
        assert_eq!(report.immediate_count, 0);
        assert_eq!(report.deferred_count, 0);
        assert_eq!(report.stale_markers_cleared, 0);
    }
}
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test -p alephcore --lib cron::service::catchup`

```bash
git add core/src/cron/service/catchup.rs
git commit -m "cron: add restart catchup with stale marker clearing and stagger"
```

---

### Task 10: Timer Loop + Worker Pool

**Files:**
- Create: `core/src/cron/service/timer.rs`

- [ ] **Step 1: Write timer loop skeleton**

```rust
// core/src/cron/service/timer.rs
use std::sync::Arc;
use std::collections::VecDeque;
use tokio::sync::Mutex;
use crate::cron::clock::Clock;
use crate::cron::config::{ExecutionResult, JobSnapshot, SessionTarget, RunStatus};
use crate::cron::service::state::ServiceState;
use crate::cron::service::concurrency::{phase1_mark_due_jobs, phase3_writeback};

/// Maximum timer delay in seconds. Prevents drift if process paused.
const MAX_TIMER_DELAY_SECS: u64 = 60;

/// Executor callback type: takes a snapshot, returns result.
pub type JobExecutorFn = Arc<dyn Fn(JobSnapshot) -> futures::future::BoxFuture<'static, ExecutionResult> + Send + Sync>;

/// Start the timer loop. Runs until shutdown is requested.
pub async fn run_timer_loop<C: Clock>(
    state: Arc<ServiceState<C>>,
    executor: JobExecutorFn,
) {
    loop {
        if state.is_shutdown() {
            tracing::info!("cron timer loop shutting down");
            break;
        }

        let interval = state.config.check_interval_secs.min(MAX_TIMER_DELAY_SECS);
        tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;

        if state.is_shutdown() { break; }

        // Skip if previous tick is still running
        if state.is_running() {
            tracing::debug!("cron timer: previous tick still running, skipping");
            continue;
        }

        state.set_running(true);

        if let Err(e) = on_timer_tick(&state, &executor).await {
            tracing::error!(error = %e, "cron timer tick failed");
        }

        state.set_running(false);
    }
}

/// Single timer tick: find due jobs → execute → writeback.
async fn on_timer_tick<C: Clock>(
    state: &ServiceState<C>,
    executor: &JobExecutorFn,
) -> Result<(), String> {
    // Phase 1: Mark due jobs
    let snapshots = phase1_mark_due_jobs(&state.store, state.clock.as_ref()).await?;

    if snapshots.is_empty() {
        return Ok(());
    }

    tracing::info!(count = snapshots.len(), "executing due cron jobs");

    // Split by session target
    let (main_jobs, isolated_jobs): (Vec<_>, Vec<_>) = snapshots
        .into_iter()
        .partition(|s| matches!(s.session_target, SessionTarget::Main));

    let mut all_results = Vec::new();

    // Main jobs: spawn each (unlimited concurrency)
    let main_handles: Vec<_> = main_jobs.into_iter().map(|snapshot| {
        let exec = executor.clone();
        let id = snapshot.id.clone();
        tokio::spawn(async move {
            let result = exec(snapshot).await;
            (id, result)
        })
    }).collect();

    // Isolated jobs: worker pool with concurrency limit
    let max_agents = state.config.max_concurrent_agents.unwrap_or(2);
    let isolated_results = run_worker_pool(isolated_jobs, max_agents, executor).await;

    // Collect main results
    for handle in main_handles {
        match handle.await {
            Ok((id, result)) => all_results.push((id, result)),
            Err(e) => tracing::error!(error = %e, "main job task panicked"),
        }
    }
    all_results.extend(isolated_results);

    // Phase 3: Write back all results
    phase3_writeback(&state.store, state.clock.as_ref(), &all_results).await?;

    Ok(())
}

/// Worker pool: shared queue, workers pull from it.
async fn run_worker_pool(
    jobs: Vec<JobSnapshot>,
    max_workers: usize,
    executor: &JobExecutorFn,
) -> Vec<(String, ExecutionResult)> {
    if jobs.is_empty() {
        return Vec::new();
    }

    let queue = Arc::new(std::sync::Mutex::new(VecDeque::from(jobs)));
    let results = Arc::new(std::sync::Mutex::new(Vec::new()));
    let worker_count = max_workers.min(queue.lock().unwrap().len());

    let mut handles = Vec::new();
    for _ in 0..worker_count {
        let q = queue.clone();
        let r = results.clone();
        let exec = executor.clone();

        handles.push(tokio::spawn(async move {
            loop {
                let snapshot = {
                    let mut q = q.lock().unwrap();
                    q.pop_front()
                };
                let snapshot = match snapshot {
                    Some(s) => s,
                    None => break,
                };

                let id = snapshot.id.clone();
                let result = exec(snapshot).await;
                r.lock().unwrap().push((id, result));
            }
        }));
    }

    futures::future::join_all(handles).await;
    Arc::try_unwrap(results).unwrap().into_inner().unwrap()
}
```

- [ ] **Step 2: Write timer tests with mock executor**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;

    fn mock_executor(status: RunStatus) -> JobExecutorFn {
        Arc::new(move |snapshot: JobSnapshot| {
            let status = status.clone();
            Box::pin(async move {
                ExecutionResult {
                    started_at: snapshot.marked_at,
                    ended_at: snapshot.marked_at + 100,
                    duration_ms: 100,
                    status,
                    output: Some("test output".to_string()),
                    error: None,
                    error_reason: None,
                    delivery_status: None,
                    agent_used_messaging_tool: false,
                }
            })
        })
    }

    #[tokio::test]
    async fn worker_pool_processes_all_jobs() {
        let jobs = (0..5).map(|i| JobSnapshot {
            id: format!("job-{}", i),
            // ... fill required fields ...
        }).collect();

        let executor = mock_executor(RunStatus::Ok);
        let results = run_worker_pool(jobs, 2, &executor).await;
        assert_eq!(results.len(), 5);
    }

    #[tokio::test]
    async fn worker_pool_empty_input() {
        let executor = mock_executor(RunStatus::Ok);
        let results = run_worker_pool(vec![], 2, &executor).await;
        assert!(results.is_empty());
    }
}
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test -p alephcore --lib cron::service::timer`

```bash
git add core/src/cron/service/timer.rs
git commit -m "cron: add timer loop with dual-path worker pool"
```

---

## Chunk 4: Execution, Delivery Enhancement, Alert

### Task 11: Execution Module (Lightweight + Isolated)

**Files:**
- Create: `core/src/cron/execution/mod.rs`
- Create: `core/src/cron/execution/lightweight.rs`
- Create: `core/src/cron/execution/isolated.rs`
- Modify: `core/src/cron/mod.rs`

- [ ] **Step 1: Create execution module facade**

```rust
// core/src/cron/execution/mod.rs
pub mod lightweight;
pub mod isolated;
```

- [ ] **Step 2: Write lightweight executor**

```rust
// core/src/cron/execution/lightweight.rs
use crate::cron::config::{ExecutionResult, JobSnapshot, RunStatus};
use crate::cron::clock::Clock;

/// Execute a lightweight job by injecting a system event into the main session.
/// This is a <1ms operation — no LLM cost, no isolation needed.
pub async fn execute_lightweight<C: Clock>(
    snapshot: &JobSnapshot,
    clock: &C,
    // event_sender: gateway event channel sender — injected by service layer
) -> ExecutionResult {
    let started_at = clock.now_ms();

    // The actual event injection will be wired by the service layer
    // via a callback that sends to the gateway's event channel.
    // Here we just produce the result structure.

    ExecutionResult {
        started_at,
        ended_at: clock.now_ms(),
        duration_ms: clock.now_ms() - started_at,
        status: RunStatus::Ok,
        output: Some(snapshot.prompt.clone()),
        error: None,
        error_reason: None,
        delivery_status: None,
        agent_used_messaging_tool: false,
    }
}
```

- [ ] **Step 3: Write isolated executor skeleton**

```rust
// core/src/cron/execution/isolated.rs
use crate::cron::config::{ExecutionResult, JobSnapshot, RunStatus, ErrorReason};
use crate::cron::clock::Clock;
use std::time::Duration;

/// Execute an isolated agent job in an independent session.
/// This is the heavyweight path — creates a session, runs an agent turn, collects output.
pub async fn execute_isolated<C: Clock>(
    snapshot: &JobSnapshot,
    clock: &C,
    // agent_runner: trait object for running agent turns — injected by service layer
) -> ExecutionResult {
    let started_at = clock.now_ms();
    let timeout = snapshot.timeout_ms
        .map(|ms| Duration::from_millis(ms as u64))
        .unwrap_or(Duration::from_secs(300));

    // TODO: Wire to actual agent_loop::run_turn() via trait/callback.
    // For now, return a placeholder that the service layer will replace
    // with real agent execution.

    ExecutionResult {
        started_at,
        ended_at: clock.now_ms(),
        duration_ms: clock.now_ms() - started_at,
        status: RunStatus::Ok,
        output: None,
        error: None,
        error_reason: None,
        delivery_status: None,
        agent_used_messaging_tool: false,
    }
}

/// Classify an error as transient or permanent.
pub fn classify_error(error: &str) -> ErrorReason {
    let lower = error.to_lowercase();
    let transient_patterns = [
        "rate_limit", "rate limit", "429",
        "timeout", "timed out",
        "overloaded", "503", "502", "500",
        "network", "connection", "dns",
        "temporarily", "retry",
    ];

    if transient_patterns.iter().any(|p| lower.contains(p)) {
        ErrorReason::Transient(error.to_string())
    } else {
        ErrorReason::Permanent(error.to_string())
    }
}
```

- [ ] **Step 4: Write error classification tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_transient_errors() {
        assert!(matches!(classify_error("rate_limit exceeded"), ErrorReason::Transient(_)));
        assert!(matches!(classify_error("Request timeout after 30s"), ErrorReason::Transient(_)));
        assert!(matches!(classify_error("HTTP 503 Service Unavailable"), ErrorReason::Transient(_)));
        assert!(matches!(classify_error("connection refused"), ErrorReason::Transient(_)));
    }

    #[test]
    fn classify_permanent_errors() {
        assert!(matches!(classify_error("invalid API key"), ErrorReason::Permanent(_)));
        assert!(matches!(classify_error("model not found"), ErrorReason::Permanent(_)));
        assert!(matches!(classify_error("permission denied"), ErrorReason::Permanent(_)));
    }
}
```

- [ ] **Step 5: Register, run tests, commit**

Add `pub mod execution;` to `core/src/cron/mod.rs`.

Run: `cargo test -p alephcore --lib cron::execution`

```bash
git add core/src/cron/execution/ core/src/cron/mod.rs
git commit -m "cron: add dual-path execution module (lightweight + isolated)"
```

---

### Task 12: Failure Alert with Cooldown

**Files:**
- Create: `core/src/cron/alert.rs`
- Modify: `core/src/cron/mod.rs`

- [ ] **Step 1: Write alert module**

```rust
// core/src/cron/alert.rs
use crate::cron::config::{CronJob, FailureAlertConfig};
use crate::cron::clock::Clock;

/// Check if a failure alert should be sent for this job.
/// Returns the alert message if conditions are met, None otherwise.
pub fn should_send_alert(
    job: &CronJob,
    alert_config: &FailureAlertConfig,
    now_ms: i64,
) -> Option<String> {
    // Check: enough consecutive errors?
    if job.state.consecutive_errors < alert_config.after {
        return None;
    }

    // Check: cooldown period
    if let Some(last_alert) = job.state.last_failure_alert_at_ms {
        if now_ms - last_alert < alert_config.cooldown_ms {
            return None; // Still in cooldown
        }
    }

    Some(format!(
        "Cron job '{}' ({}) failed {} times consecutively. Last error: {}",
        job.name,
        job.id,
        job.state.consecutive_errors,
        job.state.last_error.as_deref().unwrap_or("unknown")
    ))
}
```

- [ ] **Step 2: Write alert tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::{JobStateV2, DeliveryTargetConfig};

    fn make_alert_config() -> FailureAlertConfig {
        FailureAlertConfig {
            after: 2,
            cooldown_ms: 3_600_000,
            target: DeliveryTargetConfig::Webhook {
                url: "https://example.com".to_string(),
                method: None,
                headers: None,
            },
        }
    }

    #[test]
    fn no_alert_below_threshold() {
        let mut job = make_test_job("j1");
        job.state.consecutive_errors = 1;
        assert!(should_send_alert(&job, &make_alert_config(), 1_000_000).is_none());
    }

    #[test]
    fn alert_at_threshold() {
        let mut job = make_test_job("j1");
        job.state.consecutive_errors = 2;
        job.state.last_error = Some("timeout".to_string());
        let msg = should_send_alert(&job, &make_alert_config(), 1_000_000);
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("timeout"));
    }

    #[test]
    fn alert_respects_cooldown() {
        let mut job = make_test_job("j1");
        job.state.consecutive_errors = 5;
        job.state.last_failure_alert_at_ms = Some(1_000_000);
        // Within cooldown (1h)
        assert!(should_send_alert(&job, &make_alert_config(), 1_500_000).is_none());
        // After cooldown
        assert!(should_send_alert(&job, &make_alert_config(), 5_000_000).is_some());
    }
}
```

- [ ] **Step 3: Register, run tests, commit**

Add `pub mod alert;` to `core/src/cron/mod.rs`.

Run: `cargo test -p alephcore --lib cron::alert`

```bash
git add core/src/cron/alert.rs core/src/cron/mod.rs
git commit -m "cron: add failure alert with cooldown mechanism"
```

---

### Task 13: Delivery Enhancement (Dedup)

**Files:**
- Modify: `core/src/cron/delivery.rs`

- [ ] **Step 1: Add dedup logic to delivery engine**

Add a method to `DeliveryEngine` that checks `agent_used_messaging_tool` before delivering:

```rust
/// Deliver with dedup: skip if agent already sent via messaging tool.
pub async fn deliver_with_dedup(
    &self,
    job: &CronJob,
    run: &JobRun,
    config: &DeliveryConfig,
    agent_already_sent: bool,
) -> DeliveryStatus {
    if matches!(config.mode, DeliveryMode::None) {
        return DeliveryStatus::NotRequested;
    }

    if agent_already_sent {
        return DeliveryStatus::AlreadySentByAgent;
    }

    match self.deliver(job, run, config).await {
        Ok(outcomes) if outcomes.iter().any(|o| o.success) => DeliveryStatus::Delivered,
        _ => DeliveryStatus::NotDelivered,
    }
}
```

- [ ] **Step 2: Write dedup test**

```rust
#[test]
fn dedup_skips_when_agent_sent() {
    // ... test that AlreadySentByAgent is returned when agent_already_sent = true
}
```

- [ ] **Step 3: Run tests and commit**

Run: `cargo test -p alephcore --lib cron::delivery`

```bash
git add core/src/cron/delivery.rs
git commit -m "cron: add delivery dedup for agent-sent messages"
```

---

## Chunk 5: Migration — Adapt Existing Code

### Task 14: Migrate chain.rs from SQLite to CronStore

**Files:**
- Modify: `core/src/cron/chain.rs`

- [ ] **Step 1: Rewrite cycle detection to operate on CronStore**

Replace `detect_cycle_sync(conn: &Connection, ...)` with `detect_cycle(store: &CronStore, ...)` that traverses in-memory job list instead of SQLite queries. Same DFS algorithm, different data source.

- [ ] **Step 2: Rewrite chain triggering to operate on CronStore**

Replace `trigger_chain_job_sync(conn: &Connection, ...)` with `trigger_chain_job(store: &mut CronStore, ...)` that sets `next_run_at_ms` directly on the in-memory job.

- [ ] **Step 3: Update existing tests to use CronStore**

All 8 existing tests should be adapted to create a CronStore in tempdir instead of SQLite Connection.

- [ ] **Step 4: Run tests and commit**

Run: `cargo test -p alephcore --lib cron::chain`

```bash
git add core/src/cron/chain.rs
git commit -m "cron: migrate chain.rs from SQLite to CronStore"
```

---

### Task 15: Inject Clock into template.rs

**Files:**
- Modify: `core/src/cron/template.rs`

- [ ] **Step 1: Change `render_template` signature to accept `&dyn Clock`**

Replace the direct `Utc::now()` call (line 25) with `clock.now_utc()`. Update function signature:

```rust
pub fn render_template(
    template: &str,
    job: &CronJob,
    last_run: Option<&JobRun>,
    run_count: u64,
    clock: &dyn Clock,
) -> String {
    // ... replace Utc::now() with clock.now_utc() ...
}
```

- [ ] **Step 2: Update callers** — search for all `render_template` call sites and pass clock.

- [ ] **Step 3: Run existing template tests (they should still pass with SystemClock or be updated to FakeClock)**

Run: `cargo test -p alephcore --lib cron::template`

- [ ] **Step 4: Commit**

```bash
git add core/src/cron/template.rs
git commit -m "cron: inject Clock trait into template rendering"
```

---

### Task 16: Rebuild mod.rs as Thin Facade

**Files:**
- Modify: `core/src/cron/mod.rs` (major rewrite — from 1346 lines to ~100 lines)

- [ ] **Step 1: Extract CronService to wrap ServiceState**

The new `mod.rs` becomes a thin facade that:
1. Re-exports all public types from submodules
2. Provides a `CronService` struct that wraps `ServiceState<SystemClock>` and exposes the same public API methods (add_job, list_jobs, etc.) by delegating to `service::ops`
3. Provides `start()` and `stop()` that start/stop the timer loop

- [ ] **Step 2: Remove old SQLite-based implementation code**

Delete all the internal SQLite schema init, migration, job scheduling loop code from mod.rs. The new implementation lives in `service/`, `store.rs`, and `execution/`.

- [ ] **Step 3: Ensure Gateway handler compatibility**

The `SharedCronService = Arc<Mutex<CronService>>` type alias should still work. Gateway handlers call `cron.lock().await.list_jobs()` etc.

- [ ] **Step 4: Run full cron test suite**

Run: `cargo test -p alephcore --lib cron`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add core/src/cron/mod.rs
git commit -m "cron: rebuild mod.rs as thin facade over service layer"
```

---

### Task 17: Delete Obsolete Files

**Files:**
- Delete: `core/src/cron/scheduler.rs`
- Delete: `core/src/cron/resource.rs`

- [ ] **Step 1: Remove module declarations from mod.rs**

Remove `mod scheduler;` and `mod resource;` from `core/src/cron/mod.rs`.

- [ ] **Step 2: Delete files**

```bash
rm core/src/cron/scheduler.rs core/src/cron/resource.rs
```

- [ ] **Step 3: Fix any compilation errors from removed imports**

Search for `use crate::cron::scheduler::` and `use crate::cron::resource::` across the codebase and update.

- [ ] **Step 4: Run compile check and tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib cron`

- [ ] **Step 5: Commit**

```bash
git add -A core/src/cron/
git commit -m "cron: remove obsolete scheduler.rs and resource.rs"
```

---

### Task 18: Update Gateway Handlers

**Files:**
- Modify: `core/src/gateway/handlers/cron.rs`

- [ ] **Step 1: Update `job_to_json` to include new fields**

Add `running_at_ms`, `consecutive_errors`, `last_error_reason`, `last_delivery_status`, `failure_alert`, `anchor_ms`, `stagger_ms`, `session_target` to the JSON output.

- [ ] **Step 2: Update `handle_create` to accept new fields**

Parse `anchor_ms`, `stagger_ms`, `failure_alert`, `session_target` from request params and pass through to `ops::add_job`.

- [ ] **Step 3: Update `handle_update` to accept new fields**

Same new fields in `CronJobUpdates`.

- [ ] **Step 4: Update `handle_run` to use three-phase manual trigger**

Call `concurrency::phase1_mark_manual` → execute → `phase3_writeback` instead of direct execution.

- [ ] **Step 5: Ensure `handle_list` and `handle_get` use read-only paths**

These should call `ops::list_jobs` and `ops::get_job` which return `CronJobView` (no mutation).

- [ ] **Step 6: Run compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 7: Commit**

```bash
git add core/src/gateway/handlers/cron.rs
git commit -m "cron: update gateway handlers for redesigned service API"
```

---

## Chunk 6: Panel UI Sync

### Task 19: Update Panel API DTOs

**Files:**
- Modify: `apps/panel/src/api/cron.rs`

- [ ] **Step 1: Add new fields to CronJobInfo**

```rust
// Add to CronJobInfo struct:
pub anchor_ms: Option<i64>,
pub stagger_ms: Option<i64>,
pub running_at_ms: Option<i64>,
pub consecutive_errors: u32,
pub last_error_reason: Option<String>,
pub last_delivery_status: Option<String>,
pub session_target: Option<String>,
pub failure_alert: Option<FailureAlertInfo>,
```

- [ ] **Step 2: Add FailureAlertInfo DTO**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureAlertInfo {
    pub after: u32,
    pub cooldown_ms: i64,
    pub target_kind: String,
}
```

- [ ] **Step 3: Update CreateCronJob and UpdateCronJob**

Add `anchor_ms`, `stagger_ms`, `failure_alert`, `session_target` fields.

- [ ] **Step 4: Update JobRunInfo**

Add `delivery_status: Option<String>` and `error_reason: Option<String>`.

- [ ] **Step 5: Commit**

```bash
git add apps/panel/src/api/cron.rs
git commit -m "panel: update cron API DTOs with new fields"
```

---

### Task 20: Update Panel View

**Files:**
- Modify: `apps/panel/src/views/cron.rs`

- [ ] **Step 1: Update JobList status indicator**

Replace two-state (green/gray) with three-state + error badge:
- `running_at_ms.is_some()` → blue pulsing dot
- `enabled && !running` → green dot
- `!enabled` → gray dot
- `consecutive_errors > 0` → red badge overlay

- [ ] **Step 2: Update format_schedule_summary for new ScheduleKind**

Handle the `schedule` field being a tagged JSON object instead of flat fields. Extract `kind`, `every_ms`/`expr`/`at` from the object.

- [ ] **Step 3: Add anchor and stagger inputs to JobEditor**

When Schedule Type = `every`: show optional Anchor input.
When Schedule Type = `cron`: show optional Stagger input.

- [ ] **Step 4: Add failure alert collapsible section**

New collapsible panel in JobEditor with `after`, `cooldown`, and `target` fields.

- [ ] **Step 5: Add Delivery column to RunHistory**

New column showing delivery status with icons.

- [ ] **Step 6: Update error display in RunHistory**

Show `transient:` / `permanent:` prefix from `error_reason` field.

- [ ] **Step 7: Run panel compile check**

Run: `cargo check -p aleph-panel` (or equivalent panel crate name)

- [ ] **Step 8: Commit**

```bash
git add apps/panel/src/views/cron.rs
git commit -m "panel: update cron UI with three-state indicator, anchor/stagger, alerts"
```

---

## Chunk 7: Integration, SQLite History, Cleanup

### Task 21: SQLite Execution History

**Files:**
- Modify: `core/src/resilience/database/tasks.rs` or create new table in `state_database.rs`

- [ ] **Step 1: Add cron_job_runs table schema**

```sql
CREATE TABLE IF NOT EXISTS cron_job_runs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    trigger_source TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    duration_ms INTEGER,
    error TEXT,
    error_reason TEXT,
    output_summary TEXT,
    delivery_status TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_cron_runs_job_id ON cron_job_runs(job_id);
CREATE INDEX IF NOT EXISTS idx_cron_runs_created_at ON cron_job_runs(created_at);
```

- [ ] **Step 2: Write insert and query functions**

- `insert_cron_run(conn, run)` — insert execution record
- `get_cron_runs(conn, job_id, limit)` — query history
- `cleanup_old_runs(conn, retention_days)` — delete old records

- [ ] **Step 3: Wire history recording into Phase 3 writeback**

After `phase3_writeback`, asynchronously write each `ExecutionResult` to SQLite.

- [ ] **Step 4: Commit**

```bash
git add core/src/resilience/database/
git commit -m "cron: add SQLite execution history recording"
```

---

### Task 22: Regression Test Suite

**Files:**
- Create: `core/src/cron/tests/regression.rs` (or add to existing test modules)

- [ ] **Step 1: Write OpenClaw regression tests**

```rust
/// OpenClaw Bug #13992: maintenance recompute must not advance past-due jobs
#[test]
fn regression_13992_maintenance_recompute_no_advance() { ... }

/// OpenClaw Bug #17821: MIN_REFIRE_GAP prevents spin loops
#[test]
fn regression_17821_min_refire_gap() { ... }

/// OpenClaw Bug #17554: stale running markers cleared on startup
#[tokio::test]
async fn regression_17554_stale_running_marker() { ... }

/// OpenClaw Bug #18892: startup catchup respects max_missed limit
#[tokio::test]
async fn regression_18892_startup_overload() { ... }
```

- [ ] **Step 2: Run all regression tests**

Run: `cargo test -p alephcore --lib cron`
Expected: All pass.

- [ ] **Step 3: Commit**

```bash
git add core/src/cron/
git commit -m "cron: add regression tests from OpenClaw production bugs"
```

---

### Task 23: Final Integration Check

- [ ] **Step 1: Full compile check**

Run: `cargo check --workspace` (or `cargo check -p alephcore -p aleph-panel`)

- [ ] **Step 2: Full test suite**

Run: `cargo test -p alephcore --lib`

- [ ] **Step 3: Clippy**

Run: `cargo clippy -p alephcore -- -D warnings`

- [ ] **Step 4: Final commit if any fixes needed**

```bash
git add -A
git commit -m "cron: final integration fixes for module redesign"
```

---

## Summary

| Chunk | Tasks | Est. New/Modified Lines |
|-------|-------|------------------------|
| 1. Foundation | Tasks 1-4 + 2b: clock, config types, CronJob rewrite, schedule, stagger | ~800 |
| 2. Persistence | Task 5: JSON atomic store | ~300 |
| 3. Service Layer | Tasks 6-10: state, ops, concurrency, catchup, timer | ~700 |
| 4. Execution/Alert/Delivery | Tasks 11-13 | ~250 |
| 5. Migration | Tasks 14-18: chain, template, mod.rs, gateway | ~400 |
| 6. Panel UI | Tasks 19-20: API DTOs, views | ~200 |
| 7. Integration | Tasks 21-23: SQLite history, regression tests, cleanup | ~250 |
| **Total** | **24 tasks** | **~2900 lines** |
