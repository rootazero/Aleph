# Cron System Redesign — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Redesign Aleph's cron scheduling system to surpass openclaw — adding state-machine scheduling, restart catch-up, exponential backoff, delivery pipeline, dynamic prompt templates, resource-aware scheduling, and job chaining.

**Architecture:** Incremental refactor on existing `CronService` skeleton. Extend SQLite schema with new fields, replace window-based scheduling with `next_run_at` state machine, add new modules for delivery/template/chain/resource. All changes gated behind `#[cfg(feature = "cron")]`.

**Tech Stack:** Rust, tokio, rusqlite, chrono + chrono-tz, cron crate, sysinfo, regex

**Design Doc:** `docs/plans/2026-02-24-cron-system-redesign.md`

---

## Task 1: Add New Dependencies to Cargo.toml

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add chrono-tz, sysinfo, async-trait dependencies**

In `core/Cargo.toml`, add to `[dependencies]` section (after the existing `chrono` line ~line 60):

```toml
chrono-tz = { version = "0.10", optional = true }
sysinfo = { version = "0.33", optional = true }
async-trait = "0.1"
```

Update the `cron` feature (line 27) to include the new optional deps:

```toml
cron = ["dep:cron", "dep:chrono-tz", "dep:sysinfo", "gateway"]
```

**Step 2: Verify it compiles**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --features cron`
Expected: Compiles with no errors

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "cron: add chrono-tz, sysinfo, async-trait dependencies"
```

---

## Task 2: Extend Data Types in config.rs

**Files:**
- Modify: `core/src/cron/config.rs`

**Step 1: Write tests for new types**

Add to the existing test module at the bottom of `config.rs` (before the closing `}`):

```rust
#[test]
fn test_schedule_kind_default() {
    let kind = ScheduleKind::default();
    assert_eq!(kind, ScheduleKind::Cron);
}

#[test]
fn test_schedule_kind_display() {
    assert_eq!(ScheduleKind::Cron.as_str(), "cron");
    assert_eq!(ScheduleKind::Every.as_str(), "every");
    assert_eq!(ScheduleKind::At.as_str(), "at");
}

#[test]
fn test_schedule_kind_from_str() {
    assert_eq!(ScheduleKind::from_str("cron"), ScheduleKind::Cron);
    assert_eq!(ScheduleKind::from_str("every"), ScheduleKind::Every);
    assert_eq!(ScheduleKind::from_str("at"), ScheduleKind::At);
    assert_eq!(ScheduleKind::from_str("invalid"), ScheduleKind::Cron);
}

#[test]
fn test_delivery_config_serialization() {
    let config = DeliveryConfig {
        mode: DeliveryMode::Primary,
        targets: vec![DeliveryTargetConfig::Webhook {
            url: "https://example.com/hook".to_string(),
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
fn test_trigger_source_display() {
    assert_eq!(TriggerSource::Schedule.as_str(), "schedule");
    assert_eq!(TriggerSource::Chain.as_str(), "chain");
    assert_eq!(TriggerSource::Manual.as_str(), "manual");
    assert_eq!(TriggerSource::Catchup.as_str(), "catchup");
}

#[test]
fn test_cron_job_extended_fields_defaults() {
    let job = CronJob::new("Test", "0 * * * *", "main", "prompt");
    assert_eq!(job.schedule_kind, ScheduleKind::Cron);
    assert_eq!(job.priority, 5);
    assert_eq!(job.consecutive_failures, 0);
    assert_eq!(job.max_retries, 3);
    assert!(job.next_run_at.is_none());
    assert!(job.running_at.is_none());
    assert!(job.delivery_config.is_none());
    assert!(job.next_job_id_on_success.is_none());
    assert_eq!(job.version, 1);
}
```

**Step 2: Run tests to verify they fail**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron -- cron::config::tests`
Expected: FAIL — `ScheduleKind`, `DeliveryConfig`, etc. not defined

**Step 3: Add ScheduleKind enum**

Add after the `JobStatus` impl (after line 210 in config.rs):

```rust
/// Schedule type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleKind {
    Cron,
    Every,
    At,
}

impl Default for ScheduleKind {
    fn default() -> Self {
        Self::Cron
    }
}

impl ScheduleKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Cron => "cron",
            Self::Every => "every",
            Self::At => "at",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "every" => Self::Every,
            "at" => Self::At,
            _ => Self::Cron,
        }
    }
}
```

**Step 4: Add TriggerSource enum**

Add after ScheduleKind:

```rust
/// What triggered a job run
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerSource {
    Schedule,
    Chain,
    Manual,
    Catchup,
}

impl Default for TriggerSource {
    fn default() -> Self {
        Self::Schedule
    }
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

    pub fn from_str(s: &str) -> Self {
        match s {
            "chain" => Self::Chain,
            "manual" => Self::Manual,
            "catchup" => Self::Catchup,
            _ => Self::Schedule,
        }
    }
}
```

**Step 5: Add Delivery types**

Add after TriggerSource:

```rust
/// Delivery pipeline configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryConfig {
    pub mode: DeliveryMode,
    pub targets: Vec<DeliveryTargetConfig>,
    #[serde(default)]
    pub fallback_target: Option<DeliveryTargetConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryMode {
    None,
    Primary,
    Broadcast,
}

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

