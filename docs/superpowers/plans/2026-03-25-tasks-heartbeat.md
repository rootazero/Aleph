# Tasks & Heartbeat System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor `src/cron/` → `src/tasks/` with shared infrastructure, and add a heartbeat subsystem with L1 probe + L2 Agent two-level execution, semantic embedding dedup, wake request queue, and UI redesign.

**Architecture:** Dual-track system under `tasks/` — cron and heartbeat share Store/Delivery/Clock/Schedule infrastructure but have independent execution engines and timer loops. Heartbeat uses L1 Tool-based probes to filter empty polls before triggering L2 Agent turns.

**Tech Stack:** Rust async (tokio), SQLite (rusqlite WAL), Leptos (WASM UI), serde_json, async-trait

**Spec:** `docs/superpowers/specs/2026-03-25-tasks-heartbeat-design.md`

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `src/tasks/mod.rs` | Top-level module, re-exports SharedCronService + SharedHeartbeatService |
| `src/tasks/shared/mod.rs` | Shared sub-module root |
| `src/tasks/shared/store.rs` | `TaskDatabase` — unified SQLite connection wrapper |
| `src/tasks/shared/delivery.rs` | Generalized `DeliveryTarget` trait + `DeliveryPayload` + `DeliveryEngine` |
| `src/tasks/heartbeat/mod.rs` | `HeartbeatService`, `SharedHeartbeatService` |
| `src/tasks/heartbeat/config.rs` | `HeartbeatTask`, `ProbeConfig`, `TriggerCondition`, `HeartbeatState`, `HeartbeatConfig`, `DedupConfig` |
| `src/tasks/heartbeat/store.rs` | `HeartbeatStore` — SQLite CRUD for heartbeat_tasks table |
| `src/tasks/heartbeat/probe.rs` | `ProbeExecutor` trait + `execute_probe()` + `ProbeResult` |
| `src/tasks/heartbeat/dedup.rs` | `DedupEngine` — semantic embedding dedup |
| `src/tasks/heartbeat/wake.rs` | `WakeQueue` + `WakeRequest` + `WakePriority` |
| `src/tasks/heartbeat/executor.rs` | L2 Agent turn executor + `heartbeat_report` tool |
| `src/tasks/heartbeat/service/mod.rs` | Service sub-module root |
| `src/tasks/heartbeat/service/state.rs` | `HeartbeatServiceState` with AtomicBool guards |
| `src/tasks/heartbeat/service/ops.rs` | CRUD operations + schedule recomputation |
| `src/tasks/heartbeat/service/timer.rs` | Heartbeat timer loop (10s tick + wake select) |
| `src/tasks/heartbeat/history.rs` | SQLite schema + insert/query for heartbeat_runs |
| `src/gateway/handlers/heartbeat.rs` | 8 JSON-RPC handlers (heartbeat.*) |
| `src/builtin_tools/heartbeat_manage.rs` | LLM-callable tools for heartbeat CRUD |
| `interfaces/webchat/src/api/heartbeat.rs` | Frontend DTO + JSON-RPC API wrapper |
| `interfaces/webchat/src/views/tasks.rs` | Unified tasks view with cron + heartbeat tabs |

### Moved Files (git mv)

| From | To |
|------|-----|
| `src/cron/mod.rs` | `src/tasks/cron/mod.rs` |
| `src/cron/config.rs` | `src/tasks/cron/config.rs` |
| `src/cron/store.rs` | `src/tasks/cron/store.rs` |
| `src/cron/executor.rs` | `src/tasks/cron/executor.rs` |
| `src/cron/schedule.rs` | `src/tasks/shared/schedule.rs` |
| `src/cron/clock.rs` | `src/tasks/shared/clock.rs` |
| `src/cron/history.rs` | `src/tasks/cron/history.rs` |
| `src/cron/chain.rs` | `src/tasks/cron/chain.rs` |
| `src/cron/alert.rs` | `src/tasks/cron/alert.rs` |
| `src/cron/stagger.rs` | `src/tasks/cron/stagger.rs` |
| `src/cron/template.rs` | `src/tasks/cron/template.rs` |
| `src/cron/webhook_target.rs` | `src/tasks/cron/webhook_target.rs` |
| `src/cron/execution/*` | `src/tasks/cron/execution/*` |
| `src/cron/service/*` | `src/tasks/cron/service/*` |

### Modified Files

