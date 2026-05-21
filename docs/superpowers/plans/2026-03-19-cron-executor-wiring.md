# Cron Executor Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire up the cron system's missing executor and timer loop so scheduled jobs actually execute and push results to channels.

**Architecture:** The cron infrastructure (storage, scheduling, 3-phase concurrency, catchup, RPC) is complete. We need to: (1) add `source_channel_id` to data model, (2) build a production `JobExecutorFn` that bridges `JobSnapshot` → `ExecutionAdapter` + `AgentRegistry`, (3) spawn the timer loop at server startup, (4) auto-capture channel context in `cron_manage` tool.

**Tech Stack:** Rust, tokio, SQLite (via CronStore), ExecutionAdapter trait, AgentRegistry, ReplyEmitter

**Spec:** `docs/superpowers/specs/2026-03-19-cron-executor-wiring-design.md`

---

### Task 1: Add `source_channel_id` to CronJob and JobSnapshot

**Files:**
- Modify: `src/cron/config.rs:301-411` (CronJob struct) and `src/cron/config.rs:416-428` (JobSnapshot struct)
- Test: existing tests in `src/cron/config.rs`

- [ ] **Step 1: Add `source_channel_id` field to `CronJob`**

In `src/cron/config.rs`, add after the `agent_id` field (line 310):

```rust
/// Channel where this cron job was created (for result delivery)
#[serde(default)]
pub source_channel_id: Option<String>,
```

Also update `CronJob::new()` (around line 384) to include:
```rust
source_channel_id: None,
```

- [ ] **Step 2: Add `source_channel_id` field to `JobSnapshot`**

In `src/cron/config.rs`, add to `JobSnapshot` struct (after `agent_id` field, line 419):

```rust
/// Channel to deliver results to
pub source_channel_id: Option<String>,
```

- [ ] **Step 3: Add `source_channel_id` to `CronJobView`**

In `src/cron/config.rs`, add to `CronJobView` struct and its `From<&CronJob>` impl:

```rust
// In struct (after agent_id):
pub source_channel_id: Option<String>,

// In From impl:
source_channel_id: job.source_channel_id.clone(),
```

- [ ] **Step 4: Run tests to verify no breakage**

Run: `cargo test -p alephcore --lib cron::config`
Expected: All existing tests PASS (serde defaults handle the new `Option<String>` field)

- [ ] **Step 5: Commit**

```bash
git add src/cron/config.rs
git commit -m "cron: add source_channel_id to CronJob and JobSnapshot"
```

---

### Task 2: Update snapshot construction in concurrency.rs

**Files:**
- Modify: `src/cron/service/concurrency.rs:55-65` (phase1_mark_due_jobs snapshot) and `src/cron/service/concurrency.rs:98-108` (phase1_mark_manual snapshot)
- Test: existing tests in same file

- [ ] **Step 1: Add `source_channel_id` to phase1_mark_due_jobs snapshot**

In `src/cron/service/concurrency.rs`, inside the `phase1_mark_due_jobs` function, update the `JobSnapshot` construction (around line 55-65) to include:

```rust
source_channel_id: job.source_channel_id.clone(),
```

- [ ] **Step 2: Add `source_channel_id` to phase1_mark_manual snapshot**

Same change in `phase1_mark_manual` (around line 98-108):

```rust
source_channel_id: job.source_channel_id.clone(),
```

- [ ] **Step 3: Update test helper `make_snapshot` in timer.rs tests**

In `src/cron/service/timer.rs` test module, update `make_snapshot` (around line 206):

```rust
fn make_snapshot(id: &str) -> JobSnapshot {
    JobSnapshot {
        id: id.to_string(),
        agent_id: Some("test-agent".to_string()),
        source_channel_id: None,
        prompt: "test prompt".to_string(),
        // ... rest unchanged
    }
}
```

- [ ] **Step 4: Update test helper `make_execution_result` in concurrency.rs tests if needed**

Check the `make_test_job` helper — `CronJob::new()` already sets `source_channel_id: None` from Task 1, so no change needed here.

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib cron::service`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add src/cron/service/concurrency.rs src/cron/service/timer.rs
git commit -m "cron: carry source_channel_id through job snapshots"
```

---

### Task 3: Remove old `JobExecutor` type alias

**Files:**
- Modify: `src/cron/mod.rs:58-63`

- [ ] **Step 1: Remove the old `JobExecutor` type alias**

In `src/cron/mod.rs`, delete lines 58-63:

```rust
// DELETE THIS:
/// Callback for job execution
pub type JobExecutor = Arc<
    dyn Fn(String, String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;
```

- [ ] **Step 2: Check for any remaining references**

Run: `cargo check -p alephcore 2>&1 | head -30`

If any code references `JobExecutor`, update those references. Based on exploration, no production code uses it.

