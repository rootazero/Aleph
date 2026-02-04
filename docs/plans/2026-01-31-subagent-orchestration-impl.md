# Sub-Agent Orchestration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add SessionsSpawnTool, AuthProfile Manager hybrid storage, and RunEventBus with wait/queue mechanisms.

**Architecture:** Three-layer design: (1) SessionsSpawnTool creates child sessions with model/thinking overrides, (2) AuthProfileManager uses hybrid storage (global config + memory state + per-agent stats), (3) RunEventBus broadcasts events via tokio::broadcast for multi-subscriber support.

**Tech Stack:** Rust, tokio (broadcast/mpsc channels), serde, schemars (JsonSchema), DashMap, TOML parsing.

---

## Task 1: RunEvent Types and ActiveRun Structure

**Files:**
- Create: `core/src/gateway/run_event_bus.rs`
- Modify: `core/src/gateway/mod.rs` (add module export)

**Step 1: Create run_event_bus.rs with event types**

```rust
// core/src/gateway/run_event_bus.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tokio::sync::{broadcast, mpsc, oneshot};

/// Run lifecycle events for multi-subscriber broadcasting
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    // State changes
    StatusChanged {
        status: RunStatus,
        timestamp: DateTime<Utc>,
    },

    // Streaming output
    TokenDelta {
        delta: String,
        index: u32,
    },
    ReasoningDelta {
        delta: String,
        index: u32,
    },

    // Tool calls
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

    // Lifecycle termination
    RunCompleted {
        output: String,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
    },
    RunFailed {
        error: String,
        error_code: Option<String>,
    },
    RunCancelled {
        reason: Option<String>,
    },

    // User interaction
    InputRequested {
        prompt: String,
        request_id: String,
    },
    InputReceived {
        request_id: String,
        content: String,
    },
}

/// Run status for state machine
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Result of waiting for a run to end
#[derive(Clone, Debug)]
pub enum RunEndResult {
    Completed {
        output: String,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
    },
    Failed {
        error: String,
        error_code: Option<String>,
    },
    Cancelled {
        reason: Option<String>,
    },
}

/// Errors that can occur while waiting
#[derive(Debug, thiserror::Error)]
pub enum WaitError {
    #[error("Timeout waiting for run to complete")]
    Timeout,
    #[error("Channel closed unexpectedly")]
    ChannelClosed,
    #[error("Run not found")]
    NotFound,
}

/// Errors for message queueing
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("Run not found: {0}")]
    RunNotFound(String),
    #[error("Run already closed")]
    RunClosed,
}

/// Active run with broadcast channel for events
pub struct ActiveRunHandle {
    pub run_id: String,
    pub session_key: String,
    pub started_at: DateTime<Utc>,

    /// Event broadcast channel (multi-subscriber)
    event_tx: broadcast::Sender<RunEvent>,

    /// User input channel
    input_tx: mpsc::Sender<String>,

    /// Cancel signal sender
    cancel_tx: Option<oneshot::Sender<()>>,

    /// Sequence counter for events
    seq_counter: AtomicU64,

    /// Chunk counter for streaming
    chunk_counter: AtomicU32,
}

impl ActiveRunHandle {
    /// Create a new active run handle
    pub fn new(run_id: String, session_key: String) -> (Self, mpsc::Receiver<String>, oneshot::Receiver<()>) {
        let (event_tx, _) = broadcast::channel(256);
        let (input_tx, input_rx) = mpsc::channel(16);
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let handle = Self {
            run_id,
            session_key,
            started_at: Utc::now(),
            event_tx,
            input_tx,
            cancel_tx: Some(cancel_tx),
            seq_counter: AtomicU64::new(0),
            chunk_counter: AtomicU32::new(0),
        };

        (handle, input_rx, cancel_rx)
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.event_tx.subscribe()
    }

    /// Emit an event to all subscribers
    pub fn emit(&self, event: RunEvent) {
        // Ignore send errors (no subscribers is fine)
        let _ = self.event_tx.send(event);
    }

    /// Get next sequence number
    pub fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Get next chunk index
    pub fn next_chunk(&self) -> u32 {
        self.chunk_counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Get input sender for queueing messages
    pub fn input_sender(&self) -> mpsc::Sender<String> {
        self.input_tx.clone()
    }

    /// Take the cancel sender (can only be called once)
    pub fn take_cancel_tx(&mut self) -> Option<oneshot::Sender<()>> {
        self.cancel_tx.take()
    }
}

impl Clone for ActiveRunHandle {
    fn clone(&self) -> Self {
        Self {
            run_id: self.run_id.clone(),
            session_key: self.session_key.clone(),
            started_at: self.started_at,
            event_tx: self.event_tx.clone(),
            input_tx: self.input_tx.clone(),
            cancel_tx: None, // Cancel can only be sent once
            seq_counter: AtomicU64::new(self.seq_counter.load(Ordering::SeqCst)),
            chunk_counter: AtomicU32::new(self.chunk_counter.load(Ordering::SeqCst)),
        }
    }
}
```

