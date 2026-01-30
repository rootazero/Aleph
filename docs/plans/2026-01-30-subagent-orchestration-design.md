# Sub-Agent Orchestration Enhancement Design

> **Date:** 2026-01-30
> **Status:** Approved
> **Scope:** SessionsSpawnTool + AuthProfile Manager + RunEventBus

---

## Overview

This design adds three core capabilities to complete Aether's **Sub-Agent Orchestration + Authentication Management + Lifecycle Control** loop:

1. **SessionsSpawnTool** - Independent tool for spawning child sessions with model/thinking overrides
2. **AuthProfile Manager** - Hybrid storage architecture for API key management
3. **RunEventBus** - Unified event broadcasting for run lifecycle and waiting

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Aether Gateway                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐      │
│  │ SessionsSpawn│    │ AuthProfile  │    │  RunEventBus │      │
│  │     Tool     │    │   Manager    │    │              │      │
│  └──────┬───────┘    └──────┬───────┘    └──────┬───────┘      │
│         │                   │                   │               │
│         ▼                   ▼                   ▼               │
│  ┌──────────────────────────────────────────────────────┐      │
│  │                  ExecutionEngine                      │      │
│  │  • Create child Session (model/thinking override)     │      │
│  │  • Get available Profile (cooldown check)             │      │
│  │  • Broadcast RunEvent (multi-subscriber)              │      │
│  └──────────────────────────────────────────────────────┘      │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## Part 1: SessionsSpawnTool

### Design Decision

**Choice: Independent Tool (Option B)**

Rationale:
- **Spawn ≠ Send** — Creation vs communication are different semantic actions
- **LLM Cognitive Clarity** — Independent tool = clear Schema, reduced hallucination
- **Lifecycle Isolation** — Spawn manages creation/config/destruction; Send only handles communication

### Tool Schema

```rust
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SessionsSpawnArgs {
    /// Task description for child agent (required)
    pub task: String,

    /// Display label for UI
    #[serde(default)]
    pub label: Option<String>,

    /// Target agent ID (defaults to caller's agent_id)
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Model override (e.g., "anthropic/claude-sonnet-4-20250514")
    #[serde(default)]
    pub model: Option<String>,

    /// Thinking level override (off/minimal/low/medium/high/xhigh)
    #[serde(default)]
    pub thinking: Option<String>,

    /// Run timeout in seconds, default 300
    #[serde(default = "default_timeout")]
    pub run_timeout_seconds: u32,

    /// Cleanup policy: ephemeral (destroy on completion) | persistent (keep)
    #[serde(default = "default_cleanup")]
    pub cleanup: CleanupPolicy,
}

#[derive(Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    #[default]
    Ephemeral,
    Persistent,
}
```

### Authorization Check

```rust
fn check_spawn_authorization(
    requester_agent_id: &str,
    target_agent_id: &str,
    config: &GatewayConfig,
) -> Result<(), SpawnError> {
    let agent_config = config.get_agent(requester_agent_id)?;
    let allow_list = &agent_config.subagents.allow_agents;

    // "*" allows all
    if allow_list.iter().any(|v| v.trim() == "*") {
        return Ok(());
    }

    // Check whitelist
    if allow_list.iter().any(|v| v.eq_ignore_ascii_case(target_agent_id)) {
        return Ok(());
    }

    Err(SpawnError::Forbidden {
        requester: requester_agent_id.to_string(),
        target: target_agent_id.to_string(),
    })
}
```

### Return Structure

```rust
#[derive(Serialize, Deserialize)]
pub struct SessionsSpawnResult {
    pub status: SpawnStatus,  // Accepted | Forbidden | Error
    pub child_session_key: String,
    pub run_id: String,
    pub model_applied: Option<bool>,
    pub warning: Option<String>,
}
```

### File Location

`core/src/builtin_tools/sessions/spawn_tool.rs`

---

## Part 2: AuthProfile Manager

### Design Decision

**Choice: Hybrid Mode (Option C) with Enhancement**