- [ ] **Step 3: Commit**

```bash
git add src/cron/mod.rs
git commit -m "cron: remove unused JobExecutor type alias"
```

---

### Task 4: Build the production CronExecutor

**Files:**
- Create: `src/cron/executor.rs`
- Modify: `src/cron/mod.rs` (add `pub mod executor;`)

This is the core new code — bridges `JobSnapshot` → agent execution → result delivery.

- [ ] **Step 1: Create `src/cron/executor.rs` with the executor function builder**

```rust
//! Production cron job executor.
//!
//! Bridges `JobSnapshot` → `ExecutionAdapter` + `AgentRegistry` → `ExecutionResult`.
//! The executor creates a `RunRequest`, looks up the agent, injects cron context
//! into the prompt, and runs the agent through the standard execution pipeline.

use std::collections::HashMap;

use tracing::{error, info, warn};
use uuid::Uuid;

use crate::cron::config::{
    DeliveryStatus, ErrorReason, ExecutionResult, JobSnapshot, RunStatus, SessionTarget,
};
use crate::cron::service::timer::JobExecutorFn;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_emitter::NoOpEventEmitter;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::router::SessionKey;
use crate::sync_primitives::Arc;

/// Build a `JobExecutorFn` closure that captures the execution dependencies.
///
/// The returned closure:
/// 1. Looks up the agent from the registry
/// 2. Constructs a `SessionKey::Task` for the cron session
/// 3. Injects cron context into the prompt (including channel delivery instruction)
/// 4. Executes via `ExecutionAdapter`
/// 5. Returns `ExecutionResult` for phase 3 writeback
pub fn build_cron_executor_fn(
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
) -> JobExecutorFn {
    Arc::new(move |snapshot: JobSnapshot| {
        let adapter = Arc::clone(&execution_adapter);
        let registry = Arc::clone(&agent_registry);

        Box::pin(async move { execute_cron_job(adapter, registry, snapshot).await })
    })
}

/// Execute a single cron job snapshot.
async fn execute_cron_job(
    adapter: Arc<dyn ExecutionAdapter>,
    registry: Arc<AgentRegistry>,
    snapshot: JobSnapshot,
) -> ExecutionResult {
    let started_at = chrono::Utc::now().timestamp_millis();
    let job_id = &snapshot.id;

    // Resolve agent_id (default to "main")
    let agent_id = snapshot
        .agent_id
        .as_deref()
        .unwrap_or("main");

    // Look up agent instance
    let agent = match registry.get(agent_id).await {
        Some(a) => a,
        None => {
            warn!(job_id, agent_id, "Cron executor: agent not found");
            return make_error_result(
                started_at,
                format!("Agent '{}' not found in registry", agent_id),
                ErrorReason::Permanent(format!("agent '{}' not registered", agent_id)),
            );
        }
    };

    // Build session key
    let task_id = match snapshot.session_target {
        SessionTarget::Main => snapshot.id.clone(),
        SessionTarget::Isolated => format!("{}-{}", snapshot.id, started_at),
    };
    let session_key = SessionKey::task(agent_id, "cron", &task_id);

    // Build prompt with cron context injection
    let prompt = build_cron_prompt(&snapshot);

    // Build run request
    let run_id = Uuid::new_v4().to_string();
    let timeout_secs = snapshot.timeout_ms.map(|ms| (ms / 1000) as u64);
    let mut metadata = HashMap::new();
    metadata.insert("cron_job_id".to_string(), snapshot.id.clone());
    metadata.insert("trigger_source".to_string(), snapshot.trigger_source.as_str().to_string());
    if let Some(ref channel_id) = snapshot.source_channel_id {
        metadata.insert("source_channel_id".to_string(), channel_id.clone());
    }

    let request = RunRequest {
        run_id: run_id.clone(),
        input: prompt,
        session_key,
        timeout_secs,
        metadata,
    };

    // Create a no-op emitter for cron (no streaming needed — agent handles delivery via message tool)
    let emitter: Arc<dyn EventEmitter + Send + Sync> = Arc::new(NoOpEventEmitter::new());

    info!(job_id, agent_id, run_id = %run_id, "Cron executor: starting job");

    // Execute
    let exec_result = adapter.execute(request, agent, emitter).await;

    let ended_at = chrono::Utc::now().timestamp_millis();
    let duration_ms = ended_at - started_at;

    match exec_result {
        Ok(()) => {
            info!(job_id, agent_id, duration_ms, "Cron executor: job completed");
            ExecutionResult {
                started_at,
                ended_at,
                duration_ms,
                status: RunStatus::Ok,
                output: None,
                error: None,
                error_reason: None,
                delivery_status: Some(DeliveryStatus::AlreadySentByAgent),
                agent_used_messaging_tool: true, // agent handles delivery via prompt
            }
        }
        Err(e) => {
            let error_msg = format!("{}", e);
            error!(job_id, agent_id, error = %error_msg, "Cron executor: job failed");

            let is_timeout = error_msg.contains("timeout") || error_msg.contains("Timeout");
            ExecutionResult {
                started_at,
                ended_at,
                duration_ms,
                status: if is_timeout { RunStatus::Timeout } else { RunStatus::Error },
                output: None,
                error: Some(error_msg.clone()),
                error_reason: Some(ErrorReason::Transient(error_msg)),
                delivery_status: Some(DeliveryStatus::NotDelivered),
                agent_used_messaging_tool: false,
            }
        }
    }
}

/// Build the cron prompt with context injection.
///
/// Injects delivery instructions so the agent sends results via the message tool.
fn build_cron_prompt(snapshot: &JobSnapshot) -> String {
    let mut parts = Vec::new();

    parts.push(format!("[Cron Task: {}]", snapshot.id));

    if let Some(ref channel_id) = snapshot.source_channel_id {
        parts.push(format!(
            "You are executing a scheduled task. After completing the task, \
             send the results to channel '{}' using the message tool.",
            channel_id
        ));
    }

    parts.push(String::new()); // blank line
    parts.push(snapshot.prompt.clone());

    parts.join("\n")
}

/// Helper to create an error ExecutionResult.
fn make_error_result(started_at: i64, error: String, reason: ErrorReason) -> ExecutionResult {
    let ended_at = chrono::Utc::now().timestamp_millis();
    ExecutionResult {
        started_at,
        ended_at,
        duration_ms: ended_at - started_at,
        status: RunStatus::Error,
        output: None,
        error: Some(error),
        error_reason: Some(reason),
        delivery_status: Some(DeliveryStatus::NotDelivered),
        agent_used_messaging_tool: false,
    }
}

```