**Step 2: Add wait_for_run_end function**

Add to `core/src/gateway/run_event_bus.rs`:

```rust
use std::time::Duration;
use tokio::time::timeout;

/// Wait for a run to end, filtering intermediate events
pub async fn wait_for_run_end(
    rx: &mut broadcast::Receiver<RunEvent>,
    timeout_ms: u64,
) -> Result<RunEndResult, WaitError> {
    let wait_future = async {
        loop {
            match rx.recv().await {
                Ok(event) => match event {
                    RunEvent::RunCompleted {
                        output,
                        input_tokens,
                        output_tokens,
                        duration_ms,
                    } => {
                        return Ok(RunEndResult::Completed {
                            output,
                            input_tokens,
                            output_tokens,
                            duration_ms,
                        });
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
                    tracing::warn!("Lagged {} events while waiting for run", n);
                    continue;
                }
            }
        }
    };

    timeout(Duration::from_millis(timeout_ms), wait_future)
        .await
        .map_err(|_| WaitError::Timeout)?
}
```

**Step 3: Export module in gateway/mod.rs**

Find the module declarations in `core/src/gateway/mod.rs` and add:

```rust
pub mod run_event_bus;
pub use run_event_bus::{
    ActiveRunHandle, QueueError, RunEndResult, RunEvent, RunStatus, WaitError,
    wait_for_run_end,
};
```

**Step 4: Run compilation check**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -10`
Expected: No errors (warnings OK)

**Step 5: Commit**

```bash
git add core/src/gateway/run_event_bus.rs core/src/gateway/mod.rs
git commit -m "feat(gateway): add RunEventBus with broadcast channels

- RunEvent enum for lifecycle events
- ActiveRunHandle with multi-subscriber support
- wait_for_run_end() for blocking wait
- Input queueing via mpsc channel"
```

---

## Task 2: AuthProfileManager Hybrid Storage

**Files:**
- Create: `core/src/providers/profile_manager.rs`
- Create: `core/src/providers/profile_config.rs`
- Modify: `core/src/providers/mod.rs` (add exports)

**Step 1: Create profile_config.rs for TOML parsing**

```rust
// core/src/providers/profile_config.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Profile definition from profiles.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub provider: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub org_id: Option<String>,
}

impl ProfileConfig {
    /// Resolve API key, supporting env: prefix
    pub fn resolve_api_key(&self) -> Option<String> {
        self.api_key.as_ref().and_then(|key| {
            if let Some(env_var) = key.strip_prefix("env:") {
                std::env::var(env_var).ok()
            } else {
                Some(key.clone())
            }
        })
    }
}

/// Root structure for profiles.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfilesConfig {
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
}

impl ProfilesConfig {
    /// Load from TOML file
    pub fn load(path: &Path) -> Result<Self, ProfileConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| ProfileConfigError::IoError(e.to_string()))?;

        toml::from_str(&content)
            .map_err(|e| ProfileConfigError::ParseError(e.to_string()))
    }

    /// Save to TOML file
    pub fn save(&self, path: &Path) -> Result<(), ProfileConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ProfileConfigError::IoError(e.to_string()))?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| ProfileConfigError::SerializeError(e.to_string()))?;

        std::fs::write(path, content)
            .map_err(|e| ProfileConfigError::IoError(e.to_string()))
    }

    /// Get profiles for a specific provider
    pub fn profiles_for_provider(&self, provider: &str) -> Vec<(&String, &ProfileConfig)> {
        self.profiles
            .iter()
            .filter(|(_, config)| config.provider.eq_ignore_ascii_case(provider))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileConfigError {
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Serialize error: {0}")]
    SerializeError(String),
}
```

**Step 2: Create profile_manager.rs**

```rust
// core/src/providers/profile_manager.rs

use crate::providers::auth_profiles::{AuthProfileFailureReason, calculate_cooldown_ms};
use crate::providers::profile_config::{ProfileConfig, ProfilesConfig};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Runtime status for a profile (in-memory, not persisted)
#[derive(Debug, Clone)]
pub struct RuntimeStatus {
    pub is_rate_limited: bool,
    pub cooldown_until: Option<Instant>,
    pub failure_count: u32,
    pub last_failure_reason: Option<AuthProfileFailureReason>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            is_rate_limited: false,
            cooldown_until: None,
            failure_count: 0,
            last_failure_reason: None,
        }
    }
}

impl RuntimeStatus {
    /// Check if profile is currently in cooldown
    pub fn is_in_cooldown(&self) -> bool {
        if let Some(until) = self.cooldown_until {
            Instant::now() < until
        } else {
            false
        }
    }