/// Result of a delivery attempt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryOutcome {
    pub target_kind: String,
    pub success: bool,
    pub message: Option<String>,
}
```

**Step 6: Extend CronJob struct with new fields**

Replace the `CronJob` struct (lines 102-138) with the extended version. Add new fields after the existing ones:

```rust
/// A scheduled job definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    // === Existing fields (unchanged) ===
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub agent_id: String,
    pub prompt: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,

    // === State-machine scheduling ===
    #[serde(default)]
    pub next_run_at: Option<i64>,
    #[serde(default)]
    pub running_at: Option<i64>,
    #[serde(default)]
    pub last_run_at: Option<i64>,

    // === Resilience ===
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_priority")]
    pub priority: u32,

    // === Schedule types ===
    #[serde(default)]
    pub schedule_kind: ScheduleKind,
    #[serde(default)]
    pub every_ms: Option<i64>,
    #[serde(default)]
    pub at_time: Option<i64>,
    #[serde(default)]
    pub delete_after_run: bool,

    // === Job chaining ===
    #[serde(default)]
    pub next_job_id_on_success: Option<String>,
    #[serde(default)]
    pub next_job_id_on_failure: Option<String>,

    // === Delivery ===
    #[serde(default)]
    pub delivery_config: Option<DeliveryConfig>,

    // === Dynamic prompt ===
    #[serde(default)]
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub context_vars: Option<String>,

    // === Optimistic locking ===
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_max_retries() -> u32 { 3 }
fn default_priority() -> u32 { 5 }
fn default_version() -> u32 { 1 }
```

**Step 7: Update CronJob::new() to initialize new fields**

Replace the `new()` method (lines 142-161):

```rust
impl CronJob {
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
            // New fields with defaults
            next_run_at: None,
            running_at: None,
            last_run_at: None,
            consecutive_failures: 0,
            max_retries: 3,
            priority: 5,
            schedule_kind: ScheduleKind::Cron,
            every_ms: None,
            at_time: None,
            delete_after_run: false,
            next_job_id_on_success: None,
            next_job_id_on_failure: None,
            delivery_config: None,
            prompt_template: None,
            context_vars: None,
            version: 1,
        }
    }

    // Keep existing validate_schedule methods unchanged
```

**Step 8: Extend JobRun with new fields**

Add fields to `JobRun` struct (after line 237):

```rust
pub struct JobRun {
    pub id: String,
    pub job_id: String,
    pub status: JobStatus,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub response: Option<String>,
    // New fields
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub trigger_source: TriggerSource,
    #[serde(default)]
    pub delivery_status: Option<String>,
    #[serde(default)]
    pub delivery_error: Option<String>,
}
```

Update `JobRun::new()` to include the new fields:

```rust
impl JobRun {
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
            trigger_source: TriggerSource::Schedule,
            delivery_status: None,
            delivery_error: None,
        }
    }

    pub fn with_trigger(mut self, source: TriggerSource) -> Self {
        self.trigger_source = source;
        self
    }

    // Keep existing success(), failed(), timeout() methods unchanged
```

**Step 9: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron -- cron::config::tests`
Expected: ALL PASS

**Step 10: Commit**

```bash
git add core/src/cron/config.rs
git commit -m "cron: extend data types with scheduling, delivery, chain, template fields"
```

---

## Task 3: Migrate SQLite Schema

**Files:**
- Modify: `core/src/cron/mod.rs` (lines 150-183, `init_schema`)

**Step 1: Write test for schema migration**

Add to tests module in `mod.rs` (after line 857):

```rust
#[tokio::test]
async fn test_schema_migration_adds_new_columns() {
    let config = test_config();
    let service = CronService::new(config).unwrap();

    // Add a job — should work with new columns
    let mut job = CronJob::new("Migration Test", "0 * * * *", "main", "test");
    job.priority = 1;
    job.schedule_kind = ScheduleKind::Every;
    job.every_ms = Some(60_000);

    let job_id = job.id.clone();
    service.add_job(job).await.unwrap();

    let retrieved = service.get_job(&job_id).await.unwrap();
    assert_eq!(retrieved.priority, 1);
    assert_eq!(retrieved.schedule_kind, ScheduleKind::Every);
    assert_eq!(retrieved.every_ms, Some(60_000));
    assert_eq!(retrieved.version, 1);
    assert!(retrieved.next_run_at.is_none());
    assert!(retrieved.running_at.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron -- cron::tests::test_schema_migration`
Expected: FAIL — new columns don't exist in schema

**Step 3: Update init_schema to include new columns**

Replace `init_schema` (lines 150-183) with:

```rust
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
            -- New fields
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

    // Migration: add columns if they don't exist (for existing databases)
    Self::migrate_schema(conn)?;

    Ok(())
}

/// Add new columns to existing databases that lack them
fn migrate_schema(conn: &Connection) -> CronResult<()> {
    let migrations = [
        "ALTER TABLE cron_jobs ADD COLUMN next_run_at INTEGER",
        "ALTER TABLE cron_jobs ADD COLUMN running_at INTEGER",
        "ALTER TABLE cron_jobs ADD COLUMN last_run_at INTEGER",
        "ALTER TABLE cron_jobs ADD COLUMN consecutive_failures INTEGER DEFAULT 0",
        "ALTER TABLE cron_jobs ADD COLUMN max_retries INTEGER DEFAULT 3",
        "ALTER TABLE cron_jobs ADD COLUMN priority INTEGER DEFAULT 5",
        "ALTER TABLE cron_jobs ADD COLUMN schedule_kind TEXT DEFAULT 'cron'",
        "ALTER TABLE cron_jobs ADD COLUMN every_ms INTEGER",
        "ALTER TABLE cron_jobs ADD COLUMN at_time INTEGER",
        "ALTER TABLE cron_jobs ADD COLUMN delete_after_run INTEGER DEFAULT 0",
        "ALTER TABLE cron_jobs ADD COLUMN next_job_id_on_success TEXT",
        "ALTER TABLE cron_jobs ADD COLUMN next_job_id_on_failure TEXT",
        "ALTER TABLE cron_jobs ADD COLUMN delivery_config TEXT",
        "ALTER TABLE cron_jobs ADD COLUMN prompt_template TEXT",
        "ALTER TABLE cron_jobs ADD COLUMN context_vars TEXT",
        "ALTER TABLE cron_jobs ADD COLUMN version INTEGER DEFAULT 1",
        "ALTER TABLE cron_runs ADD COLUMN retry_count INTEGER DEFAULT 0",
        "ALTER TABLE cron_runs ADD COLUMN trigger_source TEXT DEFAULT 'schedule'",
        "ALTER TABLE cron_runs ADD COLUMN delivery_status TEXT",
        "ALTER TABLE cron_runs ADD COLUMN delivery_error TEXT",
    ];

    for sql in &migrations {
        // Ignore "duplicate column" errors from re-running migrations
        let _ = conn.execute(sql, []);
    }

    Ok(())
}
```

**Step 4: Update add_job SQL to include new columns**

Replace the INSERT in `add_job` (lines 207-223):