Uses the existing `NoOpEventEmitter` from `src/gateway/event_emitter/impls.rs` — no custom event collector needed.

- [ ] **Step 2: Add `pub mod executor;` to `src/cron/mod.rs`**

Add after the existing module declarations (around line 42):

```rust
pub mod executor;
```

- [ ] **Step 3: Run compile check**

Run: `cargo check -p alephcore 2>&1 | head -40`
Expected: Compiles without errors

- [ ] **Step 4: Write unit test for `build_cron_prompt`**

Add at the bottom of `executor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::config::{SessionTarget, TriggerSource};

    fn make_test_snapshot() -> JobSnapshot {
        JobSnapshot {
            id: "test-job-1".to_string(),
            agent_id: Some("main".to_string()),
            source_channel_id: Some("discord:general".to_string()),
            prompt: "Check the weather".to_string(),
            model: None,
            timeout_ms: Some(300_000),
            delivery: None,
            session_target: SessionTarget::Isolated,
            marked_at: 1_000_000,
            trigger_source: TriggerSource::Schedule,
        }
    }

    #[test]
    fn test_build_cron_prompt_with_channel() {
        let snapshot = make_test_snapshot();
        let prompt = build_cron_prompt(&snapshot);

        assert!(prompt.contains("[Cron Task: test-job-1]"));
        assert!(prompt.contains("discord:general"));
        assert!(prompt.contains("message tool"));
        assert!(prompt.contains("Check the weather"));
    }

    #[test]
    fn test_build_cron_prompt_without_channel() {
        let mut snapshot = make_test_snapshot();
        snapshot.source_channel_id = None;
        let prompt = build_cron_prompt(&snapshot);

        assert!(prompt.contains("[Cron Task: test-job-1]"));
        assert!(!prompt.contains("message tool"));
        assert!(prompt.contains("Check the weather"));
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib cron::executor`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add src/cron/executor.rs src/cron/mod.rs
git commit -m "cron: add production executor bridging JobSnapshot to ExecutionAdapter"
```

---

### Task 5: Wire timer loop at server startup

**Files:**
- Modify: `src/bin/aleph/commands/start/mod.rs` (around line 436-470)

- [ ] **Step 1: Add imports at top of file**

Add these imports to `src/bin/aleph/commands/start/mod.rs`:

```rust
use alephcore::cron::executor::build_cron_executor_fn;
use alephcore::cron::service::timer::run_timer_loop;
use alephcore::cron::service::catchup::run_startup_catchup;
```

- [ ] **Step 2: Spawn timer loop after agent handler registration**

Find the section after `register_agent_handlers()` returns (around line 471). After the `agent_result` handling, add:

```rust
// Spawn cron timer loop (after agent handlers are registered so AgentRegistry exists)
if let Some(ref cron_svc) = cron_service {
    if let Some(ref agent_reg) = agent_registry_for_cron {
        if let Some(ref exec_adapter) = execution_adapter_for_cron {
            let cron_state = {
                let guard = cron_svc.lock().await;
                guard.state().clone()
            };
            let executor_fn = build_cron_executor_fn(
                Arc::clone(exec_adapter),
                Arc::clone(agent_reg),
            );
            let cron_config = cron_state.config.clone();
            tokio::spawn(async move {
                // Run startup catchup to handle missed jobs
                match run_startup_catchup(
                    &cron_state.store,
                    cron_state.clock.as_ref(),
                    cron_config.max_missed_jobs_per_restart,
                    cron_config.catchup_stagger_ms,
                ).await {
                    Ok(report) => {
                        if report.stale_markers_cleared > 0 || report.immediate_count > 0 || report.deferred_count > 0 {
                            tracing::info!(
                                stale_cleared = report.stale_markers_cleared,
                                immediate = report.immediate_count,
                                deferred = report.deferred_count,
                                "Cron startup catchup complete"
                            );
                        }
                    }
                    Err(e) => tracing::error!("Cron startup catchup failed: {}", e),
                }
                // Start the timer loop (runs until shutdown)
                run_timer_loop(cron_state, executor_fn).await;
            });
            if !args.daemon {
                println!("Cron timer loop: started (check interval: {}s)",
                    cron_svc.lock().await.state().config.check_interval_secs);
            }
        }
    }
}
```

**Important:** The exact variable names for `agent_registry` and `execution_adapter` at this point in the startup code need to be determined by reading the surrounding code. They may be named `agent_registry`, `execution_adapter`, or extracted from the `agent_result`. Adjust accordingly during implementation.

- [ ] **Step 3: Run compile check**

Run: `cargo check -p alephcore --bin aleph 2>&1 | head -40`
Expected: Compiles. If variable names are wrong, adjust based on compiler errors.

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph/commands/start/mod.rs
git commit -m "cron: spawn timer loop and catchup at server startup"
```