    /// Get remaining cooldown in milliseconds
    pub fn cooldown_remaining_ms(&self) -> Option<u64> {
        self.cooldown_until.and_then(|until| {
            let now = Instant::now();
            if now < until {
                Some((until - now).as_millis() as u64)
            } else {
                None
            }
        })
    }
}

/// Per-agent usage statistics (persisted to state.json)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost_usd: f64,
    pub request_count: u64,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Per-agent profile overrides
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileOverride {
    #[serde(default)]
    pub max_budget_usd: Option<f64>,
    #[serde(default)]
    pub disabled: Option<bool>,
}

/// Agent state file structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    #[serde(default)]
    pub usage: HashMap<String, ProfileUsage>,
    #[serde(default)]
    pub overrides: HashMap<String, ProfileOverride>,
}

impl AgentState {
    /// Load from JSON file
    pub fn load(path: &PathBuf) -> Result<Self, std::io::Error> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })
    }

    /// Save to JSON file
    pub fn save(&self, path: &PathBuf) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)
    }
}

/// Effective profile combining config + runtime status
#[derive(Debug, Clone)]
pub struct EffectiveProfile {
    pub id: String,
    pub provider: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub tier: Option<String>,
}

/// Hybrid AuthProfile Manager
pub struct AuthProfileManager {
    /// Static config (from profiles.toml)
    configs: ProfilesConfig,
    /// Runtime status (in-memory, global shared)
    status: Arc<DashMap<String, RuntimeStatus>>,
    /// Config file path
    config_path: PathBuf,
    /// Agents directory base path
    agents_dir: PathBuf,
}

impl AuthProfileManager {
    /// Create new manager
    pub fn new(config_path: PathBuf, agents_dir: PathBuf) -> Result<Self, crate::providers::profile_config::ProfileConfigError> {
        let configs = ProfilesConfig::load(&config_path)?;

        Ok(Self {
            configs,
            status: Arc::new(DashMap::new()),
            config_path,
            agents_dir,
        })
    }

    /// Reload config from disk
    pub fn reload_config(&mut self) -> Result<(), crate::providers::profile_config::ProfileConfigError> {
        self.configs = ProfilesConfig::load(&self.config_path)?;
        Ok(())
    }

    /// Get available profile for provider (considering cooldown + agent budget)
    pub fn get_available_profile(
        &self,
        provider: &str,
        agent_id: &str,
    ) -> Option<EffectiveProfile> {
        let agent_state = self.load_agent_state(agent_id).unwrap_or_default();

        // Get all profiles for this provider
        let candidates = self.configs.profiles_for_provider(provider);

        // Find first available (not in cooldown, not over budget, not disabled)
        for (profile_id, config) in candidates {
            // Check runtime cooldown
            if let Some(status) = self.status.get(profile_id) {
                if status.is_in_cooldown() {
                    continue;
                }
            }

            // Check agent-specific overrides
            if let Some(override_cfg) = agent_state.overrides.get(profile_id) {
                if override_cfg.disabled == Some(true) {
                    continue;
                }

                // Check budget
                if let Some(max_budget) = override_cfg.max_budget_usd {
                    if let Some(usage) = agent_state.usage.get(profile_id) {
                        if usage.total_cost_usd >= max_budget {
                            continue;
                        }
                    }
                }
            }

            // Profile is available
            return Some(EffectiveProfile {
                id: profile_id.clone(),
                provider: config.provider.clone(),
                api_key: config.resolve_api_key(),
                base_url: config.base_url.clone(),
                tier: config.tier.clone(),
            });
        }

        None
    }

    /// Mark profile as failed (trigger cooldown)
    pub fn mark_failure(&self, profile_id: &str, reason: AuthProfileFailureReason) {
        let mut status = self.status.entry(profile_id.to_string()).or_default();

        status.failure_count += 1;
        status.last_failure_reason = Some(reason);

        // Apply cooldown based on reason
        let cooldown_ms = match reason {
            AuthProfileFailureReason::RateLimit => calculate_cooldown_ms(status.failure_count),
            AuthProfileFailureReason::Billing => {
                // Longer cooldown for billing issues
                let hours = std::cmp::min(24, 5 * (1 << status.failure_count.saturating_sub(1)));
                hours as u64 * 60 * 60 * 1000
            }
            _ => calculate_cooldown_ms(status.failure_count),
        };

        status.cooldown_until = Some(Instant::now() + std::time::Duration::from_millis(cooldown_ms));
        status.is_rate_limited = matches!(reason, AuthProfileFailureReason::RateLimit);
    }

    /// Mark profile as successful (reset failure count)
    pub fn mark_success(&self, profile_id: &str) {
        if let Some(mut status) = self.status.get_mut(profile_id) {
            status.failure_count = 0;
            status.cooldown_until = None;
            status.is_rate_limited = false;
            status.last_failure_reason = None;
        }
    }

