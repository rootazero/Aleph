# Tasks & Heartbeat System Design

> Aleph 任务系统重构：将"定时任务"升级为"任务"，新增心跳任务子系统。

## Background

Aleph 现有 `cron/` 模块提供完整的定时任务能力（Cron 表达式、Interval、One-shot），但缺少轻量级心跳感知机制。参考 OpenClaw 的 heartbeat-runner 双层架构，结合 Aleph 自身的 Rust 异步优势和 HEARTBEAT.md 身份文件，设计一个超越 OpenClaw 的任务系统。

## Key Decisions

| 决策 | 选择 | 理由 |
|------|------|------|
| 心跳感知源 | LLM 探针 + 系统事件驱动 + 环境上下文注入 | R6(AI 主动到达) + R9(工具即一切) |
| 执行模式 | L1 探针 + L2 Agent turn 两级 | R3(核心轻量化)：系统做轻量判断，LLM 做重量推理 |
| 配置粒度 | 用户创建的独立实体 | 与 CronJob CRUD 模式一致，灵活组合 |
| L1 探针实现 | 基于 Tool 的检查器 | R9(工具即一切)：任何 MCP 工具都可成为探针 |
| 去重策略 | 语义 embedding 去重 | 复用已有 embedding provider，捕获语义等价变体 |
| HEARTBEAT.md 定位 | 纯 prompt 内容层 | 职责分离：心跳任务管 When+Whether，HEARTBEAT.md 管 What |
| 模块命名 | `tasks/` (cron + heartbeat + shared) | 语义清晰，cron API 向后兼容 |

## Module Structure

```
core/src/tasks/
├── mod.rs                           # Top-level exports
├── shared/
│   ├── mod.rs
│   ├── store.rs                     # TaskDatabase: unified SQLite connection
│   ├── history.rs                   # Shared cleanup logic
│   ├── delivery.rs                  # DeliveryEngine + DeliveryTarget trait
│   ├── clock.rs                     # Clock trait (SystemClock, FakeClock)
│   └── schedule.rs                  # Pure scheduling computation functions
├── cron/                            # Scheduled tasks (migrated from cron/)
│   ├── mod.rs                       # CronService, SharedCronService
│   ├── config.rs                    # CronJob, ScheduleKind, JobStateV2, CronConfig
│   ├── service/
│   │   ├── mod.rs
│   │   ├── state.rs                 # ServiceState<C: Clock>
│   │   ├── ops.rs                   # CRUD + schedule recomputation
│   │   ├── timer.rs                 # Timer loop + worker pool
│   │   ├── concurrency.rs          # Three-phase execution model
│   │   └── catchup.rs              # Missed job catchup
│   ├── execution/
│   │   ├── mod.rs
│   │   ├── isolated.rs
│   │   └── lightweight.rs
│   ├── executor.rs
│   ├── template.rs
│   ├── stagger.rs
│   ├── webhook_target.rs
│   ├── alert.rs
│   └── chain.rs
└── heartbeat/                       # Heartbeat tasks (new)
    ├── mod.rs                       # HeartbeatService, SharedHeartbeatService
    ├── config.rs                    # HeartbeatTask, ProbeConfig, HeartbeatState
    ├── service/
    │   ├── mod.rs
    │   ├── state.rs                 # HeartbeatServiceState
    │   ├── ops.rs                   # CRUD operations
    │   └── timer.rs                 # Heartbeat timer loop (10s tick)
    ├── probe.rs                     # L1 probe execution framework
    ├── dedup.rs                     # Semantic embedding dedup engine
    ├── wake.rs                      # Wake request queue with priority coalescing
    └── executor.rs                  # L2 Agent turn executor
```

## Type System

### HeartbeatTask