```rust
conn.execute(
    r#"
    INSERT INTO cron_jobs (
        id, name, schedule, agent_id, prompt, enabled, timezone, tags,
        created_at, updated_at, next_run_at, running_at, last_run_at,
        consecutive_failures, max_retries, priority,
        schedule_kind, every_ms, at_time, delete_after_run,
        next_job_id_on_success, next_job_id_on_failure,
        delivery_config, prompt_template, context_vars, version
    )
    VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
        ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
        ?21, ?22, ?23, ?24, ?25, ?26
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
        job.delivery_config.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()),
        job.prompt_template,
        job.context_vars,
        job.version,
    ],
)?;
```

**Step 5: Update get_job and list_jobs SELECT queries to read new columns**

Update the row-mapping closure in `get_job` and `list_jobs` to read new columns. The SELECT should include all columns and the mapping should populate the new fields.

**Step 6: Update update_job to persist new fields**

Update the UPDATE SQL in `update_job` to include all new columns.

**Step 7: Update save_run_sync to persist new JobRun fields**

Update the INSERT in `save_run_sync` and the SELECT in `get_job_runs` to include retry_count, trigger_source, delivery_status, delivery_error.

**Step 8: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron -- cron::tests`
Expected: ALL PASS (including original tests + new migration test)

**Step 9: Commit**

```bash
git add core/src/cron/mod.rs
git commit -m "cron: migrate schema with state-machine, resilience, delivery, chain fields"
```

---

## Task 4: Prompt Template Engine

**Files:**
- Create: `core/src/cron/template.rs`
- Modify: `core/src/cron/mod.rs` (add `pub mod template;`)

**Step 1: Write failing tests**

Create `core/src/cron/template.rs`:

```rust
//! Dynamic prompt template engine for cron jobs.
//!
//! Supports {{variable}} substitution with built-in and environment variables.

use crate::cron::config::{CronJob, JobRun};

/// Render a prompt template with variable substitution.
///
/// Built-in variables:
/// - `{{now}}` — current time ISO 8601
/// - `{{now_unix}}` — Unix timestamp (seconds)
/// - `{{job_name}}` — job name
/// - `{{last_output}}` — previous run's response
/// - `{{run_count}}` — total execution count
/// - `{{env:VAR}}` — environment variable
pub fn render_template(
    template: &str,
    job: &CronJob,
    last_run: Option<&JobRun>,
    run_count: u64,
) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_basic_variables() {
        let job = CronJob::new("Daily News", "0 9 * * *", "main", "unused");
        let result = render_template(
            "Hello {{job_name}}, run #{{run_count}}",
            &job,
            None,
            5,
        );
        assert_eq!(result, "Hello Daily News, run #5");
    }

    #[test]
    fn test_render_now_variables() {
        let job = CronJob::new("Test", "0 * * * *", "main", "unused");
        let result = render_template("Time: {{now}}", &job, None, 0);
        // Should contain a valid ISO 8601 timestamp
        assert!(result.starts_with("Time: 20"));
        assert!(result.contains("T"));
    }

    #[test]
    fn test_render_last_output_with_previous_run() {
        let job = CronJob::new("Test", "0 * * * *", "main", "unused");
        let mut run = JobRun::new("job-1");
        run = run.success(Some("Previous AI response here".to_string()));

        let result = render_template(
            "Based on: {{last_output}}",
            &job,
            Some(&run),
            1,
        );
        assert_eq!(result, "Based on: Previous AI response here");
    }

    #[test]
    fn test_render_last_output_first_run() {
        let job = CronJob::new("Test", "0 * * * *", "main", "unused");
        let result = render_template(
            "Previous: {{last_output}}",
            &job,
            None,
            0,
        );
        assert_eq!(result, "Previous: (first run)");
    }

    #[test]
    fn test_render_env_variable() {
        std::env::set_var("ALEPH_TEST_VAR", "hello_world");
        let job = CronJob::new("Test", "0 * * * *", "main", "unused");
        let result = render_template(
            "Value: {{env:ALEPH_TEST_VAR}}",
            &job,
            None,
            0,
        );
        assert_eq!(result, "Value: hello_world");
        std::env::remove_var("ALEPH_TEST_VAR");
    }

    #[test]
    fn test_render_no_templates() {
        let job = CronJob::new("Test", "0 * * * *", "main", "unused");
        let result = render_template("Plain text with no variables", &job, None, 0);
        assert_eq!(result, "Plain text with no variables");
    }

    #[test]
    fn test_render_unknown_variable_left_as_is() {
        let job = CronJob::new("Test", "0 * * * *", "main", "unused");
        let result = render_template("{{unknown_var}}", &job, None, 0);
        assert_eq!(result, "{{unknown_var}}");
    }
}
```

**Step 2: Register module in mod.rs**

Add `pub mod template;` after `pub mod config;` in `core/src/cron/mod.rs` (after line 53).

**Step 3: Run tests to verify they fail**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron -- cron::template::tests`
Expected: FAIL with "not yet implemented"

**Step 4: Implement render_template**

Replace the `todo!()` in `render_template`:

```rust
pub fn render_template(
    template: &str,
    job: &CronJob,
    last_run: Option<&JobRun>,
    run_count: u64,
) -> String {
    let now = chrono::Utc::now();
    let mut result = template.to_string();

    // Built-in variables (check existence before replacing to leave unknown vars intact)
    if result.contains("{{now}}") {
        result = result.replace("{{now}}", &now.to_rfc3339());
    }
    if result.contains("{{now_unix}}") {
        result = result.replace("{{now_unix}}", &now.timestamp().to_string());
    }
    if result.contains("{{job_name}}") {
        result = result.replace("{{job_name}}", &job.name);
    }
    if result.contains("{{run_count}}") {
        result = result.replace("{{run_count}}", &run_count.to_string());
    }
    if result.contains("{{last_output}}") {
        let last = last_run
            .and_then(|r| r.response.as_deref())
            .unwrap_or("(first run)");
        result = result.replace("{{last_output}}", last);
    }

    // Environment variables: {{env:VAR_NAME}}
    let env_re = regex::Regex::new(r"\{\{env:(\w+)\}\}").unwrap();
    result = env_re
        .replace_all(&result, |caps: &regex::Captures| {
            std::env::var(&caps[1]).unwrap_or_default()
        })
        .to_string();

    result
}
```