| File | Change |
|------|--------|
| `src/lib.rs:109` | `pub mod cron` → `pub mod tasks` |
| `src/config/structs.rs:6,109,319` | `cron::CronConfig` → `tasks::cron::CronConfig`, add `heartbeat: HeartbeatConfig` |
| `src/bin/aleph-server/commands/start/mod.rs:21-24,446-460,629-669` | Update imports, add HeartbeatService init |
| `src/bin/aleph-server/commands/start/builder/handlers.rs:787-808` | Update imports, add heartbeat handler registration |
| `src/gateway/handlers/mod.rs` | Add `pub mod heartbeat` |
| `src/gateway/handlers/cron.rs:13-18` | Update `use crate::cron::` → `use crate::tasks::cron::` |
| `src/builtin_tools/cron_manage.rs:13` | Update import path |
| `src/cron/delivery.rs` → `src/tasks/shared/delivery.rs` | Generalize `DeliveryTarget` to accept `DeliveryPayload` |
| All `src/cron/**/*.rs` internal imports | `use crate::cron::` → `use crate::tasks::cron::` or `use crate::tasks::shared::` |
| `interfaces/webchat/src/api.rs:7` | `pub mod cron` stays, add `pub mod heartbeat` |
| `interfaces/webchat/src/views/mod.rs:7` | `pub mod cron` → `pub mod tasks` |
| `interfaces/webchat/src/views/cron.rs` → `views/tasks.rs` | Wrap in tabs, add heartbeat tab |
| `interfaces/webchat/src/app.rs:12,121` | Update route from cron to tasks |

---

## Task 1: Directory Migration — Move cron/ to tasks/cron/

**Files:**
- Move: `src/cron/` → `src/tasks/cron/`
- Create: `src/tasks/mod.rs`
- Modify: `src/lib.rs:109`

- [ ] **Step 1: Create tasks directory and move cron files**

```bash
mkdir -p src/tasks
git mv src/cron src/tasks/cron
```

- [ ] **Step 2: Create tasks/mod.rs**

```rust
// src/tasks/mod.rs
pub mod cron;
```

- [ ] **Step 3: Update lib.rs module declaration**

In `src/lib.rs`, change line 109:
```rust
// Before:
pub mod cron;
// After:
pub mod tasks;
```

- [ ] **Step 4: Batch update all internal cron imports**

All `use crate::cron::` inside `src/tasks/cron/**/*.rs` must become `use crate::tasks::cron::`. Files to update (internal cross-references):

```bash
# Use perl for reliable batch replacement within tasks/cron/ only
find src/tasks/cron -name '*.rs' -exec perl -pi -e 's/use crate::cron::/use crate::tasks::cron::/g' {} +
```

Verify no references to old path remain:
```bash
grep -r "use crate::cron::" src/tasks/
```

- [ ] **Step 5: Update external imports**

Files that import from `crate::cron` outside the cron module:

1. `src/config/structs.rs:6` — `use crate::cron::CronConfig` → `use crate::tasks::cron::CronConfig`
2. `src/gateway/handlers/cron.rs:13-18` — update all `use crate::cron::` → `use crate::tasks::cron::`
3. `src/builtin_tools/cron_manage.rs:13` — update import path
4. `src/bin/aleph-server/commands/start/mod.rs:21-24` — `use alephcore::cron::` → `use alephcore::tasks::cron::`
5. `src/bin/aleph-server/commands/start/builder/handlers.rs` — update `alephcore::cron::` references

```bash
# Verify all external references updated
grep -rn "use.*cron::" src/ --include='*.rs' | grep -v "tasks/cron/"
# Should only show the gateway/handlers/cron.rs file references (already updated above)
```

- [ ] **Step 6: Compile check**

```bash
cargo check -p alephcore 2>&1 | head -50
```

Expected: compiles cleanly with no `cron` path errors.

- [ ] **Step 7: Run tests**

```bash
cargo test -p alephcore --lib 2>&1 | tail -20
```

Expected: all existing cron tests pass.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor: move cron/ to tasks/cron/ — directory migration"
```

---

## Task 2: Extract Shared Infrastructure — clock, schedule, delivery

**Files:**
- Create: `src/tasks/shared/mod.rs`, `src/tasks/shared/store.rs`
- Move: `src/tasks/cron/clock.rs` → `src/tasks/shared/clock.rs`
- Move: `src/tasks/cron/schedule.rs` → `src/tasks/shared/schedule.rs`
- Move + Modify: `src/tasks/cron/delivery.rs` → `src/tasks/shared/delivery.rs`

- [ ] **Step 1: Create shared module structure**

```bash
mkdir -p src/tasks/shared
```

```rust
// src/tasks/shared/mod.rs
pub mod clock;
pub mod delivery;
pub mod schedule;
pub mod store;
```

- [ ] **Step 2: Move clock.rs and schedule.rs**

```bash
git mv src/tasks/cron/clock.rs src/tasks/shared/clock.rs
git mv src/tasks/cron/schedule.rs src/tasks/shared/schedule.rs
```

Update internal imports in moved files:
- `shared/schedule.rs`: any `use crate::tasks::cron::clock` → `use crate::tasks::shared::clock`
- `shared/clock.rs`: self-contained, no changes needed

- [ ] **Step 3: Update all consumers of clock and schedule**

All files that imported `crate::tasks::cron::clock` or `crate::tasks::cron::schedule` now import from `crate::tasks::shared::`:

```bash
find src/tasks/cron -name '*.rs' -exec perl -pi -e \
  's/use crate::tasks::cron::clock/use crate::tasks::shared::clock/g; s/use crate::tasks::cron::schedule/use crate::tasks::shared::schedule/g' {} +