```rust
pub struct HeartbeatTask {
    pub id: String,                          // UUID
    pub name: String,
    pub agent_id: String,                    // Target agent (reads its HEARTBEAT.md)
    pub enabled: bool,
    pub interval_ms: i64,                    // Heartbeat interval (e.g. 300_000 = 5min)
    pub probe: ProbeConfig,                  // L1 probe configuration
    pub delivery_config: Option<DeliveryConfig>,  // Reuse cron delivery
    pub dedup: DedupConfig,                  // Per-task dedup settings
    pub state: HeartbeatState,
    pub created_at: i64,
    pub updated_at: i64,
}
```

### ProbeConfig

```rust
pub struct ProbeConfig {
    pub tool_name: String,                   // Probe tool (e.g. "gmail.unread_count")
    pub tool_params: Option<serde_json::Value>,
    pub trigger_condition: TriggerCondition,
}

pub enum TriggerCondition {
    NonEmpty,                    // Return value is non-empty/non-null/non-zero
    GreaterThan(f64),            // Numeric value > threshold
    Contains(String),            // Return value contains string
    Changed,                     // Different from last probe result
    Always,                      // Always trigger L2 (degrades to pure timed)
}
```

### HeartbeatState

```rust
pub struct HeartbeatState {
    pub next_due_ms: Option<i64>,
    pub last_probe_at_ms: Option<i64>,
    pub last_probe_result: Option<String>,   // For Changed condition
    pub last_l2_at_ms: Option<i64>,
    pub last_l2_status: Option<RunStatus>,   // Reuse cron's RunStatus
    pub last_output_hash: Option<String>,
    pub consecutive_errors: u32,
    pub last_error: Option<String>,
}
```

### DedupConfig

```rust
pub struct DedupConfig {
    pub window_ms: i64,              // Time window (default 86_400_000 = 24h)
    pub similarity_threshold: f32,   // Cosine similarity threshold (default 0.85)
    pub max_history: usize,          // Max records per task (default 10)
}
```

### HeartbeatConfig

```rust
pub struct HeartbeatConfig {
    pub enabled: bool,                       // Global switch (default true)
    pub tick_interval_secs: u64,             // Timer tick (default 10)
    pub max_concurrent: usize,               // Max concurrent heartbeats (default 3)
    pub job_timeout_secs: u64,               // L2 timeout (default 120)
    pub history_retention_days: u32,         // History retention (default 30)
    pub dedup: DedupConfig,                  // Default dedup config
}
```

## Execution Flow: L1 + L2 Two-Level Model

### Overview

```
Timer tick (every 10s) + Wake events via tokio::select!
  │
  ├─ Scan due heartbeat tasks + drain wake queue
  │
  └─ For each due task (concurrent via Semaphore):
       │
       ├─ [L1 Probe] Call tool via ToolRegistry, no LLM
       │    ├─ Condition NOT met → skip, advance next_due_ms
       │    └─ Condition met → enter L2
       │
       └─ [L2 Execute] Full Agent turn
            ├─ Build prompt: probe result summary + HEARTBEAT.md (already in system prompt)
            ├─ Agent replies ALEPH_HEARTBEAT_OK → silent, skip
            ├─ Agent replies ALEPH_NEEDS_ATTENTION or other → check dedup
            │    ├─ Semantic duplicate → skip
            │    └─ Not duplicate → deliver via DeliveryEngine
            └─ Record embedding to dedup history
```

### L1 Probe Execution

```rust
// tasks/heartbeat/probe.rs

pub struct ProbeResult {
    pub raw_value: serde_json::Value,
    pub triggered: bool,
    pub duration_ms: i64,
}

/// Execute probe: direct Tool call, bypasses LLM entirely
pub async fn execute_probe(
    probe: &ProbeConfig,
    tool_registry: &ToolRegistry,
    last_probe_result: Option<&str>,
) -> Result<ProbeResult>;
```

L1 directly calls tools via ToolRegistry. This is Aleph's core advantage over OpenClaw — Rust's ToolRegistry provides complete tool invocation infrastructure; L1 simply reuses it.

### L2 Execution