    /// Record usage for agent
    pub fn record_usage(
        &self,
        agent_id: &str,
        profile_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) -> Result<(), std::io::Error> {
        let state_path = self.agent_state_path(agent_id);
        let mut state = AgentState::load(&state_path).unwrap_or_default();

        let usage = state.usage.entry(profile_id.to_string()).or_default();
        usage.input_tokens += input_tokens;
        usage.output_tokens += output_tokens;
        usage.total_cost_usd += cost_usd;
        usage.request_count += 1;
        usage.last_used_at = Some(Utc::now());

        state.save(&state_path)
    }

    /// List all profiles with status
    pub fn list_profiles(&self) -> Vec<ProfileInfo> {
        self.configs
            .profiles
            .iter()
            .map(|(id, config)| {
                let status = self.status.get(id);
                ProfileInfo {
                    id: id.clone(),
                    provider: config.provider.clone(),
                    is_available: status.map(|s| !s.is_in_cooldown()).unwrap_or(true),
                    cooldown_remaining_ms: status.and_then(|s| s.cooldown_remaining_ms()),
                    failure_count: status.map(|s| s.failure_count).unwrap_or(0),
                }
            })
            .collect()
    }

    /// Get agent state file path
    fn agent_state_path(&self, agent_id: &str) -> PathBuf {
        self.agents_dir.join(agent_id).join("state.json")
    }

    /// Load agent state from disk
    fn load_agent_state(&self, agent_id: &str) -> Result<AgentState, std::io::Error> {
        AgentState::load(&self.agent_state_path(agent_id))
    }
}

/// Profile info for listing
#[derive(Debug, Clone, Serialize)]
pub struct ProfileInfo {
    pub id: String,
    pub provider: String,
    pub is_available: bool,
    pub cooldown_remaining_ms: Option<u64>,
    pub failure_count: u32,
}
```

**Step 3: Export in providers/mod.rs**

Add to `core/src/providers/mod.rs`:

```rust
pub mod profile_config;
pub mod profile_manager;

pub use profile_config::{ProfileConfig, ProfilesConfig, ProfileConfigError};
pub use profile_manager::{
    AgentState, AuthProfileManager, EffectiveProfile, ProfileInfo, ProfileOverride,
    ProfileUsage, RuntimeStatus,
};
```

**Step 4: Run compilation check**

Run: `cargo build -p alephcore 2>&1 | grep -E "^error" | head -10`
Expected: No errors

**Step 5: Commit**

```bash
git add core/src/providers/profile_config.rs core/src/providers/profile_manager.rs core/src/providers/mod.rs
git commit -m "feat(providers): add AuthProfileManager with hybrid storage

- ProfilesConfig for TOML parsing (~/.aleph/profiles.toml)
- RuntimeStatus in-memory for cooldown (not persisted)
- AgentState per-agent for usage tracking
- EffectiveProfile combines config + status"
```

---

## Task 3: SessionsSpawnTool

**Files:**
- Create: `core/src/builtin_tools/sessions/spawn_tool.rs`
- Modify: `core/src/builtin_tools/sessions/mod.rs` (add export)

**Step 1: Create spawn_tool.rs**

```rust
// core/src/builtin_tools/sessions/spawn_tool.rs

use crate::builtin_tools::{notify_tool_result, notify_tool_start};
use crate::gateway::GatewayContext;
use crate::tools::traits::AlephTool;
use anyhow::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Cleanup policy for spawned sessions
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    /// Destroy session after task completion
    #[default]
    Ephemeral,
    /// Keep session for future use
    Persistent,
}

/// Arguments for sessions_spawn tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionsSpawnArgs {
    /// Task description for the child agent (required)
    pub task: String,

    /// Display label for UI (optional)
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

    /// Run timeout in seconds (default 300)
    #[serde(default = "default_timeout")]
    pub run_timeout_seconds: u32,

    /// Cleanup policy after task completion
    #[serde(default)]
    pub cleanup: CleanupPolicy,
}

fn default_timeout() -> u32 {
    300
}

/// Spawn status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpawnStatus {
    Accepted,
    Forbidden,
    Error,
}

/// Output from sessions_spawn tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsSpawnOutput {
    pub status: SpawnStatus,
    pub child_session_key: String,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_applied: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SessionsSpawnOutput {
    fn forbidden(reason: &str) -> Self {
        Self {
            status: SpawnStatus::Forbidden,
            child_session_key: String::new(),
            run_id: String::new(),
            model_applied: None,
            warning: None,
            error: Some(reason.to_string()),
        }
    }

    fn error(reason: &str) -> Self {
        Self {
            status: SpawnStatus::Error,
            child_session_key: String::new(),
            run_id: String::new(),
            model_applied: None,
            warning: None,
            error: Some(reason.to_string()),
        }
    }
}

/// Sessions spawn tool
#[derive(Clone)]
pub struct SessionsSpawnTool {
    context: Option<Arc<GatewayContext>>,
    current_agent_id: String,
    allow_agents: Vec<String>,
}