**Step 5: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron -- cron::template::tests`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add core/src/cron/template.rs core/src/cron/mod.rs
git commit -m "cron: add prompt template engine with variable substitution"
```

---

## Task 5: Scheduler Engine — Core Logic

**Files:**
- Create: `core/src/cron/scheduler.rs`
- Modify: `core/src/cron/mod.rs` (add `pub mod scheduler;`)

**Step 1: Write tests for compute_next_run_at and backoff**

Create `core/src/cron/scheduler.rs` with tests first:

```rust
//! Scheduler engine for the cron system.
//!
//! Replaces the old window-based scheduling with a state-machine approach
//! using `next_run_at` for precise, deduplicated job execution.

use chrono::{DateTime, Utc};
use crate::cron::config::{CronJob, ScheduleKind};

/// Exponential backoff schedule for consecutive failures
pub const BACKOFF_SCHEDULE_MS: &[u64] = &[
    30_000,     // 1st failure → 30s
    60_000,     // 2nd → 1 min
    300_000,    // 3rd → 5 min
    900_000,    // 4th → 15 min
    3_600_000,  // 5th+ → 60 min
];

/// Threshold for detecting stuck jobs (2 hours)
pub const STUCK_THRESHOLD_MS: i64 = 2 * 60 * 60 * 1000;

/// Compute the backoff delay based on consecutive failure count.
pub fn compute_backoff_ms(consecutive_failures: u32) -> u64 {
    if consecutive_failures == 0 {
        return 0;
    }
    let idx = (consecutive_failures.saturating_sub(1) as usize)
        .min(BACKOFF_SCHEDULE_MS.len() - 1);
    BACKOFF_SCHEDULE_MS[idx]
}

/// Compute next run time for a job, based on its schedule kind.
///
/// Returns millisecond timestamp or None if the job should not run again.
#[cfg(feature = "cron")]
pub fn compute_next_run_at(job: &CronJob, from: DateTime<Utc>) -> Option<i64> {
    let from_ms = from.timestamp_millis();

    match job.schedule_kind {
        ScheduleKind::Cron => {
            use std::str::FromStr;
            let schedule = cron::Schedule::from_str(&job.schedule).ok()?;

            #[cfg(feature = "cron")]
            {
                if let Some(tz_str) = job.timezone.as_deref() {
                    if let Ok(tz) = tz_str.parse::<chrono_tz::Tz>() {
                        let local_now = from.with_timezone(&tz);
                        return schedule
                            .after(&local_now)
                            .next()
                            .map(|t| t.with_timezone(&Utc).timestamp_millis());
                    }
                }
            }

            // Fallback to UTC
            schedule
                .upcoming(Utc)
                .next()
                .map(|t| t.timestamp_millis())
        }
        ScheduleKind::Every => {
            let interval = job.every_ms?;
            if interval <= 0 {
                return None;
            }
            Some(from_ms + interval)
        }
        ScheduleKind::At => {
            let target = job.at_time?;
            if target > from_ms {
                Some(target)
            } else {
                None // Already past
            }
        }
    }
}

#[cfg(not(feature = "cron"))]
pub fn compute_next_run_at(job: &CronJob, from: DateTime<Utc>) -> Option<i64> {
    let from_ms = from.timestamp_millis();
    match job.schedule_kind {
        ScheduleKind::Every => {
            let interval = job.every_ms?;
            Some(from_ms + interval)
        }
        ScheduleKind::At => {
            let target = job.at_time?;
            if target > from_ms { Some(target) } else { None }
        }
        _ => None,
    }
}

/// Check if a one-shot job has already been completed.
pub fn is_completed_oneshot(job: &CronJob) -> bool {
    job.schedule_kind == ScheduleKind::At && job.last_run_at.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_zero_failures() {
        assert_eq!(compute_backoff_ms(0), 0);
    }

    #[test]
    fn test_backoff_schedule() {
        assert_eq!(compute_backoff_ms(1), 30_000);
        assert_eq!(compute_backoff_ms(2), 60_000);
        assert_eq!(compute_backoff_ms(3), 300_000);
        assert_eq!(compute_backoff_ms(4), 900_000);
        assert_eq!(compute_backoff_ms(5), 3_600_000);
        // Beyond 5 should cap at 60 min
        assert_eq!(compute_backoff_ms(100), 3_600_000);
    }

    #[test]
    fn test_compute_next_run_every() {
        let mut job = CronJob::new("Test", "unused", "main", "prompt");
        job.schedule_kind = ScheduleKind::Every;
        job.every_ms = Some(60_000); // 1 minute

        let now = Utc::now();
        let next = compute_next_run_at(&job, now).unwrap();
        assert_eq!(next, now.timestamp_millis() + 60_000);
    }

    #[test]
    fn test_compute_next_run_at_future() {
        let mut job = CronJob::new("Test", "unused", "main", "prompt");
        job.schedule_kind = ScheduleKind::At;
        let future = Utc::now().timestamp_millis() + 3_600_000; // 1 hour from now
        job.at_time = Some(future);

        let next = compute_next_run_at(&job, Utc::now()).unwrap();
        assert_eq!(next, future);
    }

    #[test]
    fn test_compute_next_run_at_past() {
        let mut job = CronJob::new("Test", "unused", "main", "prompt");
        job.schedule_kind = ScheduleKind::At;
        let past = Utc::now().timestamp_millis() - 3_600_000; // 1 hour ago
        job.at_time = Some(past);

        let next = compute_next_run_at(&job, Utc::now());
        assert!(next.is_none());
    }

    #[test]
    fn test_is_completed_oneshot() {
        let mut job = CronJob::new("Test", "unused", "main", "prompt");
        job.schedule_kind = ScheduleKind::At;
        assert!(!is_completed_oneshot(&job));

        job.last_run_at = Some(1000);
        assert!(is_completed_oneshot(&job));
    }

    #[test]
    fn test_is_completed_oneshot_cron_job() {
        let mut job = CronJob::new("Test", "0 * * * *", "main", "prompt");
        job.last_run_at = Some(1000);
        // Cron jobs are never "completed oneshots"
        assert!(!is_completed_oneshot(&job));
    }

    #[cfg(feature = "cron")]
    #[test]
    fn test_compute_next_run_cron_expression() {
        let job = CronJob::new("Test", "0 * * * *", "main", "prompt");
        let now = Utc::now();
        let next = compute_next_run_at(&job, now);
        assert!(next.is_some());
        assert!(next.unwrap() > now.timestamp_millis());
    }
}
```

