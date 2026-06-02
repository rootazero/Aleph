//! Execution Engine
//!
//! Bridges the Gateway with the AgentLoop.
//! Manages run lifecycle, emits events, and handles cancellation.
//!
//! # Module structure
//!
//! - `engine` - Full `ExecutionEngine<P,R>` with AgentLoop integration
//! - `simple` - `SimpleExecutionEngine` for when providers/tools are not available

mod adapter;
mod callback;
mod deadline;
mod engine;
pub(crate) mod event_drain;
mod execute;
mod fast_path;
pub mod helpers;
mod history;
pub mod markdown_skill_refresh;
mod orchestrator;
mod persistence;
mod run_loop;
mod scratchpad_progress_sink;
mod simple;
mod slash_command;
mod steering;
mod tool_refresh;
mod tool_service_builder;
mod trace_sink_adapter;

#[cfg(test)]
mod tests;

pub use engine::ExecutionEngine;
pub use simple::SimpleExecutionEngine;
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use tool_service_builder::build_request_tool_service;
pub use tool_service_builder::set_confirmation_requester;
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use trace_sink_adapter::GatewayTraceSink;
#[allow(unused_imports)] // wired into run_loop.rs in this commit
pub(crate) use scratchpad_progress_sink::ScratchpadProgressSink;

use crate::gateway::media::PendingMedia;
use crate::sync_primitives::{AtomicU32, AtomicU64, Ordering};
use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::mpsc;

use super::router::SessionKey;

/// Configuration for the execution engine
#[derive(Debug, Clone)]
pub struct ExecutionEngineConfig {
    /// Maximum concurrent runs per agent
    pub max_concurrent_runs: usize,
    /// Default timeout for runs (seconds)
    pub default_timeout_secs: u64,
    /// Enable detailed tracing
    pub enable_tracing: bool,
    /// Mid-loop steering: when a message arrives for a session whose loop is
    /// already running, inject it into the live event log (the running loop
    /// consumes it at the next turn boundary) instead of rejecting with
    /// `AgentBusy`. Disable to restore the legacy busy/retry behaviour.
    pub mid_turn_steering: bool,
    /// R5 progress push: when a run is bound to a user channel, mirror
    /// scratchpad progress + watchdog-boundary events to that channel so
    /// headless / background runs aren't a black box. Pure I/O side-channel
    /// (see `scratchpad_progress_sink`). Default off — opt in via
    /// `[execution] progress_push`.
    pub scratchpad_progress_push: bool,
}

impl Default for ExecutionEngineConfig {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 5,
            default_timeout_secs: 172_800,
            enable_tracing: true,
            mid_turn_steering: true,
            scratchpad_progress_push: false,
        }
    }
}

/// A run request
#[derive(Clone)]
pub struct RunRequest {
    /// Unique run ID
    pub run_id: String,
    /// Input message
    pub input: String,
    /// Session key for context
    pub session_key: SessionKey,
    /// Optional timeout override
    pub timeout_secs: Option<u64>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
    /// Attachments from inbound message (images, audio, documents)
    pub attachments: Vec<crate::gateway::channel::Attachment>,
    /// Shared pending media buffer (for media attachment delivery)
    pub pending_media: PendingMedia,
    /// G2 — per-run sandbox override. `None` defers to the orchestrator's
    /// sandbox factory (production default); `Some(sandbox)` short-circuits
    /// the factory and is used by the team dispatcher to wrap each member
    /// task in an isolated git worktree.
    pub sandbox_override: Option<std::sync::Arc<dyn crate::sandbox::Sandbox>>,
    /// Per-run workspace override (project mode). When `Some`, this path
    /// replaces `agent.workspace()` as the effective working directory
    /// for the run — used for `ToolContext`, the default cwd of shell
    /// tools, and project-local file/skill discovery
    /// (`<root>/AGENTS.md`, `<root>/CLAUDE.md`, `<root>/.claude/skills`,
    /// `<root>/.aleph/skills`).
    ///
    /// `None` keeps the legacy behaviour of running inside
    /// `~/.aleph/workspaces/{agent_id}/`. The path is **not** validated by
    /// the engine; the gateway handler that constructs `RunRequest` is
    /// responsible for trust + existence checks.
    pub workspace_override: Option<PathBuf>,
    /// D2: per-run Think→Act iteration cap override. When `Some(n>0)`, this
    /// wins over both `FlowOverrides.max_iterations` and the boot-time
    /// `[execution] max_iterations` default. Cron-driven runs set this from
    /// `CronConfig::default_max_iterations` so a single misbehaving job
    /// can't burn the much-larger global cap (default 1000) before the
    /// wall-clock timeout fires. `None` falls through to the legacy
    /// resolution chain.
    pub max_iterations_override: Option<u32>,
    /// Chat-window picker — per-turn model override. When `Some`, the
    /// run_loop short-circuits `provider_registry.resolve_with_fallback`
    /// and pins the requested (provider, model) pair (Qualified) or the
    /// requested model with auto-resolved provider (Raw). `None` keeps the
    /// agent's configured default + fallback chain.
    pub model_override: Option<crate::gateway::model_override::ModelOverride>,
}

impl std::fmt::Debug for RunRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunRequest")
            .field("run_id", &self.run_id)
            .field("input", &self.input)
            .field("session_key", &self.session_key)
            .field("timeout_secs", &self.timeout_secs)
            .field("metadata", &self.metadata)
            .field("attachments", &self.attachments)
            .field(
                "sandbox_override",
                &self.sandbox_override.as_ref().map(|_| "<dyn Sandbox>"),
            )
            .field("workspace_override", &self.workspace_override)
            .finish()
    }
}

/// Run state
#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    /// Run is queued
    Queued,
    /// Run is executing
    Running,
    /// Run is paused (waiting for user input)
    Paused { reason: String },
    /// Run completed successfully
    Completed,
    /// Run was cancelled
    Cancelled,
    /// Run failed
    Failed { error: String },
}

/// Run status information
#[derive(Debug, Clone)]
pub struct RunStatus {
    pub run_id: String,
    pub state: RunState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub steps_completed: u32,
    pub current_tool: Option<String>,
}

/// Internal run tracking
pub(crate) struct ActiveRun {
    pub(crate) request: RunRequest,
    pub(crate) state: RunState,
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    pub(crate) completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) steps_completed: u32,
    pub(crate) current_tool: Option<String>,
    pub(crate) cancel_tx: Option<mpsc::Sender<()>>,
    pub(crate) seq_counter: AtomicU64,
    pub(crate) chunk_counter: AtomicU32,
}

impl ActiveRun {
    pub(crate) fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn next_chunk(&self) -> u32 {
        self.chunk_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Execution errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Too many concurrent runs: {0}")]
    TooManyRuns(String),

    #[error("Agent is busy: {0}")]
    AgentBusy(String),

    #[error("Run not found: {0}")]
    RunNotFound(String),

    #[error("Run is not active: {0}")]
    RunNotActive(String),

    #[error("Run was cancelled")]
    Cancelled,

    #[error("Run timed out")]
    Timeout,

    #[error("Execution failed: {0}")]
    Failed(String),

    #[error("Command requires LLM processing: {reason}")]
    Fallthrough { reason: String },

    #[error("Escalated to {route_label} after {completed_steps} steps")]
    Escalated {
        route_label: String,
        route: crate::routing::TaskRoute,
        context: crate::routing::EscalationSnapshot,
        completed_steps: usize,
    },

    #[error("orchestrator: {0}")]
    Orchestrator(String),
}