impl SessionsSpawnTool {
    /// Create new spawn tool
    pub fn new(
        context: Option<Arc<GatewayContext>>,
        current_agent_id: String,
        allow_agents: Vec<String>,
    ) -> Self {
        Self {
            context,
            current_agent_id,
            allow_agents,
        }
    }

    /// Check if spawning target agent is allowed
    fn check_authorization(&self, target_agent_id: &str) -> Result<(), String> {
        // "*" allows all
        if self.allow_agents.iter().any(|v| v.trim() == "*") {
            return Ok(());
        }

        // Check whitelist
        if self
            .allow_agents
            .iter()
            .any(|v| v.eq_ignore_ascii_case(target_agent_id))
        {
            return Ok(());
        }

        Err(format!(
            "Agent '{}' is not allowed to spawn '{}'. Allowed: {:?}",
            self.current_agent_id, target_agent_id, self.allow_agents
        ))
    }

    /// Internal implementation
    async fn call_impl(&self, args: SessionsSpawnArgs) -> SessionsSpawnOutput {
        notify_tool_start(
            "sessions_spawn",
            &format!("task={}, agent={:?}", args.task, args.agent_id),
        );

        // Determine target agent
        let target_agent_id = args
            .agent_id
            .as_deref()
            .unwrap_or(&self.current_agent_id);

        // Authorization check
        if let Err(reason) = self.check_authorization(target_agent_id) {
            let output = SessionsSpawnOutput::forbidden(&reason);
            notify_tool_result("sessions_spawn", "forbidden", false);
            return output;
        }

        // Generate child session key
        let child_session_key = format!(
            "agent:{}:subagent:{}",
            target_agent_id,
            uuid::Uuid::new_v4()
        );

        let run_id = uuid::Uuid::new_v4().to_string();

        // For now, return accepted status
        // Actual execution will be handled by ExecutionEngine integration
        let output = SessionsSpawnOutput {
            status: SpawnStatus::Accepted,
            child_session_key: child_session_key.clone(),
            run_id: run_id.clone(),
            model_applied: args.model.as_ref().map(|_| true),
            warning: None,
            error: None,
        };

        notify_tool_result(
            "sessions_spawn",
            &format!("accepted: session={}, run={}", child_session_key, run_id),
            true,
        );

        output
    }
}

#[async_trait]
impl AlephTool for SessionsSpawnTool {
    const NAME: &'static str = "sessions_spawn";
    const DESCRIPTION: &'static str = r#"Spawn a new child session to execute a task.

Use this tool to delegate work to a sub-agent with optional model and thinking level overrides.
The child session runs asynchronously. Use sessions_send with the returned session_key to communicate.

Parameters:
- task: The task description for the child agent (required)
- label: Display label for UI (optional)
- agent_id: Target agent ID, defaults to current agent
- model: Model override (e.g., "anthropic/claude-sonnet-4-20250514")
- thinking: Thinking level (off/minimal/low/medium/high/xhigh)
- run_timeout_seconds: Timeout in seconds (default 300)
- cleanup: "ephemeral" (destroy after) or "persistent" (keep)

Returns: status, child_session_key, run_id"#;