Key insight: **Rate Limit is per-Key, not per-Agent.** If Agent A exhausts Key #1, Agent B should not retry Key #1.

### Three-Layer Storage Architecture

```
~/.aether/
├── profiles.toml                    # Global config (user-maintained)
└── agents/
    └── {agent_id}/
        └── state.json               # Per-agent stats (auto-maintained)

In Memory:
└── Arc<RwLock<RuntimeStatusMap>>    # Global shared Cooldown state
```

| Layer | Location | Content | Maintained By |
|-------|----------|---------|---------------|
| **Static Config** | `~/.aether/profiles.toml` | API Keys, Provider URLs | User manually |
| **Runtime State** | Memory `Arc<RwLock>` | Cooldown, Rate Limit | Auto, not persisted |
| **Persistent Stats** | `~/.aether/agents/{id}/state.json` | Token usage, Budget | Auto, per-agent |

### Global Config Definition

**File:** `~/.aether/profiles.toml`

```toml
[profiles.anthropic_main]
provider = "anthropic"
api_key = "sk-ant-..."  # or "env:ANTHROPIC_API_KEY"
tier = "tier-4"

[profiles.openai_backup]
provider = "openai"
api_key = "env:OPENAI_API_KEY"
tier = "tier-3"

[profiles.ollama_local]
provider = "ollama"
base_url = "http://localhost:11434"
```

### Runtime State (Memory, Not Persisted)

```rust
use dashmap::DashMap;
use std::time::Instant;

/// Global shared Profile runtime state
pub struct RuntimeStatus {
    /// Is rate limited
    pub is_rate_limited: bool,
    /// Cooldown end time
    pub cooldown_until: Option<Instant>,
    /// Failure count in current window
    pub failure_count: u32,
    /// Last failure reason
    pub last_failure_reason: Option<FailureReason>,
}

/// Key: profile_id, Value: RuntimeStatus
pub type RuntimeStatusMap = DashMap<String, RuntimeStatus>;
```

### Per-Agent Persistent Stats

**File:** `~/.aether/agents/{agent_id}/state.json`

```rust
#[derive(Serialize, Deserialize, Default)]
pub struct AgentState {
    /// Usage stats per Profile
    pub usage: HashMap<String, ProfileUsage>,
    /// Config overrides for this Agent
    pub overrides: HashMap<String, ProfileOverride>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct ProfileUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost_usd: f64,
    pub request_count: u64,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize)]
pub struct ProfileOverride {
    /// Budget limit for this Agent
    pub max_budget_usd: Option<f64>,
    /// Disable this Profile for this Agent
    pub disabled: Option<bool>,
}
```

### Core Manager Structure

```rust
pub struct AuthProfileManager {
    /// Static config (read-only, from profiles.toml)
    configs: HashMap<String, ProfileConfig>,
    /// Runtime state (global shared, in memory)
    status: Arc<RuntimeStatusMap>,
    /// Config file path
    config_path: PathBuf,
}

impl AuthProfileManager {
    /// Get available Profile (considering Cooldown + Agent budget)
    pub fn get_available_profile(
        &self,
        provider: &str,
        agent_id: &str,
    ) -> Option<EffectiveProfile>;

    /// Mark Profile failure (trigger Cooldown)
    pub fn mark_failure(
        &self,
        profile_id: &str,
        reason: FailureReason,
    );

    /// Mark Profile success (reset failure count)
    pub fn mark_success(&self, profile_id: &str);

    /// Record usage (write to Agent state.json)
    pub async fn record_usage(
        &self,
        agent_id: &str,
        profile_id: &str,
        usage: &TokenUsage,
    ) -> Result<()>;
}
```

### File Location

`core/src/providers/profile_manager.rs`

---

## Part 3: RunEventBus

### Design Decision

**Choice: EventBus Integration (Option C)**