```rust
// tasks/heartbeat/executor.rs

pub enum HeartbeatL2Status {
    Silent,                          // Agent said nothing to report
    NeedsDelivery(String),           // Needs delivery to user
    Error(String),
}

/// L2: trigger full Agent turn with probe context
pub async fn execute_heartbeat_l2(
    task: &HeartbeatTask,
    probe_result: &ProbeResult,
    wake_reason: Option<&str>,
    adapter: &dyn ExecutionAdapter,
) -> Result<HeartbeatL2Result>;
```

L2 prompt injects probe result summary: "Heartbeat probe detected change: {probe_summary}. Process per HEARTBEAT.md." The agent's HEARTBEAT.md content is already in its system prompt via IdentityFilesLayer.

### L1 Capability Boundary

L1 answers "has data changed?" not "is this change worth bothering the user about?" — that judgment belongs to L2 (the LLM). Users who don't want L1 filtering can set `TriggerCondition::Always`.

## Wake Request Queue

```rust
// tasks/heartbeat/wake.rs

pub enum WakePriority {
    Interval = 0,    // Scheduled timer (lowest)
    SystemEvent = 1, // System event trigger
    UserAction = 2,  // Manual wake (highest)
}

pub struct WakeRequest {
    pub task_id: String,
    pub priority: WakePriority,
    pub reason: Option<String>,
    pub requested_at: i64,
}

pub struct WakeQueue {
    pending: Mutex<HashMap<String, WakeRequest>>,  // Coalesce by task_id
    notify: tokio::sync::Notify,                    // Wake timer loop
}
```

Multiple requests for the same task are coalesced, keeping the highest priority. External systems (Gateway, Daemon, MCP callbacks) inject events via `wake_queue.enqueue()`. The timer loop responds immediately via `tokio::select!`.

**Advantages over OpenClaw**: Type-safe WakeRequest with enum priorities (vs string concatenation), `tokio::select!` instant response (vs JavaScript setTimeout 250ms coalescing delay), wake still goes through L1→L2 pipeline (may be filtered at L1, saving tokens).

## Semantic Dedup Engine

```rust
// tasks/heartbeat/dedup.rs

pub struct DedupEngine {
    store: Arc<Mutex<TaskDatabase>>,
    embedding_provider: Arc<dyn EmbeddingProvider>,  // Reuse memory system
    default_config: DedupConfig,
}

impl DedupEngine {
    /// Check if output is semantically duplicate within window
    pub async fn is_duplicate(&self, task_id: &str, output: &str) -> bool;

    /// Record output embedding after successful delivery
    pub async fn record(&self, task_id: &str, output: &str);

    /// Cleanup expired records
    pub async fn cleanup(&self, retention_ms: i64);
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32;
```

Design points:
- **Embedding failure = pass through** — dedup is best-effort, never blocks delivery
- **Per-task isolation** — different tasks maintain independent history windows
- **SQLite BLOB storage** — embedding vectors serialized as `Vec<f32>` → `&[u8]`, no vector DB needed
- **Dual eviction** — time window (24h default) + count limit (10 per task), whichever is stricter

## Persistence

### Unified SQLite Database

Single file `~/.aleph/data/tasks.db` (migrated from `cron.db`). Cron and heartbeat use separate tables:

```sql
-- Existing tables unchanged
CREATE TABLE cron_jobs (...);
CREATE TABLE cron_job_runs (...);

-- New heartbeat tables
CREATE TABLE heartbeat_tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    data TEXT NOT NULL
);

CREATE TABLE heartbeat_runs (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    trigger_source TEXT NOT NULL,    -- Interval / Wake / Manual
    l1_status TEXT NOT NULL,         -- Triggered / Skipped / Error
    l2_status TEXT,                  -- Silent / Delivered / Skipped(Dedup) / Error / NULL
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    l1_duration_ms INTEGER,
    l2_duration_ms INTEGER,
    error TEXT,
    delivery_status TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_heartbeat_runs_task_id ON heartbeat_runs(task_id);
CREATE INDEX idx_heartbeat_runs_created_at ON heartbeat_runs(created_at DESC);

CREATE TABLE heartbeat_dedup (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    output_text TEXT NOT NULL,
    embedding BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_heartbeat_dedup_task_id ON heartbeat_dedup(task_id);
```