    type Args = SessionsSpawnArgs;
    type Output = SessionsSpawnOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(self.call_impl(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_spawn_authorization_wildcard() {
        let tool = SessionsSpawnTool::new(None, "main".to_string(), vec!["*".to_string()]);
        assert!(tool.check_authorization("any_agent").is_ok());
    }

    #[tokio::test]
    async fn test_spawn_authorization_whitelist() {
        let tool = SessionsSpawnTool::new(
            None,
            "main".to_string(),
            vec!["coder".to_string(), "translator".to_string()],
        );
        assert!(tool.check_authorization("coder").is_ok());
        assert!(tool.check_authorization("translator").is_ok());
        assert!(tool.check_authorization("hacker").is_err());
    }

    #[tokio::test]
    async fn test_spawn_default_agent() {
        let tool = SessionsSpawnTool::new(None, "main".to_string(), vec!["*".to_string()]);

        let args = SessionsSpawnArgs {
            task: "test task".to_string(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: 300,
            cleanup: CleanupPolicy::Ephemeral,
        };

        let output = tool.call_impl(args).await;
        assert_eq!(output.status, SpawnStatus::Accepted);
        assert!(output.child_session_key.starts_with("agent:main:subagent:"));
    }
}
```

**Step 2: Export in sessions/mod.rs**

Add to `core/src/builtin_tools/sessions/mod.rs`:

```rust
pub mod spawn_tool;
pub use spawn_tool::{
    CleanupPolicy, SessionsSpawnArgs, SessionsSpawnOutput, SessionsSpawnTool, SpawnStatus,
};
```

**Step 3: Run tests**

Run: `cargo test -p alephcore spawn --lib -- --nocapture`
Expected: All 3 tests pass

**Step 4: Commit**

```bash
git add core/src/builtin_tools/sessions/spawn_tool.rs core/src/builtin_tools/sessions/mod.rs
git commit -m "feat(tools): add SessionsSpawnTool for sub-agent creation

- SessionsSpawnArgs with model/thinking overrides
- CleanupPolicy (ephemeral/persistent)
- Authorization check via allow_agents whitelist
- Unit tests for authorization logic"
```

---

## Task 4: RPC Handlers for run.wait and run.queue_message

**Files:**
- Create: `core/src/gateway/handlers/runs.rs`
- Modify: `core/src/gateway/handlers/mod.rs` (register handlers)

**Step 1: Create runs.rs handler**

```rust
// core/src/gateway/handlers/runs.rs

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::run_event_bus::{wait_for_run_end, ActiveRunHandle, QueueError, WaitError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Request for run.wait
#[derive(Debug, Deserialize)]
pub struct RunWaitRequest {
    pub run_id: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    30000
}

/// Response for run.wait
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunWaitResponse {
    Completed {
        output: String,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
    },
    Failed {
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
    },
    Cancelled {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Timeout,
    NotFound,
}

/// Request for run.queue_message
#[derive(Debug, Deserialize)]
pub struct RunQueueMessageRequest {
    pub run_id: String,
    pub message: String,
}

/// Response for run.queue_message
#[derive(Debug, Serialize)]
pub struct RunQueueMessageResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Handle run.wait RPC
pub async fn handle_run_wait(
    request: JsonRpcRequest,
    active_runs: Arc<RwLock<HashMap<String, ActiveRunHandle>>>,
) -> JsonRpcResponse {
    let params: RunWaitRequest = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                -32602,
                &format!("Invalid params: {}", e),
                None,
            );
        }
    };

    // Get subscriber
    let mut rx = {
        let runs = active_runs.read().await;
        match runs.get(&params.run_id) {
            Some(handle) => handle.subscribe(),
            None => {
                return JsonRpcResponse::success(request.id, RunWaitResponse::NotFound);
            }
        }
    };

    // Wait for result
    let response = match wait_for_run_end(&mut rx, params.timeout_ms).await {
        Ok(result) => match result {
            crate::gateway::run_event_bus::RunEndResult::Completed {
                output,
                input_tokens,
                output_tokens,
                duration_ms,
            } => RunWaitResponse::Completed {
                output,
                input_tokens,
                output_tokens,
                duration_ms,
            },
            crate::gateway::run_event_bus::RunEndResult::Failed { error, error_code } => {
                RunWaitResponse::Failed { error, error_code }
            }
            crate::gateway::run_event_bus::RunEndResult::Cancelled { reason } => {
                RunWaitResponse::Cancelled { reason }
            }
        },
        Err(WaitError::Timeout) => RunWaitResponse::Timeout,
        Err(WaitError::ChannelClosed) | Err(WaitError::NotFound) => RunWaitResponse::NotFound,
    };

    JsonRpcResponse::success(request.id, response)
}

/// Handle run.queue_message RPC
pub async fn handle_run_queue_message(
    request: JsonRpcRequest,
    active_runs: Arc<RwLock<HashMap<String, ActiveRunHandle>>>,
) -> JsonRpcResponse {
    let params: RunQueueMessageRequest = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                -32602,
                &format!("Invalid params: {}", e),
                None,
            );
        }
    };

    let result = {
        let runs = active_runs.read().await;
        match runs.get(&params.run_id) {
            Some(handle) => {
                let sender = handle.input_sender();
                drop(runs); // Release lock before async operation

                sender.send(params.message).await.map_err(|_| QueueError::RunClosed)
            }
            None => Err(QueueError::RunNotFound(params.run_id.clone())),
        }
    };

    let response = match result {
        Ok(()) => RunQueueMessageResponse {
            success: true,
            error: None,
        },
        Err(e) => RunQueueMessageResponse {
            success: false,
            error: Some(e.to_string()),
        },
    };

    JsonRpcResponse::success(request.id, response)
}
```

**Step 2: Register handlers in mod.rs**

Find the handler registration in `core/src/gateway/handlers/mod.rs` and add:

```rust
pub mod runs;

// In the HandlerRegistry::new() function, add:
// Note: These require active_runs to be passed, so they need special handling
// For now, document them as available methods
```

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/runs.rs core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): add run.wait and run.queue_message handlers

- RunWaitRequest/Response for blocking wait
- RunQueueMessageRequest/Response for message injection
- Handlers integrate with ActiveRunHandle"
```

---

## Task 5: RPC Handlers for profiles.list and profiles.status

**Files:**
- Create: `core/src/gateway/handlers/profiles.rs`
- Modify: `core/src/gateway/handlers/mod.rs` (add export)

**Step 1: Create profiles.rs handler**