Rationale:
- **Unified "Stream" Philosophy** — Aether's core is Streaming; Run completion is naturally an event
- **Multi-Subscriber Support** — CLI, GUI, Logs can all subscribe to the same run's events
- **Solves Message Queueing** — `queueEmbeddedPiMessage` becomes just another event type

### Event Definition

```rust
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    // ===== State Changes =====
    StatusChanged {
        status: RunStatus,
        timestamp: DateTime<Utc>,
    },

    // ===== Streaming Output =====
    TokenDelta {
        delta: String,
        index: u32,
    },
    ReasoningDelta {
        delta: String,
        index: u32,
    },

    // ===== Tool Calls =====
    ToolStart {
        tool_name: String,
        tool_id: String,
        arguments: serde_json::Value,
    },
    ToolEnd {
        tool_id: String,
        success: bool,
        result_summary: Option<String>,
    },

    // ===== Lifecycle Termination =====
    RunCompleted {
        output: String,
        usage: TokenUsage,
        duration_ms: u64,
    },
    RunFailed {
        error: String,
        error_code: Option<String>,
    },
    RunCancelled {
        reason: Option<String>,
    },

    // ===== User Interaction =====
    InputRequested {
        prompt: String,
        request_id: String,
    },
    InputReceived {
        request_id: String,
        content: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}
```

### ActiveRun Structure

```rust
use tokio::sync::{broadcast, mpsc};

pub struct ActiveRun {
    pub run_id: String,
    pub session_key: String,
    pub started_at: DateTime<Utc>,

    /// Event broadcast channel (multi-subscriber)
    event_tx: broadcast::Sender<RunEvent>,

    /// User input receiving channel
    input_rx: mpsc::Receiver<String>,
    input_tx: mpsc::Sender<String>,

    /// Cancel signal
    cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ActiveRun {
    pub fn new(run_id: String, session_key: String) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let (input_tx, input_rx) = mpsc::channel(16);

        Self {
            run_id,
            session_key,
            started_at: Utc::now(),
            event_tx,
            input_rx,
            input_tx,
            cancel_tx: None,
        }
    }

    /// Get event subscriber
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.event_tx.subscribe()
    }

    /// Emit event
    pub fn emit(&self, event: RunEvent) {
        let _ = self.event_tx.send(event);
    }
}
```

### Wait Mechanism Implementation

```rust
/// Wait for Run to end, return final result
pub async fn wait_for_run_end(
    rx: &mut broadcast::Receiver<RunEvent>,
    timeout_ms: u64,
) -> Result<RunEndResult, WaitError> {
    let wait_future = async {
        loop {
            match rx.recv().await {
                Ok(event) => match event {
                    RunEvent::RunCompleted { output, usage, duration_ms } => {
                        return Ok(RunEndResult::Completed { output, usage, duration_ms });
                    }
                    RunEvent::RunFailed { error, error_code } => {
                        return Ok(RunEndResult::Failed { error, error_code });
                    }
                    RunEvent::RunCancelled { reason } => {
                        return Ok(RunEndResult::Cancelled { reason });
                    }
                    _ => continue, // Ignore intermediate events
                },
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(WaitError::ChannelClosed);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Lagged {} events", n);
                    continue;
                }
            }
        }
    };

    tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        wait_future,
    ).await.map_err(|_| WaitError::Timeout)?
}
```

### Message Queueing Implementation

```rust
impl ExecutionEngine {
    /// Send user input to running Run
    pub async fn queue_message(
        &self,
        run_id: &str,
        message: String,
    ) -> Result<(), QueueError> {
        let runs = self.active_runs.read().await;

        let run = runs.get(run_id)
            .ok_or(QueueError::RunNotFound)?;

        run.input_tx.send(message).await
            .map_err(|_| QueueError::RunClosed)?;

        // Also broadcast event so subscribers know there's input
        run.emit(RunEvent::InputReceived {
            request_id: uuid::Uuid::new_v4().to_string(),
            content: message.clone(),
        });

        Ok(())
    }
}
```

### File Location