```

Also update `src/gateway/handlers/cron.rs` if it imports clock.

- [ ] **Step 4: Update cron/mod.rs module declarations**

Remove `pub mod clock` and `pub mod schedule` from `src/tasks/cron/mod.rs`. Add re-exports if needed for backward compatibility:

```rust
// In tasks/cron/mod.rs, remove these lines:
// pub mod clock;
// pub mod schedule;

// Add re-exports for backward compat within cron:
pub use crate::tasks::shared::clock;
pub use crate::tasks::shared::schedule;
```

- [ ] **Step 5: Update tasks/mod.rs**

```rust
// src/tasks/mod.rs
pub mod cron;
pub mod shared;
```

- [ ] **Step 6: Move delivery types to shared**

First, extract delivery-related types from `cron/config.rs` into a new shared location. The types `DeliveryConfig`, `DeliveryMode`, `DeliveryTargetConfig`, `DeliveryOutcome`, `DeliveryStatus` are used by both cron and heartbeat, so they belong in shared.

Move these types from `src/tasks/cron/config.rs` to `src/tasks/shared/delivery.rs` (they will live alongside the `DeliveryTarget` trait and `DeliveryEngine`).

In `cron/config.rs`, replace the moved type definitions with re-exports:
```rust
pub use crate::tasks::shared::delivery::{
    DeliveryConfig, DeliveryMode, DeliveryTargetConfig, DeliveryOutcome, DeliveryStatus,
};
```

This avoids circular dependencies: shared/delivery.rs defines both the types AND the trait, cron/config.rs re-exports them for backward compatibility.

- [ ] **Step 7: Move and generalize delivery.rs**

```bash
git mv src/tasks/cron/delivery.rs src/tasks/shared/delivery.rs
```

In `src/tasks/shared/delivery.rs`:
1. Move the delivery types from Step 6 into this file
2. Add the generic `DeliveryPayload` struct
3. Update `DeliveryTarget` trait to accept `&DeliveryPayload` instead of `&CronJob, &JobRun`
4. Update `DeliveryEngine::deliver()` and `deliver_to_target()` signatures
5. Update the test module at bottom of file to use new signatures

```rust
/// Generic delivery payload for both cron and heartbeat
#[derive(Debug, Clone, Serialize)]
pub struct DeliveryPayload {
    pub source_type: String,
    pub task_name: String,
    pub agent_id: String,
    pub output: String,
    pub channel_id: Option<String>,
    pub metadata: serde_json::Value,
}