**Step 2: Register module**

Add `pub mod scheduler;` in `core/src/cron/mod.rs`.

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron -- cron::scheduler::tests`
Expected: ALL PASS (these are pure functions, no DB)

**Step 4: Commit**

```bash
git add core/src/cron/scheduler.rs core/src/cron/mod.rs
git commit -m "cron: add scheduler engine with backoff, next_run_at computation"
```

---

## Task 6: Resource-Aware Scheduling

**Files:**
- Create: `core/src/cron/resource.rs`
- Modify: `core/src/cron/mod.rs` (add `pub mod resource;`)

**Step 1: Write tests and implementation**

Create `core/src/cron/resource.rs`:

```rust
//! Resource-aware scheduling for cron jobs.
//!
//! Adjusts concurrency based on system CPU load to prevent
//! AI "thundering herd" from overwhelming the host.

/// Resolve effective concurrency based on system load.
///
/// - CPU > 80%: limit to 1 (only highest priority)
/// - CPU > 60%: half of configured max
/// - Otherwise: full configured max
///
/// Result is clamped to available semaphore permits.
pub fn resolve_effective_concurrency(
    config_max: usize,
    available_permits: usize,
) -> usize {
    let cpu = get_cpu_usage();

    let limit = if cpu > 0.8 {
        1
    } else if cpu > 0.6 {
        (config_max / 2).max(1)
    } else {
        config_max
    };

    limit.max(1).min(available_permits)
}

/// Get current CPU usage as a fraction (0.0 - 1.0).
#[cfg(feature = "cron")]
fn get_cpu_usage() -> f64 {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_cpu_usage();
    // sysinfo needs a brief delay for accurate readings
    std::thread::sleep(std::time::Duration::from_millis(100));
    sys.refresh_cpu_usage();

    let usage: f64 = sys.cpus().iter().map(|c| c.cpu_usage() as f64).sum::<f64>()
        / sys.cpus().len().max(1) as f64
        / 100.0;

    usage.clamp(0.0, 1.0)
}

#[cfg(not(feature = "cron"))]
fn get_cpu_usage() -> f64 {
    0.0 // No resource awareness without feature
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_concurrency_normal_load() {
        // With low CPU (mocked as 0.0), should return config_max
        let result = resolve_effective_concurrency(5, 10);
        // Can't predict exact CPU, but should be >= 1
        assert!(result >= 1);
        assert!(result <= 10);
    }

    #[test]
    fn test_resolve_concurrency_clamped_to_permits() {
        // Even with full concurrency, can't exceed available permits
        let result = resolve_effective_concurrency(10, 2);
        assert!(result <= 2);
    }

    #[test]
    fn test_resolve_concurrency_minimum_one() {
        let result = resolve_effective_concurrency(1, 1);
        assert_eq!(result, 1);
    }
}
```

**Step 2: Register module, run tests, commit**

```bash
# Register in mod.rs
# Run: cargo test -p alephcore --features cron -- cron::resource::tests
# Commit
git add core/src/cron/resource.rs core/src/cron/mod.rs
git commit -m "cron: add resource-aware scheduling with CPU load gating"
```

---

## Task 7: Job Chain Logic

**Files:**
- Create: `core/src/cron/chain.rs`
- Modify: `core/src/cron/mod.rs` (add `pub mod chain;`)

**Step 1: Write tests and implementation**

Create `core/src/cron/chain.rs`:

```rust
//! Job chain logic for on_success/on_failure triggers.
//!
//! Supports lightweight dependency chains between cron jobs.
//! Includes cycle detection to prevent infinite trigger loops.

use rusqlite::{params, Connection};
use std::collections::HashSet;
use std::path::Path;

use crate::cron::CronResult;

/// Detect if adding a chain link would create a cycle.
///
/// Follows the chain from `new_target` through on_success links.
/// Returns true if the chain leads back to `start_id`.
pub fn detect_cycle_sync(conn: &Connection, start_id: &str, new_target: &str) -> CronResult<bool> {
    let mut visited = HashSet::new();
    let mut current = Some(new_target.to_string());

    while let Some(id) = current {
        if id == start_id {
            return Ok(true); // Cycle detected
        }
        if !visited.insert(id.clone()) {
            break; // Already visited (shouldn't happen in acyclic graph)
        }

        // Follow the chain: check both on_success and on_failure
        let mut stmt = conn.prepare(
            "SELECT next_job_id_on_success, next_job_id_on_failure FROM cron_jobs WHERE id = ?1",
        )?;

        current = stmt
            .query_row(params![id], |row| {
                let on_success: Option<String> = row.get(0)?;
                let on_failure: Option<String> = row.get(1)?;
                Ok(on_success.or(on_failure))
            })
            .unwrap_or(None);
    }

    Ok(false)
}