### Migration Strategy

```rust
fn migrate_cron_db_if_needed(old_path: &Path, new_path: &Path) {
    if old_path.exists() && !new_path.exists() {
        std::fs::rename(old_path, new_path).ok();  // cron.db → tasks.db
    }
    // CREATE TABLE IF NOT EXISTS for heartbeat tables
}
```

Zero data loss. Existing cron data preserved as-is.

## Timer Loop & Concurrency

### Dual Timer Design

| Dimension | Cron Timer | Heartbeat Timer |
|-----------|-----------|-----------------|
| Tick interval | 60s | 10s |
| Execution model | Three-phase (mark→execute→writeback) | Two-level (L1→L2), single-phase writeback |
| Concurrency | Worker Pool + VecDeque | Semaphore (lighter weight) |
| Wake mechanism | Pure timer | Timer + `tokio::select!` event wake |
| Lock granularity | Global Mutex on store | Semaphore per-task concurrency |

### Heartbeat Timer Loop

```rust
pub async fn run_heartbeat_loop(
    state: Arc<HeartbeatServiceState>,
    wake_queue: Arc<WakeQueue>,
    tool_registry: Arc<ToolRegistry>,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    delivery_engine: Arc<DeliveryEngine>,
    dedup: Arc<DedupEngine>,
) {
    loop {
        tokio::select! {
            _ = sleep(Duration::from_secs(state.config.tick_interval_secs)) => {},
            _ = wake_queue.notified() => {},
        }
        if state.is_shutdown() { break; }
        if state.is_running() { continue; }
        state.set_running(true);

        let wake_requests = wake_queue.drain();
        let due_tasks = collect_due_tasks(&state, &wake_requests).await;

        let semaphore = Arc::new(Semaphore::new(state.config.max_concurrent));
        let handles: Vec<_> = due_tasks.into_iter().map(|(task, wake_reason)| {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            tokio::spawn(async move {
                let result = execute_heartbeat_tick(&task, wake_reason, ...).await;
                drop(permit);
                (task.id.clone(), result)
            })
        }).collect();

        let results = join_all(handles).await;
        writeback_results(&state, results).await;
        state.set_running(false);
    }
}
```

## Gateway RPC

New `heartbeat.*` methods parallel to `cron.*`:

| Method | Description |
|--------|-------------|
| `heartbeat.list` | List all heartbeat tasks |
| `heartbeat.get` | Get single task |
| `heartbeat.create` | Create task |
| `heartbeat.update` | Update task |
| `heartbeat.delete` | Delete task |
| `heartbeat.toggle` | Enable/disable |
| `heartbeat.wake` | Manual wake (immediate trigger) |
| `heartbeat.runs` | Query execution history |

Cron RPC methods remain unchanged for backward compatibility.

## UI Redesign

### Navigation

Settings sidebar: "定时任务" → "任务". Internal segment tabs switch between "定时任务" and "心跳任务".

### Heartbeat Task List

Each item shows: status indicator (green=enabled, blue-pulse=running, gray=disabled), agent ID, interval, and today's L1/L2 counter showing probe filtering efficiency.

### Heartbeat Task Editor

Sections: basic info (name, agent, interval), probe config (tool selector from ToolRegistry, params JSON, trigger condition), delivery config (reuse cron delivery UI), dedup config (window, similarity threshold), execution history (L1→L2 flow per record).

### Execution History Table

Each record shows the full L1→L2 flow: timestamp, L1 result (triggered/skipped), L2 result (silent/delivered/deduped/error), output preview, duration. Users can see probe filtering efficiency at a glance.