`core/src/gateway/run_event_bus.rs`

---

## Part 4: ExecutionEngine Integration

### Integrated Structure

```rust
pub struct ExecutionEngine<P: AiProvider, R: ProviderRegistry> {
    /// Agent instance management
    agents: HashMap<String, AgentInstance>,

    /// Active Run registry
    active_runs: Arc<RwLock<HashMap<String, ActiveRun>>>,

    /// AuthProfile manager (NEW)
    profile_manager: Arc<AuthProfileManager>,

    /// Provider Registry
    provider_registry: Arc<R>,

    /// Config
    config: GatewayConfig,

    /// Event bus (reuse existing EventBus)
    event_bus: Arc<EventBus>,
}
```

### Spawn Flow Integration

```rust
impl<P: AiProvider, R: ProviderRegistry> ExecutionEngine<P, R> {
    /// Handle SessionsSpawnTool call
    pub async fn handle_spawn(
        &self,
        requester_session_key: &str,
        args: SessionsSpawnArgs,
    ) -> Result<SessionsSpawnResult, SpawnError> {
        // 1. Authorization check
        let requester_agent_id = parse_agent_id(requester_session_key)?;
        let target_agent_id = args.agent_id.as_deref()
            .unwrap_or(&requester_agent_id);

        check_spawn_authorization(
            &requester_agent_id,
            target_agent_id,
            &self.config,
        )?;

        // 2. Generate child Session Key
        let child_session_key = format!(
            "agent:{}:subagent:{}",
            target_agent_id,
            uuid::Uuid::new_v4()
        );

        // 3. Apply model/thinking overrides
        let mut child_config = self.config.get_agent(target_agent_id)?
            .clone();

        let model_applied = if let Some(model) = &args.model {
            match self.validate_model(model) {
                Ok(_) => {
                    child_config.model = model.clone();
                    Some(true)
                }
                Err(e) => {
                    tracing::warn!("Invalid model {}: {}", model, e);
                    Some(false)
                }
            }
        } else {
            None
        };

        if let Some(thinking) = &args.thinking {
            child_config.thinking = normalize_think_level(thinking);
        }

        // 4. Create Run and register
        let run_id = uuid::Uuid::new_v4().to_string();
        let active_run = ActiveRun::new(
            run_id.clone(),
            child_session_key.clone(),
        );

        // 5. Register to active runs table
        {
            let mut runs = self.active_runs.write().await;
            runs.insert(run_id.clone(), active_run);
        }

        // 6. Start async execution
        let cleanup = args.cleanup.clone();
        let task = args.task.clone();
        let engine = self.clone();

        tokio::spawn(async move {
            engine.execute_spawned_run(
                &run_id,
                &child_session_key,
                &task,
                child_config,
                cleanup,
            ).await
        });

        Ok(SessionsSpawnResult {
            status: SpawnStatus::Accepted,
            child_session_key,
            run_id,
            model_applied,
            warning: None,
        })
    }
}
```

---

## Part 5: Gateway RPC Interface

### New RPC Methods

| Method | Description | Handler File |
|--------|-------------|--------------|
| `sessions.spawn` | Create child Session | `sessions.rs` |
| `run.wait` | Wait for Run to end | `runs.rs` |
| `run.queue_message` | Send input to Run | `runs.rs` |
| `profiles.list` | List available Profiles | `profiles.rs` |
| `profiles.status` | Query Profile status | `profiles.rs` |

### sessions.spawn

```rust
#[derive(Deserialize)]
pub struct SessionsSpawnRequest {
    pub task: String,
    pub label: Option<String>,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub run_timeout_seconds: Option<u32>,
    pub cleanup: Option<String>,
}

#[derive(Serialize)]
pub struct SessionsSpawnResponse {
    pub status: String,
    pub child_session_key: String,
    pub run_id: String,
    pub model_applied: Option<bool>,
    pub warning: Option<String>,
}
```

### run.wait