/// Trigger a chained job by setting its next_run_at to now.
///
/// The job will be picked up in the next scheduler tick.
pub fn trigger_chain_job_sync(
    conn: &Connection,
    target_job_id: &str,
    now_ms: i64,
) -> CronResult<bool> {
    let rows = conn.execute(
        "UPDATE cron_jobs SET next_run_at = ?1 WHERE id = ?2 AND enabled = 1",
        params![now_ms, target_job_id],
    )?;
    Ok(rows > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use crate::cron::CronService;
    use crate::cron::config::CronConfig;

    fn setup_db() -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();

        // Create schema
        conn.execute_batch(r#"
            CREATE TABLE cron_jobs (
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
                next_run_at INTEGER,
                running_at INTEGER,
                last_run_at INTEGER,
                consecutive_failures INTEGER DEFAULT 0,
                max_retries INTEGER DEFAULT 3,
                priority INTEGER DEFAULT 5,
                schedule_kind TEXT DEFAULT 'cron',
                every_ms INTEGER,
                at_time INTEGER,
                delete_after_run INTEGER DEFAULT 0,
                next_job_id_on_success TEXT,
                next_job_id_on_failure TEXT,
                delivery_config TEXT,
                prompt_template TEXT,
                context_vars TEXT,
                version INTEGER DEFAULT 1
            );
        "#).unwrap();

        (dir, conn)
    }

    #[test]
    fn test_no_cycle_simple() {
        let (_dir, conn) = setup_db();

        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at) VALUES ('a', 'A', '0 * * * *', 'main', 'p', 0, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at) VALUES ('b', 'B', '0 * * * *', 'main', 'p', 0, 0)",
            [],
        ).unwrap();

        // A -> B: no cycle
        assert!(!detect_cycle_sync(&conn, "a", "b").unwrap());
    }

    #[test]
    fn test_detect_cycle_direct() {
        let (_dir, conn) = setup_db();

        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at, next_job_id_on_success) VALUES ('a', 'A', '0 * * * *', 'main', 'p', 0, 0, 'b')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at, next_job_id_on_success) VALUES ('b', 'B', '0 * * * *', 'main', 'p', 0, 0, 'a')",
            [],
        ).unwrap();

        // B -> A: cycle (A -> B -> A)
        assert!(detect_cycle_sync(&conn, "a", "b").unwrap());
    }

    #[test]
    fn test_detect_cycle_transitive() {
        let (_dir, conn) = setup_db();

        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at, next_job_id_on_success) VALUES ('a', 'A', '0 * * * *', 'main', 'p', 0, 0, 'b')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at, next_job_id_on_success) VALUES ('b', 'B', '0 * * * *', 'main', 'p', 0, 0, 'c')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at, next_job_id_on_success) VALUES ('c', 'C', '0 * * * *', 'main', 'p', 0, 0, 'a')",
            [],
        ).unwrap();

        // A -> B -> C -> A: cycle
        assert!(detect_cycle_sync(&conn, "a", "b").unwrap());
    }

    #[test]
    fn test_trigger_chain_job() {
        let (_dir, conn) = setup_db();

        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, created_at, updated_at) VALUES ('target', 'Target', '0 * * * *', 'main', 'p', 0, 0)",
            [],
        ).unwrap();

        let now_ms = 1000000;
        let triggered = trigger_chain_job_sync(&conn, "target", now_ms).unwrap();
        assert!(triggered);

        // Verify next_run_at was set
        let next: Option<i64> = conn
            .query_row("SELECT next_run_at FROM cron_jobs WHERE id = 'target'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(next, Some(now_ms));
    }

    #[test]
    fn test_trigger_disabled_job() {
        let (_dir, conn) = setup_db();

        conn.execute(
            "INSERT INTO cron_jobs (id, name, schedule, agent_id, prompt, enabled, created_at, updated_at) VALUES ('disabled', 'Disabled', '0 * * * *', 'main', 'p', 0, 0, 0)",
            [],
        ).unwrap();

        let triggered = trigger_chain_job_sync(&conn, "disabled", 1000).unwrap();
        assert!(!triggered); // Disabled jobs should not be triggered
    }
}
```

**Step 2: Register module, run tests, commit**

```bash
# Add pub mod chain; to mod.rs
# Run: cargo test -p alephcore --features cron -- cron::chain::tests
# Commit
git add core/src/cron/chain.rs core/src/cron/mod.rs
git commit -m "cron: add job chain logic with cycle detection"
```

---

## Task 8: Delivery Pipeline — Trait + Engine

**Files:**
- Create: `core/src/cron/delivery.rs`
- Modify: `core/src/cron/mod.rs` (add `pub mod delivery;`)

**Step 1: Write the delivery trait, engine, and tests**

Create `core/src/cron/delivery.rs`:

```rust
//! Delivery pipeline for cron job results.
//!
//! Supports pluggable delivery targets via the `DeliveryTarget` trait.
//! Built-in targets: Gateway (Telegram/Discord), Webhook, Memory.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

use crate::cron::config::{
    CronJob, DeliveryConfig, DeliveryMode, DeliveryOutcome, DeliveryTargetConfig, JobRun,
};

/// Error type for delivery operations
#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("Invalid delivery config: {0}")]
    InvalidConfig(String),

    #[error("Delivery failed: {0}")]
    Failed(String),

    #[error("Target not registered: {0}")]
    TargetNotRegistered(String),
}

/// Trait for delivery targets.
///
/// Each implementation handles delivering job results to a specific destination.
#[async_trait]
pub trait DeliveryTarget: Send + Sync {
    /// Identifier for this delivery target type
    fn kind(&self) -> &str;

    /// Deliver a job result to the target
    async fn deliver(
        &self,
        job: &CronJob,
        run: &JobRun,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError>;
}

/// Delivery engine that dispatches results to registered targets.
pub struct DeliveryEngine {
    targets: HashMap<String, Arc<dyn DeliveryTarget>>,
}

impl DeliveryEngine {
    pub fn new() -> Self {
        Self {
            targets: HashMap::new(),
        }
    }

    /// Register a delivery target
    pub fn register(&mut self, target: Arc<dyn DeliveryTarget>) {
        self.targets.insert(target.kind().to_string(), target);
    }

    /// Execute delivery for a job result according to its config
    pub async fn deliver(
        &self,
        job: &CronJob,
        run: &JobRun,
        config: &DeliveryConfig,
    ) -> Vec<DeliveryOutcome> {
        let mut outcomes = Vec::new();

        match config.mode {
            DeliveryMode::None => {}
            DeliveryMode::Primary => {
                if let Some(target_cfg) = config.targets.first() {
                    let outcome = self.deliver_to_target(job, run, target_cfg).await;
                    let success = outcome.success;
                    outcomes.push(outcome);

                    // Fallback on failure
                    if !success {
                        if let Some(fallback) = &config.fallback_target {
                            outcomes.push(self.deliver_to_target(job, run, fallback).await);
                        }
                    }
                }
            }
            DeliveryMode::Broadcast => {
                for target_cfg in &config.targets {
                    outcomes.push(self.deliver_to_target(job, run, target_cfg).await);
                }
            }
        }

        outcomes
    }