### File Changes

| From | To |
|------|------|
| `views/cron.rs` | `views/tasks.rs` (two tab sub-views) |
| `api/cron.rs` | Kept, add `api/heartbeat.rs` |
| Settings sidebar "定时任务" | "任务" |

## Initialization & Startup

```rust
// start/mod.rs

// 1. Migrate DB: cron.db → tasks.db
let task_db_path = data_dir.join("tasks.db");
migrate_cron_db_if_needed(&old_cron_db_path, &task_db_path);

// 2. CronService (points to new DB, logic unchanged)
let cron_service = CronService::new(cron_config, task_db_path.clone()).await?;

// 3. HeartbeatService (new)
let heartbeat_service = HeartbeatService::new(
    heartbeat_config, task_db_path,
    tool_registry.clone(), execution_adapter.clone(),
    delivery_engine.clone(), embedding_provider.clone(),
).await?;

// 4. Two independent timer loops
tokio::spawn(cron_service.run_timer_loop());
tokio::spawn(heartbeat_service.run_heartbeat_loop());
```

## Configuration

```toml
[tasks.cron]
enabled = true
check_interval_secs = 60
# ... existing config unchanged

[tasks.heartbeat]
enabled = true
tick_interval_secs = 10
max_concurrent = 3
job_timeout_secs = 120
history_retention_days = 30

[tasks.heartbeat.dedup]
window_ms = 86400000
similarity_threshold = 0.85
max_history = 10
```

## Legacy Code Cleanup

| Operation | Description |
|-----------|-------------|
| `core/src/cron/` → `core/src/tasks/cron/` | Directory move |
| `cron/store.rs` | Split: generic → `tasks/shared/store.rs`, cron-specific stays |
| `cron/delivery.rs` → `tasks/shared/delivery.rs` | Move shared delivery engine |
| `cron/clock.rs` → `tasks/shared/clock.rs` | Move clock abstraction |
| `cron/schedule.rs` → `tasks/shared/schedule.rs` | Move scheduling functions |
| `cron/history.rs` | Split: generic cleanup → `tasks/shared/history.rs` |
| `gateway/handlers/cron.rs` | Keep, add `handlers/heartbeat.rs` |
| `views/cron.rs` → `views/tasks.rs` | Merge into tabbed view |
| `api/cron.rs` | Keep, add `api/heartbeat.rs` |
| `lib.rs`: `pub mod cron` → `pub mod tasks` | Module rename |
| All `use crate::cron::` → `use crate::tasks::cron::` | Batch replace |

Principle: move > copy > create. No stale paths left behind. `git mv` preserves history.

## Advantages Over OpenClaw

| Dimension | OpenClaw | Aleph |
|-----------|----------|-------|
| L1 probe | None. Every heartbeat = full Agent turn | Tool call filters empty polls, 90%+ cycles zero LLM cost |
| Dedup | Simple text comparison (24h exact match) | Semantic embedding dedup, catches paraphrased variants |
| Wake coalescing | String key + setTimeout delay | Type-safe WakeRequest + enum priority + `tokio::select!` |
| Concurrency | Single-threaded JS, serial | Rust async + Semaphore, true parallel multi-task |
| Timer precision | JavaScript setTimeout, ms-level jitter | tokio timer, microsecond precision |
| Persistence | Heartbeat state in-memory (lost on restart) | Unified SQLite WAL, heartbeat state persisted |
| Config granularity | Per-agent fixed binding | User-created entities, flexible agent x interval x probe x delivery |
| Observability | Simple event log | L1/L2 phased history with UI visualization |
| HEARTBEAT.md | No equivalent | Agent identity file, LLM self-determines heartbeat behavior |
| Event-driven | Single `requestHeartbeatNow()` entry | WakeQueue with multi-source, priority coalescing |
| Architecture | Heartbeat and Cron are separate systems, code duplication | Shared infrastructure (Store/Delivery/History/Clock), independent engines |