```rust
#[derive(Deserialize)]
pub struct RunWaitRequest {
    pub run_id: String,
    #[serde(default = "default_wait_timeout")]
    pub timeout_ms: u64,  // default 30000
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunWaitResponse {
    Completed {
        output: String,
        usage: TokenUsage,
        duration_ms: u64,
    },
    Failed {
        error: String,
        error_code: Option<String>,
    },
    Cancelled {
        reason: Option<String>,
    },
    Timeout,
    NotFound,
}
```

### run.queue_message

```rust
#[derive(Deserialize)]
pub struct RunQueueMessageRequest {
    pub run_id: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct RunQueueMessageResponse {
    pub success: bool,
    pub error: Option<String>,
}
```

### Handler Registration

```rust
pub fn register_handlers(registry: &mut HandlerRegistry) {
    // Existing handlers...

    // New
    registry.register("sessions.spawn", handle_sessions_spawn);
    registry.register("run.wait", handle_run_wait);
    registry.register("run.queue_message", handle_run_queue_message);
    registry.register("profiles.list", handle_profiles_list);
    registry.register("profiles.status", handle_profiles_status);
}
```

---

## Part 6: Implementation Plan

### New Files

| File | Description | Est. Lines |
|------|-------------|------------|
| `core/src/builtin_tools/sessions/spawn_tool.rs` | SessionsSpawnTool implementation | ~250 |
| `core/src/providers/profile_manager.rs` | AuthProfile three-layer manager | ~400 |
| `core/src/gateway/run_event_bus.rs` | RunEvent definition + ActiveRun | ~300 |
| `core/src/gateway/handlers/runs.rs` | run.wait / run.queue_message | ~150 |
| `core/src/gateway/handlers/profiles.rs` | profiles.list / profiles.status | ~100 |

### Modified Files

| File | Changes |
|------|---------|
| `core/src/gateway/execution_engine.rs` | Integrate ProfileManager + EventBus |
| `core/src/gateway/handlers/mod.rs` | Register new handlers |
| `core/src/builtin_tools/sessions/mod.rs` | Export SpawnTool |
| `core/src/providers/mod.rs` | Export ProfileManager |
| `core/src/config/types/agent/mod.rs` | Add `subagents.allow_agents` field |

### Implementation Phases

```
Phase 1: Infrastructure
├── 1.1 RunEventBus + ActiveRun structure
├── 1.2 ProfileManager three-layer architecture
└── 1.3 profiles.toml parsing

Phase 2: Core Functionality
├── 2.1 ExecutionEngine integrate EventBus
├── 2.2 wait_for_run_end() implementation
├── 2.3 queue_message() implementation
└── 2.4 ProfileManager integrate into Run execution flow

Phase 3: SessionsSpawn
├── 3.1 SessionsSpawnTool definition
├── 3.2 Authorization check (allow_agents)
├── 3.3 model/thinking override logic
└── 3.4 cleanup policy implementation

Phase 4: RPC Interface
├── 4.1 sessions.spawn handler
├── 4.2 run.wait / run.queue_message handlers
└── 4.3 profiles.list / profiles.status handlers

Phase 5: Testing & Documentation
├── 5.1 Unit tests
├── 5.2 Integration tests
└── 5.3 Update CLAUDE.md
```

### Config Schema Extension

```toml
# ~/.aether/config.toml new fields

[agents.defaults.subagents]
allow_agents = ["*"]  # or ["translator", "coder"]
default_cleanup = "ephemeral"
default_timeout_seconds = 300
```

---

## Summary

This design completes Aether's sub-agent orchestration capabilities:

| Component | Decision | Key Benefit |
|-----------|----------|-------------|
| **SessionsSpawnTool** | Independent tool | SRP, LLM clarity |
| **AuthProfile Manager** | Hybrid storage | Config global, state shared, stats isolated |
| **RunEventBus** | EventBus integration | Unified streaming, multi-subscriber |

The architecture is cleaner and more modern than Moltbot (Rust + WebSocket + EventBus).