    /// Deliver to a specific target configuration
    async fn deliver_to_target(
        &self,
        job: &CronJob,
        run: &JobRun,
        config: &DeliveryTargetConfig,
    ) -> DeliveryOutcome {
        let kind = match config {
            DeliveryTargetConfig::Gateway { .. } => "gateway",
            DeliveryTargetConfig::Memory { .. } => "memory",
            DeliveryTargetConfig::Webhook { .. } => "webhook",
        };

        match self.targets.get(kind) {
            Some(target) => match target.deliver(job, run, config).await {
                Ok(outcome) => outcome,
                Err(e) => DeliveryOutcome {
                    target_kind: kind.to_string(),
                    success: false,
                    message: Some(format!("Delivery error: {}", e)),
                },
            },
            None => DeliveryOutcome {
                target_kind: kind.to_string(),
                success: false,
                message: Some(format!("Target '{}' not registered", kind)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Test delivery target that records calls
    struct MockTarget {
        kind: String,
        call_count: AtomicU32,
        should_fail: bool,
    }

    impl MockTarget {
        fn new(kind: &str, should_fail: bool) -> Self {
            Self {
                kind: kind.to_string(),
                call_count: AtomicU32::new(0),
                should_fail,
            }
        }

        fn calls(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DeliveryTarget for MockTarget {
        fn kind(&self) -> &str {
            &self.kind
        }

        async fn deliver(
            &self,
            _job: &CronJob,
            _run: &JobRun,
            _config: &DeliveryTargetConfig,
        ) -> Result<DeliveryOutcome, DeliveryError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if self.should_fail {
                Err(DeliveryError::Failed("mock failure".into()))
            } else {
                Ok(DeliveryOutcome {
                    target_kind: self.kind.clone(),
                    success: true,
                    message: None,
                })
            }
        }
    }

    #[tokio::test]
    async fn test_delivery_none_mode() {
        let engine = DeliveryEngine::new();
        let job = CronJob::new("Test", "0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
        let config = DeliveryConfig {
            mode: DeliveryMode::None,
            targets: vec![],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&job, &run, &config).await;
        assert!(outcomes.is_empty());
    }

    #[tokio::test]
    async fn test_delivery_primary_mode() {
        let mut engine = DeliveryEngine::new();
        let mock = Arc::new(MockTarget::new("webhook", false));
        engine.register(mock.clone());

        let job = CronJob::new("Test", "0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Webhook {
                url: "https://example.com".into(),
                method: None,
                headers: None,
            }],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&job, &run, &config).await;
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].success);
        assert_eq!(mock.calls(), 1);
    }

    #[tokio::test]
    async fn test_delivery_primary_with_fallback() {
        let mut engine = DeliveryEngine::new();
        let failing = Arc::new(MockTarget::new("gateway", true));
        let fallback = Arc::new(MockTarget::new("webhook", false));
        engine.register(failing.clone());
        engine.register(fallback.clone());

        let job = CronJob::new("Test", "0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Gateway {
                channel: "telegram".into(),
                chat_id: "123".into(),
                format: None,
            }],
            fallback_target: Some(DeliveryTargetConfig::Webhook {
                url: "https://fallback.com".into(),
                method: None,
                headers: None,
            }),
        };

        let outcomes = engine.deliver(&job, &run, &config).await;
        assert_eq!(outcomes.len(), 2);
        assert!(!outcomes[0].success); // Primary failed
        assert!(outcomes[1].success); // Fallback succeeded
        assert_eq!(failing.calls(), 1);
        assert_eq!(fallback.calls(), 1);
    }

    #[tokio::test]
    async fn test_delivery_broadcast_mode() {
        let mut engine = DeliveryEngine::new();
        let webhook = Arc::new(MockTarget::new("webhook", false));
        let memory = Arc::new(MockTarget::new("memory", false));
        engine.register(webhook.clone());
        engine.register(memory.clone());

        let job = CronJob::new("Test", "0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
        let config = DeliveryConfig {
            mode: DeliveryMode::Broadcast,
            targets: vec![
                DeliveryTargetConfig::Webhook {
                    url: "https://example.com".into(),
                    method: None,
                    headers: None,
                },
                DeliveryTargetConfig::Memory {
                    tags: vec!["cron".into()],
                    importance: None,
                },
            ],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&job, &run, &config).await;
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|o| o.success));
        assert_eq!(webhook.calls(), 1);
        assert_eq!(memory.calls(), 1);
    }

    #[tokio::test]
    async fn test_delivery_unregistered_target() {
        let engine = DeliveryEngine::new(); // No targets registered
        let job = CronJob::new("Test", "0 * * * *", "main", "prompt");
        let run = JobRun::new("job-1");
        let config = DeliveryConfig {
            mode: DeliveryMode::Primary,
            targets: vec![DeliveryTargetConfig::Webhook {
                url: "https://example.com".into(),
                method: None,
                headers: None,
            }],
            fallback_target: None,
        };

        let outcomes = engine.deliver(&job, &run, &config).await;
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].success);
        assert!(outcomes[0].message.as_ref().unwrap().contains("not registered"));
    }
}
```

**Step 2: Register module, run tests, commit**

```bash
# Add pub mod delivery; to mod.rs
# Run: cargo test -p alephcore --features cron -- cron::delivery::tests
# Commit
git add core/src/cron/delivery.rs core/src/cron/mod.rs
git commit -m "cron: add delivery pipeline with DeliveryTarget trait and engine"
```

---

## Task 9: Webhook Delivery Target

**Files:**
- Create: `core/src/cron/webhook_target.rs`
- Modify: `core/src/cron/mod.rs`

**Step 1: Implement WebhookTarget**

Create `core/src/cron/webhook_target.rs`:

```rust
//! Webhook delivery target for cron job results.
//!
//! Sends job results to external HTTP endpoints.

use async_trait::async_trait;

use crate::cron::config::{CronJob, DeliveryOutcome, DeliveryTargetConfig, JobRun};
use crate::cron::delivery::{DeliveryError, DeliveryTarget};

pub struct WebhookTarget {
    client: reqwest::Client,
}

impl WebhookTarget {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl DeliveryTarget for WebhookTarget {
    fn kind(&self) -> &str {
        "webhook"
    }