#[async_trait]
pub trait DeliveryTarget: Send + Sync {
    fn kind(&self) -> &str;
    async fn deliver(
        &self,
        payload: &DeliveryPayload,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError>;
}
```

- [ ] **Step 8: Update webhook_target.rs to match new trait**

In `src/tasks/cron/webhook_target.rs`, update `DeliveryTarget::deliver` impl to accept `&DeliveryPayload` instead of `&CronJob, &JobRun`. Update import from `crate::tasks::shared::delivery`.

- [ ] **Step 9: Add DeliveryPayload conversion to CronJob**

In `src/tasks/cron/config.rs`, add:

```rust
use crate::tasks::shared::delivery::DeliveryPayload;

impl CronJob {
    pub fn to_delivery_payload(&self, output: &str, run: &JobRun) -> DeliveryPayload {
        DeliveryPayload {
            source_type: "cron".to_string(),
            task_name: self.name.clone(),
            agent_id: self.agent_id.clone().unwrap_or_default(),
            output: output.to_string(),
            channel_id: self.source_channel_id.clone(),
            metadata: serde_json::json!({
                "job_id": self.id,
                "run_id": run.id,
                "trigger_source": run.trigger_source.as_str(),
            }),
        }
    }
}
```

Update all call sites in `cron/service/concurrency.rs` and `cron/service/timer.rs` that call `delivery_engine.deliver()`.

- [ ] **Step 10: Update cron/mod.rs re-exports**

In `src/tasks/cron/mod.rs`:
1. Remove `pub mod delivery`
2. Update the `pub use` line that re-exports `DeliveryEngine, DeliveryTarget` to point to shared:
```rust
pub use crate::tasks::shared::delivery::{DeliveryEngine, DeliveryTarget};
```

- [ ] **Step 11: Create shared/store.rs — TaskDatabase wrapper**

```rust
// src/tasks/shared/store.rs
use rusqlite::Connection;
use std::path::Path;

/// Unified SQLite database for both cron and heartbeat
pub struct TaskDatabase {
    conn: Connection,
}

impl TaskDatabase {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}
```

Note: CronStore currently owns its own `Connection`. We will NOT change CronStore's internals in this task — that would be too risky. Instead, `TaskDatabase` is used by the new HeartbeatStore. CronStore continues to own its own connection (same DB file, WAL mode allows concurrent readers). This pragmatic approach minimizes cron regressions.

- [ ] **Step 12: Update cron db_path default**

In `src/tasks/cron/config.rs`, update the default `db_path` from `"~/.aleph/data/cron.db"` to `"~/.aleph/data/tasks.db"`.

- [ ] **Step 13: Compile check and test**

```bash
cargo check -p alephcore 2>&1 | head -50
cargo test -p alephcore --lib 2>&1 | tail -20
```

- [ ] **Step 14: Commit**

```bash
git add -A
git commit -m "refactor: extract shared infrastructure (clock, schedule, delivery, store)"
```

---

## Task 3: Heartbeat Types & Configuration

**Files:**
- Create: `src/tasks/heartbeat/mod.rs`
- Create: `src/tasks/heartbeat/config.rs`
- Modify: `src/tasks/mod.rs`
- Modify: `src/config/structs.rs`

- [ ] **Step 1: Write tests for config types**

```rust
// In src/tasks/heartbeat/config.rs (at bottom, #[cfg(test)] mod)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_condition_serde_roundtrip() {
        let conditions = vec![
            TriggerCondition::NonEmpty,
            TriggerCondition::GreaterThan(5.0),
            TriggerCondition::Contains("error".to_string()),
            TriggerCondition::Changed,
            TriggerCondition::Always,
        ];
        for cond in conditions {
            let json = serde_json::to_string(&cond).unwrap();
            let back: TriggerCondition = serde_json::from_str(&json).unwrap();
            assert_eq!(format!("{:?}", cond), format!("{:?}", back));
        }
    }

    #[test]
    fn test_heartbeat_config_defaults() {
        let config = HeartbeatConfig::default();
        assert!(config.enabled);
        assert_eq!(config.tick_interval_secs, 10);
        assert_eq!(config.max_concurrent, 3);
        assert_eq!(config.job_timeout_secs, 120);
    }

    #[test]
    fn test_error_backoff_ms() {
        assert_eq!(error_backoff_ms(0), 0);
        assert_eq!(error_backoff_ms(1), 30_000);
        assert_eq!(error_backoff_ms(2), 60_000);
        assert_eq!(error_backoff_ms(5), 3_600_000);
        assert_eq!(error_backoff_ms(100), 3_600_000); // capped
    }

    #[test]
    fn test_heartbeat_task_new() {
        let task = HeartbeatTask::new(
            "Test Task".to_string(),
            "main".to_string(),
            300_000,
            ProbeConfig {
                tool_name: "gmail.unread_count".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::GreaterThan(0.0),
            },
        );
        assert!(task.enabled);
        assert_eq!(task.interval_ms, 300_000);
        assert_eq!(task.consecutive_errors(), 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib heartbeat::config 2>&1 | tail -20
```

Expected: FAIL (module not found)

- [ ] **Step 3: Implement heartbeat config types**

Create `src/tasks/heartbeat/config.rs` with all types from the spec:

```rust
use serde::{Deserialize, Serialize};

// HeartbeatConfig, HeartbeatTask, ProbeConfig, TriggerCondition,
// HeartbeatState, DedupConfig, HeartbeatRunRecord, HeartbeatTickResult,
// HeartbeatL2Status, error_backoff_ms()
// (Full implementations per spec — see design doc sections "Type System" and "Error Backoff")
```

Key types: `HeartbeatTask`, `ProbeConfig`, `TriggerCondition`, `HeartbeatState`, `HeartbeatConfig`, `DedupConfig`, `error_backoff_ms()`.

Also create `src/tasks/heartbeat/mod.rs`:

```rust
pub mod config;
```

Update `src/tasks/mod.rs`:
```rust
pub mod cron;
pub mod heartbeat;
pub mod shared;
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alephcore --lib heartbeat::config 2>&1 | tail -20
```

Expected: all 4 tests PASS.

- [ ] **Step 5: Add HeartbeatConfig to app config**

In `src/config/structs.rs`, add:
```rust
use crate::tasks::heartbeat::config::HeartbeatConfig;

// In AppConfig struct, after `cron: CronConfig`:
#[serde(default)]
pub heartbeat: HeartbeatConfig,
```

And in `Default` impl:
```rust
heartbeat: HeartbeatConfig::default(),
```

- [ ] **Step 6: Compile check and test**

```bash
cargo check -p alephcore 2>&1 | head -50
cargo test -p alephcore --lib 2>&1 | tail -20
```

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(heartbeat): add config types and HeartbeatConfig"
```

---

## Task 4: HeartbeatStore — SQLite Persistence

**Files:**
- Create: `src/tasks/heartbeat/store.rs`
- Create: `src/tasks/heartbeat/history.rs`
- Modify: `src/tasks/heartbeat/mod.rs`

- [ ] **Step 1: Write store tests**

```rust
// In src/tasks/heartbeat/store.rs (at bottom)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::heartbeat::config::*;

    fn make_test_task(name: &str) -> HeartbeatTask {
        HeartbeatTask::new(
            name.to_string(),
            "main".to_string(),
            300_000,
            ProbeConfig {
                tool_name: "test.probe".to_string(),
                tool_params: None,
                trigger_condition: TriggerCondition::Always,
            },
        )
    }

    #[test]
    fn test_store_crud() {
        let store = HeartbeatStore::open_in_memory().unwrap();
        let mut store = store;

        // Add
        let task = make_test_task("Test 1");
        let id = task.id.clone();
        store.add_task(task);
        assert_eq!(store.tasks().len(), 1);

        // Get
        let t = store.get_task(&id).unwrap();
        assert_eq!(t.name, "Test 1");

        // Update
        store.get_task_mut(&id).unwrap().name = "Updated".to_string();
        store.mark_dirty();
        assert_eq!(store.get_task(&id).unwrap().name, "Updated");

        // Persist and reload
        store.persist().unwrap();
        store.force_reload().unwrap();
        assert_eq!(store.get_task(&id).unwrap().name, "Updated");

        // Delete
        store.remove_task(&id);
        store.persist().unwrap();
        assert!(store.get_task(&id).is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib heartbeat::store 2>&1 | tail -20
```

- [ ] **Step 3: Implement HeartbeatStore**

Create `src/tasks/heartbeat/store.rs`. Follow `CronStore` pattern exactly — in-memory `Vec<HeartbeatTask>` + dirty flag + SQLite backend. Table: `heartbeat_tasks (id, name, agent_id, enabled, data)`.

- [ ] **Step 4: Implement heartbeat history**

Create `src/tasks/heartbeat/history.rs`. Table: `heartbeat_runs` with L1/L2 split fields per spec. Functions: `init_schema()`, `insert_run_record()`, `get_run_records()`, `prune_old_records()`.

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p alephcore --lib heartbeat::store 2>&1 | tail -20
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(heartbeat): add HeartbeatStore and history persistence"
```

---

## Task 5: ProbeExecutor — L1 Probe Framework

**Files:**
- Create: `src/tasks/heartbeat/probe.rs`
- Modify: `src/tasks/heartbeat/mod.rs`

- [ ] **Step 1: Write probe tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evaluate_trigger_non_empty() {
        assert!(evaluate_trigger(&TriggerCondition::NonEmpty, &json!("hello"), None));
        assert!(evaluate_trigger(&TriggerCondition::NonEmpty, &json!(42), None));
        assert!(!evaluate_trigger(&TriggerCondition::NonEmpty, &json!(null), None));
        assert!(!evaluate_trigger(&TriggerCondition::NonEmpty, &json!(""), None));
        assert!(!evaluate_trigger(&TriggerCondition::NonEmpty, &json!(0), None));
    }

    #[test]
    fn test_evaluate_trigger_greater_than() {
        assert!(evaluate_trigger(&TriggerCondition::GreaterThan(5.0), &json!(10), None));
        assert!(!evaluate_trigger(&TriggerCondition::GreaterThan(5.0), &json!(3), None));
        assert!(!evaluate_trigger(&TriggerCondition::GreaterThan(5.0), &json!("not a number"), None));
    }

    #[test]
    fn test_evaluate_trigger_contains() {
        assert!(evaluate_trigger(&TriggerCondition::Contains("error".to_string()), &json!("found an error"), None));
        assert!(!evaluate_trigger(&TriggerCondition::Contains("error".to_string()), &json!("all good"), None));
    }

    #[test]
    fn test_evaluate_trigger_changed() {
        assert!(evaluate_trigger(&TriggerCondition::Changed, &json!("new"), Some("old")));
        assert!(!evaluate_trigger(&TriggerCondition::Changed, &json!("same"), Some("same")));
        assert!(evaluate_trigger(&TriggerCondition::Changed, &json!("first"), None)); // no previous = changed
    }

    #[test]
    fn test_evaluate_trigger_always() {
        assert!(evaluate_trigger(&TriggerCondition::Always, &json!(null), None));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib heartbeat::probe 2>&1 | tail -20
```

- [ ] **Step 3: Implement probe module**

Create `src/tasks/heartbeat/probe.rs`:

```rust
use async_trait::async_trait;
use serde_json::Value;
use crate::tasks::heartbeat::config::{ProbeConfig, TriggerCondition};

#[derive(Debug)]
pub struct ProbeResult {
    pub raw_value: Value,
    pub triggered: bool,
    pub duration_ms: i64,
}

#[async_trait]
pub trait ProbeExecutor: Send + Sync {
    async fn execute(
        &self,
        tool_name: &str,
        params: Option<&Value>,
    ) -> Result<Value, String>;
}

/// Pure function: evaluate trigger condition against probe result
pub fn evaluate_trigger(
    condition: &TriggerCondition,
    value: &Value,
    last_result: Option<&str>,
) -> bool {
    match condition {
        TriggerCondition::NonEmpty => !is_empty_value(value),
        TriggerCondition::GreaterThan(threshold) => {
            value.as_f64().map(|v| v > *threshold).unwrap_or(false)
        }
        TriggerCondition::Contains(s) => {
            value.as_str().map(|v| v.contains(s.as_str())).unwrap_or(false)
        }
        TriggerCondition::Changed => {
            let current = value.to_string();
            last_result.map(|prev| prev != current).unwrap_or(true)
        }
        TriggerCondition::Always => true,
    }
}

pub async fn execute_probe(
    probe: &ProbeConfig,
    executor: &dyn ProbeExecutor,
    last_probe_result: Option<&str>,
) -> Result<ProbeResult, String> {
    let start = std::time::Instant::now();
    let raw_value = executor.execute(&probe.tool_name, probe.tool_params.as_ref()).await?;
    let duration_ms = start.elapsed().as_millis() as i64;
    let triggered = evaluate_trigger(&probe.trigger_condition, &raw_value, last_probe_result);
    Ok(ProbeResult { raw_value, triggered, duration_ms })
}

fn is_empty_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Number(n) => n.as_f64().map(|f| f == 0.0).unwrap_or(false),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Bool(b) => !b,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p alephcore --lib heartbeat::probe 2>&1 | tail -20
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(heartbeat): add ProbeExecutor trait and L1 trigger evaluation"
```

---

## Task 6: DedupEngine — Semantic Embedding Dedup

**Files:**
- Create: `src/tasks/heartbeat/dedup.rs`
- Modify: `src/tasks/heartbeat/mod.rs`

- [ ] **Step 1: Write dedup tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_dedup_config_defaults() {
        let config = DedupConfig::default();
        assert_eq!(config.window_ms, 86_400_000);
        assert!((config.similarity_threshold - 0.85).abs() < 0.01);
        assert_eq!(config.max_history, 10);
    }
}
```

- [ ] **Step 2: Implement DedupEngine**

Create `src/tasks/heartbeat/dedup.rs`:
- `cosine_similarity(a, b) -> f32` — pure function
- `DedupEngine` struct holding DB connection + `Arc<dyn EmbeddingProvider>`
- `is_duplicate(task_id, output) -> bool` — read DB (lock), release, embed (async), relock, compare
- `record(task_id, output)` — embed, store BLOB + model name
- `cleanup(retention_ms)` — delete expired
- `prune_old(task_id, max_history)` — keep only newest N per task

SQLite schema: `heartbeat_dedup (id, task_id, output_text, embedding BLOB, model TEXT, created_at INTEGER)`.

Key: DB lock is only held during read/write, NOT during embedding API call.

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib heartbeat::dedup 2>&1 | tail -20
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(heartbeat): add DedupEngine with semantic embedding dedup"
```

---

## Task 7: WakeQueue — Event-Driven Wake Requests

**Files:**
- Create: `src/tasks/heartbeat/wake.rs`
- Modify: `src/tasks/heartbeat/mod.rs`

- [ ] **Step 1: Write wake queue tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_and_drain() {
        let queue = WakeQueue::new();
        queue.enqueue(WakeRequest {
            task_id: "t1".to_string(),
            priority: WakePriority::Interval,
            reason: None,
            requested_at: 1000,
        });
        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].task_id, "t1");
        assert!(queue.drain().is_empty()); // empty after drain
    }

    #[test]
    fn test_coalesce_keeps_higher_priority() {
        let queue = WakeQueue::new();
        queue.enqueue(WakeRequest {
            task_id: "t1".to_string(),
            priority: WakePriority::Interval,
            reason: Some("timer".to_string()),
            requested_at: 1000,
        });
        queue.enqueue(WakeRequest {
            task_id: "t1".to_string(),
            priority: WakePriority::UserAction,
            reason: Some("manual".to_string()),
            requested_at: 2000,
        });
        let drained = queue.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].priority, WakePriority::UserAction);
        assert_eq!(drained[0].reason.as_deref(), Some("manual"));
    }

    #[test]
    fn test_multiple_tasks() {
        let queue = WakeQueue::new();
        queue.enqueue(WakeRequest { task_id: "t1".to_string(), priority: WakePriority::Interval, reason: None, requested_at: 0 });
        queue.enqueue(WakeRequest { task_id: "t2".to_string(), priority: WakePriority::SystemEvent, reason: None, requested_at: 0 });
        assert_eq!(queue.drain().len(), 2);
    }
}
```

- [ ] **Step 2: Implement WakeQueue**

Create `src/tasks/heartbeat/wake.rs`:

```rust
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum WakePriority {
    Interval = 0,
    SystemEvent = 1,
    UserAction = 2,
}