---

### Task 6: Auto-capture `source_channel_id` in cron_manage tool

**Files:**
- Modify: `src/builtin_tools/cron_manage.rs:154-163` (CronManageTool struct and constructor)
- Modify: `src/builtin_tools/cron_manage.rs:190-216` (Create action)

- [ ] **Step 1: Add `source_channel_id` field to `CronManageTool` struct**

In `src/builtin_tools/cron_manage.rs`, update the struct and constructor:

```rust
/// Tool for managing cron/scheduled tasks via natural language.
#[derive(Clone)]
pub struct CronManageTool {
    service: SharedCronService,
    /// Channel ID where this tool instance is operating (injected at construction)
    source_channel_id: Option<String>,
}

impl CronManageTool {
    pub fn new(service: SharedCronService) -> Self {
        Self {
            service,
            source_channel_id: None,
        }
    }

    /// Create with a source channel context
    pub fn with_channel(service: SharedCronService, channel_id: Option<String>) -> Self {
        Self {
            service,
            source_channel_id: channel_id,
        }
    }
}
```

- [ ] **Step 2: Set `source_channel_id` on created CronJob**

In the `Create` action handler (around line 204), after creating the `CronJob`:

```rust
let mut job = CronJob::new(&name, &agent_id, &prompt, schedule_kind);
job.source_channel_id = self.source_channel_id.clone();
```

- [ ] **Step 3: Find where `CronManageTool::new()` is called and update if channel context is available**

Search for `CronManageTool::new` in the codebase. If it's constructed in a context where the channel ID is available (e.g., in `BuiltinToolConfig` or agent handler setup), update to use `with_channel()`. If not immediately available, leave as `new()` — the field defaults to `None` and can be set later.

Run: `grep -rn "CronManageTool::new" src/`

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib builtin_tools::cron_manage`
Expected: PASS (if tests exist; otherwise just compile check)

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/cron_manage.rs
git commit -m "cron: auto-capture source_channel_id in cron_manage tool"
```

---

### Task 7: Integration verification

**Files:**
- No new files — verification only

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: Clean compile, no errors

- [ ] **Step 2: Run all cron tests**

Run: `cargo test -p alephcore --lib cron`
Expected: All PASS

- [ ] **Step 3: Run full core test suite**

Run: `cargo test -p alephcore --lib`
Expected: All PASS (pre-existing `markdown_skill::loader` failures are known, ignore those)

- [ ] **Step 4: Manual smoke test (if server is runnable)**

```bash
# Kill any existing processes
pkill -f "target/debug/aleph" 2>/dev/null; sleep 2

# Build and start
cargo run --bin aleph -- start

# In another terminal, check cron status via RPC or create a test job
```

- [ ] **Step 5: Final commit if any fixups needed**

```bash
git add -A
git commit -m "cron: integration fixes for executor wiring"
```