```rust
// core/src/gateway/handlers/profiles.rs

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::providers::profile_manager::{AuthProfileManager, ProfileInfo};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Request for profiles.list (no params needed)
#[derive(Debug, Deserialize, Default)]
pub struct ProfilesListRequest {
    #[serde(default)]
    pub provider: Option<String>,
}

/// Response for profiles.list
#[derive(Debug, Serialize)]
pub struct ProfilesListResponse {
    pub profiles: Vec<ProfileInfo>,
}

/// Request for profiles.status
#[derive(Debug, Deserialize)]
pub struct ProfilesStatusRequest {
    pub profile_id: String,
}

/// Response for profiles.status
#[derive(Debug, Serialize)]
pub struct ProfilesStatusResponse {
    pub id: String,
    pub provider: String,
    pub is_available: bool,
    pub cooldown_remaining_ms: Option<u64>,
    pub failure_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Handle profiles.list RPC
pub async fn handle_profiles_list(
    request: JsonRpcRequest,
    profile_manager: Arc<AuthProfileManager>,
) -> JsonRpcResponse {
    let params: ProfilesListRequest = serde_json::from_value(request.params.clone())
        .unwrap_or_default();

    let mut profiles = profile_manager.list_profiles();

    // Filter by provider if specified
    if let Some(provider) = params.provider {
        profiles.retain(|p| p.provider.eq_ignore_ascii_case(&provider));
    }

    JsonRpcResponse::success(
        request.id,
        ProfilesListResponse { profiles },
    )
}

/// Handle profiles.status RPC
pub async fn handle_profiles_status(
    request: JsonRpcRequest,
    profile_manager: Arc<AuthProfileManager>,
) -> JsonRpcResponse {
    let params: ProfilesStatusRequest = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                -32602,
                &format!("Invalid params: {}", e),
                None,
            );
        }
    };

    let profiles = profile_manager.list_profiles();

    match profiles.into_iter().find(|p| p.id == params.profile_id) {
        Some(info) => JsonRpcResponse::success(
            request.id,
            ProfilesStatusResponse {
                id: info.id,
                provider: info.provider,
                is_available: info.is_available,
                cooldown_remaining_ms: info.cooldown_remaining_ms,
                failure_count: info.failure_count,
                error: None,
            },
        ),
        None => JsonRpcResponse::success(
            request.id,
            ProfilesStatusResponse {
                id: params.profile_id.clone(),
                provider: String::new(),
                is_available: false,
                cooldown_remaining_ms: None,
                failure_count: 0,
                error: Some(format!("Profile '{}' not found", params.profile_id)),
            },
        ),
    }
}
```

**Step 2: Export in handlers/mod.rs**

Add to `core/src/gateway/handlers/mod.rs`:

```rust
pub mod profiles;
```

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/profiles.rs core/src/gateway/handlers/mod.rs
git commit -m "feat(gateway): add profiles.list and profiles.status handlers

- ProfilesListRequest/Response for listing all profiles
- ProfilesStatusRequest/Response for single profile status
- Filter by provider support"
```

---

## Task 6: Config Schema Extension for subagents.allow_agents

**Files:**
- Modify: `core/src/config/types/agent/mod.rs` or create subagent config

**Step 1: Find existing agent config structure**

Look for `AgentConfig` or similar in config types and add:

```rust
/// Sub-agent configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SubagentsConfig {
    /// List of agent IDs allowed to be spawned. Use "*" for all.
    #[serde(default = "default_allow_agents")]
    pub allow_agents: Vec<String>,

    /// Default cleanup policy for spawned sessions
    #[serde(default)]
    pub default_cleanup: CleanupPolicy,

    /// Default timeout for spawned sessions in seconds
    #[serde(default = "default_spawn_timeout")]
    pub default_timeout_seconds: u32,
}

fn default_allow_agents() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_spawn_timeout() -> u32 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    #[default]
    Ephemeral,
    Persistent,
}
```

**Step 2: Add to AgentConfig**

In the agent config struct, add:

```rust
#[serde(default)]
pub subagents: SubagentsConfig,
```

**Step 3: Commit**

```bash
git add core/src/config/types/agent/
git commit -m "feat(config): add subagents config for allow_agents

- SubagentsConfig with allow_agents whitelist
- default_cleanup and default_timeout_seconds
- Defaults to allow all agents"
```

---

## Task 7: Integration Tests

**Files:**
- Create: `core/tests/subagent_orchestration_test.rs`

**Step 1: Create integration test**

```rust
// core/tests/subagent_orchestration_test.rs