#[derive(Debug, Clone)]
pub struct WakeRequest {
    pub task_id: String,
    pub priority: WakePriority,
    pub reason: Option<String>,
    pub requested_at: i64,
}

pub struct WakeQueue {
    pending: Mutex<HashMap<String, WakeRequest>>,
    notify: Notify,
}

impl WakeQueue {
    pub fn new() -> Self { /* ... */ }
    pub fn enqueue(&self, req: WakeRequest) { /* coalesce by task_id, keep higher priority, notify */ }
    pub fn drain(&self) -> Vec<WakeRequest> { /* take all pending, return vec */ }
    pub async fn notified(&self) { self.notify.notified().await; }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib heartbeat::wake 2>&1 | tail -20
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(heartbeat): add WakeQueue with priority coalescing"
```

---

## Task 8: HeartbeatService — State, Ops, Timer Loop

**Files:**
- Create: `src/tasks/heartbeat/service/mod.rs`
- Create: `src/tasks/heartbeat/service/state.rs`
- Create: `src/tasks/heartbeat/service/ops.rs`
- Create: `src/tasks/heartbeat/service/timer.rs`
- Create: `src/tasks/heartbeat/executor.rs`
- Modify: `src/tasks/heartbeat/mod.rs`

- [ ] **Step 1: Implement HeartbeatServiceState**

`service/state.rs`: Follows cron's `ServiceState` pattern — `Arc<tokio::sync::Mutex<HeartbeatStore>>`, `AtomicBool` for running/shutdown, `HeartbeatConfig`.

- [ ] **Step 2: Implement CRUD ops**

`service/ops.rs`: `list_tasks()`, `get_task()`, `add_task()`, `update_task()`, `delete_task()`, `toggle_task()`. Schedule recomputation on add/update/toggle.

- [ ] **Step 3: Implement L2 executor**

`executor.rs`: `execute_heartbeat_l2()` — builds prompt with probe result summary, calls `ExecutionAdapter`, parses response (prefer `heartbeat_report` tool call, fallback to protocol tokens).

- [ ] **Step 4: Implement timer loop**

`service/timer.rs`: The main `run_heartbeat_loop()` — `tokio::select!` on sleep + wake, `compare_exchange` guard, collect due tasks, `for` loop with semaphore, `execute_heartbeat_tick()` (L1→L2→dedup→deliver), writeback.

Refer to spec section "Timer Loop & Concurrency" for exact pseudocode.

- [ ] **Step 5: Implement DefaultProbeExecutor**

Create `src/tasks/heartbeat/probe.rs` addition — `DefaultProbeExecutor` struct:

```rust
/// Production ProbeExecutor that routes between builtin and MCP tools
pub struct DefaultProbeExecutor {
    tool_registry: Arc<BuiltinToolRegistry>,
    // For MCP tools, we need access to MCP client connections.
    // Use the existing McpManager or equivalent from gateway.
}

#[async_trait]
impl ProbeExecutor for DefaultProbeExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        params: Option<&Value>,
    ) -> Result<Value, String> {
        // 1. Check if tool_name is a builtin tool via tool_registry
        // 2. If builtin: call tool_registry.execute_tool(name, args)
        // 3. If MCP tool (contains '.'): route via MCP client
        // 4. Return tool result as Value
    }
}
```

This bridges the `ProbeExecutor` trait (heartbeat-specific) with the actual tool infrastructure. MCP tool routing can be deferred to a follow-up if MCP integration is complex — start with builtin tools only, add MCP support incrementally.

- [ ] **Step 6: Wire up HeartbeatService in mod.rs**

`heartbeat/mod.rs`: `HeartbeatService` struct wrapping state + wake_queue + probe_executor + adapter + delivery + dedup. Public methods: `new()`, `list_tasks()`, `get_task()`, `add_task()`, `update_task()`, `delete_task()`, `toggle_task()`, `wake_task()`, `run_heartbeat_loop()`.

`SharedHeartbeatService = Arc<tokio::sync::Mutex<HeartbeatService>>`

- [ ] **Step 6: Compile check**

```bash
cargo check -p alephcore 2>&1 | head -50
```

- [ ] **Step 7: Write integration test for timer tick**

Test that a heartbeat task with `TriggerCondition::Always` + mock ProbeExecutor + mock ExecutionAdapter produces the expected flow: L1 triggers → L2 executes → result recorded.

- [ ] **Step 8: Run tests**

```bash
cargo test -p alephcore --lib heartbeat 2>&1 | tail -20
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(heartbeat): add HeartbeatService with timer loop and L1/L2 execution"
```

---

## Task 9: Gateway Handlers + LLM Tools

**Files:**
- Create: `src/gateway/handlers/heartbeat.rs`
- Create: `src/builtin_tools/heartbeat_manage.rs`
- Modify: `src/gateway/handlers/mod.rs`
- Modify: `src/bin/aleph-server/commands/start/mod.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs`

- [ ] **Step 1: Implement gateway handlers**

Create `src/gateway/handlers/heartbeat.rs` — 8 handlers following `cron.rs` patterns:
- `handle_heartbeat_list`, `handle_heartbeat_get`, `handle_heartbeat_create`, `handle_heartbeat_update`, `handle_heartbeat_delete`, `handle_heartbeat_toggle`, `handle_heartbeat_wake`, `handle_heartbeat_runs`

Add `pub mod heartbeat;` to `src/gateway/handlers/mod.rs`.

- [ ] **Step 2: Register handlers in builder**

In `src/bin/aleph-server/commands/start/builder/handlers.rs`, add `register_heartbeat_handlers()` function following `register_cron_handlers()` pattern. Register 8 RPC methods: `heartbeat.list`, `heartbeat.get`, etc.

- [ ] **Step 3: Implement builtin tools**

Create `src/builtin_tools/heartbeat_manage.rs` — LLM-callable tools: `heartbeat_create`, `heartbeat_list`, `heartbeat_update`, `heartbeat_delete`, `heartbeat_toggle`. Follow `cron_manage.rs` pattern.

Register in `src/executor/builtin_registry/` (definitions + registry match arms).

- [ ] **Step 4: Implement migrate_task_db()**

Add to `src/tasks/shared/store.rs`:

```rust
pub fn migrate_task_db(old_cron_path: &Path, new_path: &Path) -> Result<(), String> {
    // 1. If tasks.db exists → ensure heartbeat tables exist → done
    if new_path.exists() {
        let conn = Connection::open(new_path).map_err(|e| e.to_string())?;
        create_heartbeat_tables_if_not_exist(&conn)?;
        return Ok(());
    }
    // 2. If cron.db exists → rename (or copy for cross-fs) to tasks.db
    if old_cron_path.exists() {
        std::fs::rename(old_cron_path, new_path)
            .or_else(|_| std::fs::copy(old_cron_path, new_path).map(|_| ()))
            .map_err(|e| e.to_string())?;
    }
    // 3. Open (or create fresh) tasks.db, ensure all tables exist
    let conn = Connection::open(new_path).map_err(|e| e.to_string())?;
    create_heartbeat_tables_if_not_exist(&conn)?;
    Ok(())
}
```

- [ ] **Step 5: Update server startup**

In `src/bin/aleph-server/commands/start/mod.rs`:
1. Add imports for HeartbeatService
2. Call `migrate_task_db()` before creating CronService
3. Create HeartbeatService after CronService
4. Spawn heartbeat timer loop after cron timer loop
5. Call `register_heartbeat_handlers()` in builder

- [ ] **Step 6: Wire heartbeat into shutdown handler**

Find the server's SIGTERM/SIGINT handler (in `start/mod.rs` or equivalent). Add `heartbeat_service.request_shutdown()` alongside the existing cron shutdown. The heartbeat timer loop will drain in-flight tasks with a `job_timeout_secs + 5s` deadline before the process exits.

- [ ] **Step 5: Compile and test**

```bash
cargo check -p alephcore 2>&1 | head -50
cargo test -p alephcore --lib 2>&1 | tail -20
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(heartbeat): add gateway handlers, LLM tools, and server initialization"
```

---

## Task 10: WebChat UI — Tasks View with Tabs

**Files:**
- Create: `interfaces/webchat/src/api/heartbeat.rs`
- Create: `interfaces/webchat/src/views/tasks.rs`
- Modify: `interfaces/webchat/src/api.rs`
- Modify: `interfaces/webchat/src/views/mod.rs`
- Modify: `interfaces/webchat/src/app.rs`

- [ ] **Step 1: Create heartbeat API module**

`interfaces/webchat/src/api/heartbeat.rs` — DTO structs (`HeartbeatTaskInfo`, `CreateHeartbeatTask`, `UpdateHeartbeatTask`, `HeartbeatRunInfo`) + API trait with JSON-RPC calls. Follow `api/cron.rs` pattern.

Add `pub mod heartbeat;` to `api.rs`.

- [ ] **Step 2: Create unified tasks view**

`interfaces/webchat/src/views/tasks.rs` — Leptos component with two tabs:
- Tab 1 "定时任务": Embed existing `CronView` component (from `views/cron.rs`)
- Tab 2 "心跳任务": New heartbeat UI with:
  - Left panel: task list (status indicator, agent, interval, L1/L2 counters)
  - Right panel: task editor (name, agent, interval, probe config, delivery, dedup, history)

- [ ] **Step 3: Update routing**

In `views/mod.rs`:
```rust
pub mod tasks;  // Add alongside existing pub mod cron
```

In `app.rs`, add route:
```rust
"/dashboard/tasks" => view! { <TasksView /> }
```

Keep the old `/dashboard/cron` route pointing to `CronView` for backward compatibility.

- [ ] **Step 4: Update sidebar navigation**

Find where "定时任务" appears in the sidebar and change it to "任务", pointing to `/dashboard/tasks`.

- [ ] **Step 5: Build WASM and verify**

```bash
cd interfaces/webchat && trunk build 2>&1 | tail -20
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(heartbeat): add tasks view with cron + heartbeat tabs"
```

---

## Task 11: Final Cleanup

**Files:**
- Remove: old `src/cron/` directory (if any remnants)

Note: `migrate_task_db()` was implemented in Task 9 Step 4.

- [ ] **Step 1: Verify no stale paths remain**

```bash
# Should return nothing:
grep -rn "crate::cron\b" src/ --include='*.rs' | grep -v "tasks/cron/" | grep -v "tasks/shared/"
grep -rn "alephcore::cron\b" src/ --include='*.rs' | grep -v "tasks/cron/"
ls src/cron/ 2>/dev/null  # Should not exist
```

- [ ] **Step 3: Full test suite**

```bash
cargo test -p alephcore --lib 2>&1 | tail -30
cargo check -p alephcore 2>&1 | head -20
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: add DB migration and clean up stale cron paths"
```

---

## Task 12: Final Integration Test + Clippy

**Files:** No new files

- [ ] **Step 1: Run full test suite**

```bash
cargo test -p alephcore --lib 2>&1
```

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p alephcore -- -W clippy::all 2>&1 | head -50
```

Fix any warnings.

- [ ] **Step 3: Verify WASM builds**

```bash
cd interfaces/webchat && trunk build 2>&1 | tail -20
```

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "chore: fix clippy warnings and verify full build"
```