    async fn deliver(
        &self,
        job: &CronJob,
        run: &JobRun,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let (url, method, headers) = match config {
            DeliveryTargetConfig::Webhook {
                url,
                method,
                headers,
            } => (url, method, headers),
            _ => return Err(DeliveryError::InvalidConfig("Expected Webhook config".into())),
        };

        let payload = serde_json::json!({
            "job_id": job.id,
            "job_name": job.name,
            "status": run.status.to_string(),
            "response": run.response,
            "error": run.error,
            "duration_ms": run.duration_ms,
            "started_at": run.started_at,
            "ended_at": run.ended_at,
        });

        let method = method.as_deref().unwrap_or("POST");
        let mut request = match method {
            "PUT" => self.client.put(url),
            _ => self.client.post(url),
        };

        request = request
            .header("Content-Type", "application/json")
            .json(&payload);

        // Add custom headers
        if let Some(hdrs) = headers {
            for (key, value) in hdrs {
                request = request.header(key.as_str(), value.as_str());
            }
        }

        match request.send().await {
            Ok(resp) if resp.status().is_success() => Ok(DeliveryOutcome {
                target_kind: "webhook".to_string(),
                success: true,
                message: Some(format!("HTTP {}", resp.status())),
            }),
            Ok(resp) => Err(DeliveryError::Failed(format!(
                "HTTP {} from {}",
                resp.status(),
                url
            ))),
            Err(e) => Err(DeliveryError::Failed(format!("Request failed: {}", e))),
        }
    }
}
```

**Step 2: Register module, commit**

```bash
# Add pub mod webhook_target; to mod.rs
git add core/src/cron/webhook_target.rs core/src/cron/mod.rs
git commit -m "cron: add webhook delivery target"
```

---

## Task 10: Integrate New Scheduler into CronService

**Files:**
- Modify: `core/src/cron/mod.rs`

This is the largest task. It replaces the existing `check_and_run_jobs` with the new state-machine scheduler.

**Step 1: Write integration test**

Add to tests in `mod.rs`:

```rust
#[tokio::test]
async fn test_scheduler_startup_computes_next_run() {
    let config = test_config();
    let service = CronService::new(config).unwrap();

    let job = CronJob::new("Hourly Job", "0 * * * *", "main", "Do work");
    let job_id = job.id.clone();
    service.add_job(job).await.unwrap();

    // After adding, next_run_at should be computed
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
```

**Step 2: Update add_job to compute next_run_at on creation**

In `add_job`, after validating the schedule and before the INSERT, compute `next_run_at`:

```rust
// Compute initial next_run_at
let mut job = job;
if job.next_run_at.is_none() && job.enabled {
    job.next_run_at = crate::cron::scheduler::compute_next_run_at(&job, Utc::now());
}
```

**Step 3: Replace check_and_run_jobs with new scheduler tick**

Replace the `#[cfg(feature = "cron")] check_and_run_jobs` method (lines 592-741) with the new state-machine based implementation that:

1. Calls `clear_stuck_jobs` (UPDATE running_at = NULL WHERE running_at < cutoff)
2. Uses `atomic_acquire` (BEGIN IMMEDIATE; SELECT WHERE next_run_at <= now AND running_at IS NULL; UPDATE SET running_at = now)
3. Spawns jobs with template rendering
4. In finalize: updates next_run_at, handles backoff, triggers chains, runs delivery

**Step 4: Add startup_catchup method**

Add a method that runs on `start()`:

```rust
async fn startup_catchup(db_path: &Path) -> CronResult<()> {
    // Phase 1: Clear all running_at markers
    // Phase 2: Find overdue jobs
    // Phase 3: Execute missed (skip completed one-shots)
    // Phase 4: Recompute all next_run_at
}
```

Call it from `start()` before entering the main loop.

**Step 5: Run all tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron -- cron::tests`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add core/src/cron/mod.rs
git commit -m "cron: replace window-based scheduling with state-machine engine"
```

---

## Task 11: Wire Up Gateway Handlers

**Files:**
- Modify: `core/src/gateway/handlers/cron.rs`

**Step 1: Replace stub implementations with real logic**

The gateway handlers need access to a shared `CronService` instance. This requires passing a reference through the handler context. The exact wiring depends on how other handlers access shared state (check the pattern in adjacent handlers).

Key changes:
- `handle_list` → calls `service.list_jobs()`
- `handle_status` → returns service state (running, job_count, next_tick)
- `handle_run` → triggers immediate execution of a job
- Add new handlers: `handle_add`, `handle_update`, `handle_delete`

**Step 2: Register new handlers in mod.rs**

Update the handler registry to include `cron.add`, `cron.update`, `cron.delete`.

**Step 3: Run tests, commit**

```bash
git add core/src/gateway/handlers/cron.rs core/src/gateway/handlers/mod.rs
git commit -m "cron: wire gateway RPC handlers to CronService"
```

---

## Task 12: Final Integration Test & Cleanup

**Files:**
- Modify: `core/src/cron/mod.rs` (re-export new modules)

**Step 1: Add comprehensive integration test**

```rust
#[tokio::test]
async fn test_full_job_lifecycle() {
    // 1. Create service
    // 2. Add a job with delivery config
    // 3. Verify next_run_at computed
    // 4. Verify template rendering works
    // 5. Verify job chaining doesn't create cycles
    // 6. Cleanup
}
```

**Step 2: Update module re-exports in mod.rs**

```rust
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
```

**Step 3: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --features cron`
Expected: ALL PASS

**Step 4: Commit**

```bash
git add -A core/src/cron/
git commit -m "cron: complete redesign with delivery, templates, chains, resource awareness"
```

---

## Summary

| Task | Description | Files | Estimated Steps |
|------|------------|-------|-----------------|
| 1 | Add dependencies | Cargo.toml | 3 |
| 2 | Extend data types | config.rs | 10 |
| 3 | Migrate SQLite schema | mod.rs | 9 |
| 4 | Prompt template engine | template.rs (new) | 6 |
| 5 | Scheduler core logic | scheduler.rs (new) | 4 |
| 6 | Resource-aware scheduling | resource.rs (new) | 3 |
| 7 | Job chain logic | chain.rs (new) | 3 |
| 8 | Delivery pipeline | delivery.rs (new) | 3 |
| 9 | Webhook target | webhook_target.rs (new) | 2 |
| 10 | Integrate into CronService | mod.rs | 6 |
| 11 | Gateway handlers | handlers/cron.rs | 3 |
| 12 | Final integration | mod.rs | 4 |