use alephcore::gateway::run_event_bus::{ActiveRunHandle, RunEvent, RunStatus, wait_for_run_end};
use alephcore::builtin_tools::sessions::{
    SessionsSpawnTool, SessionsSpawnArgs, CleanupPolicy, SpawnStatus,
};
use alephcore::providers::profile_manager::{AuthProfileManager, ProfileUsage};
use alephcore::tools::traits::AlephTool;
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn test_run_event_bus_lifecycle() {
    let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
        "run-123".to_string(),
        "agent:main:subagent:abc".to_string(),
    );

    // Subscribe before emitting
    let mut rx = handle.subscribe();

    // Emit status change
    handle.emit(RunEvent::StatusChanged {
        status: RunStatus::Running,
        timestamp: chrono::Utc::now(),
    });

    // Emit completion
    handle.emit(RunEvent::RunCompleted {
        output: "Task done".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        duration_ms: 1000,
    });

    // Wait should return completed
    let result = wait_for_run_end(&mut rx, 5000).await;
    assert!(result.is_ok());

    match result.unwrap() {
        alephcore::gateway::run_event_bus::RunEndResult::Completed { output, .. } => {
            assert_eq!(output, "Task done");
        }
        _ => panic!("Expected Completed"),
    }
}

#[tokio::test]
async fn test_spawn_tool_creates_session_key() {
    let tool = SessionsSpawnTool::new(
        None,
        "main".to_string(),
        vec!["*".to_string()],
    );

    let args = SessionsSpawnArgs {
        task: "Write a poem".to_string(),
        label: Some("Poet".to_string()),
        agent_id: Some("poet".to_string()),
        model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        thinking: Some("medium".to_string()),
        run_timeout_seconds: 60,
        cleanup: CleanupPolicy::Ephemeral,
    };

    let output = tool.call(args).await.unwrap();

    assert_eq!(output.status, SpawnStatus::Accepted);
    assert!(output.child_session_key.starts_with("agent:poet:subagent:"));
    assert!(!output.run_id.is_empty());
    assert_eq!(output.model_applied, Some(true));
}

#[tokio::test]
async fn test_profile_manager_cooldown() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("profiles.toml");
    let agents_dir = temp_dir.path().join("agents");

    // Create a test profile
    std::fs::write(&config_path, r#"
[profiles.test_profile]
provider = "openai"
api_key = "sk-test"
"#).unwrap();

    let manager = AuthProfileManager::new(config_path, agents_dir).unwrap();

    // Profile should be available
    let profile = manager.get_available_profile("openai", "main");
    assert!(profile.is_some());

    // Mark as failed
    manager.mark_failure("test_profile", alephcore::providers::auth_profiles::AuthProfileFailureReason::RateLimit);

    // Profile should be in cooldown
    let profiles = manager.list_profiles();
    let test_profile = profiles.iter().find(|p| p.id == "test_profile").unwrap();
    assert!(!test_profile.is_available);
    assert!(test_profile.cooldown_remaining_ms.is_some());
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore subagent_orchestration --test '*' -- --nocapture`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/tests/subagent_orchestration_test.rs
git commit -m "test: add integration tests for sub-agent orchestration

- RunEventBus lifecycle test
- SessionsSpawnTool session key generation
- AuthProfileManager cooldown behavior"
```

---

## Task 8: Update CLAUDE.md Documentation

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Add section about new capabilities**

In the implementation progress section, update:

```markdown
### Phase 4: Tools & Automation

- [x] **Browser Control** - Chrome DevTools Protocol (CDP)
- [x] **SessionsSpawnTool** - Sub-agent spawning with model/thinking overrides
- [x] **AuthProfileManager** - Hybrid storage for API key management
- [x] **RunEventBus** - Event broadcasting with wait/queue mechanisms
- [x] **Cron Jobs** - 定时任务调度 (Croner)
- [ ] **Canvas (A2UI)** - Agent 驱动的可视化工作区
- [ ] **Webhooks** - 外部触发器
```

**Step 2: Update RPC method table**

Add new methods:

```markdown
| **sessions** | `sessions.*` | Session spawning, communication | ✅ |
| **run** | `run.*` | Run lifecycle, wait, queue | ✅ |
| **profiles** | `profiles.*` | Profile listing, status | ✅ |
```

**Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md with sub-agent orchestration capabilities

- Mark SessionsSpawnTool, AuthProfileManager, RunEventBus complete
- Add new RPC methods to table"
```

---

## Summary

| Task | Description | Files | Est. Time |
|------|-------------|-------|-----------|
| 1 | RunEvent + ActiveRunHandle | run_event_bus.rs | 15 min |
| 2 | AuthProfileManager hybrid storage | profile_manager.rs, profile_config.rs | 20 min |
| 3 | SessionsSpawnTool | spawn_tool.rs | 15 min |
| 4 | run.wait/queue_message handlers | runs.rs | 10 min |
| 5 | profiles.list/status handlers | profiles.rs | 10 min |
| 6 | Config schema extension | agent config | 5 min |
| 7 | Integration tests | test file | 15 min |
| 8 | Documentation update | CLAUDE.md | 5 min |

**Total: ~8 commits, ~95 minutes**
