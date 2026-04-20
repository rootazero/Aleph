//! Orchestrator core + seven-step dispatch. See design §6.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_registry::FlowRegistry;
use crate::orchestrator::flow_spec::{AgentId, FlowId, FlowInput, FlowSpec};
use crate::orchestrator::resolver::{
    depth_guard, resolve_flow_id, resolve_session, RoutingOverrides, SessionResolveInput,
};
use crate::orchestrator::sandbox_factory::SandboxFactory;

/// Spawn handle returned to the Gateway.
pub struct FlowHandle {
    pub session_key: String,
    pub events: broadcast::Receiver<FlowStreamEvent>,
    pub completion: oneshot::Receiver<Result<FlowOutcome, FlowError>>,
    pub cancel: CancellationToken,
}

impl std::fmt::Debug for FlowHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowHandle")
            .field("session_key", &self.session_key)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FlowStreamEvent {
    /// Incremental assistant text (preserved from original).
    Delta(String),
    /// Thinking/reasoning fragment. Fired when provider returns thinking content.
    Reasoning(String),
    /// Tool call started. `id` uniquely identifies this invocation for pairing
    /// with `ToolCallDone` and `ToolSummary`.
    ToolCallStart {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// Tool call completed. `result` and `error` are mutually exclusive.
    ToolCallDone {
        id: String,
        result: Option<serde_json::Value>,
        error: Option<String>,
    },
    /// One-line summary of a tool call (async LLM-generated; silently skipped on failure).
    ToolSummary { id: String, text: String },
    /// Safety gate blocked the turn. `reason` is for i18n formatting.
    SafetyBlock { reason: String },
    /// Stop-hook blocked the turn; harness has forced another model turn.
    StopHookBlock { reason: String },
    /// Model fallback (primary provider unavailable, switched to backup).
    ModelFallback { reason: String, fallback_model: String },
    /// Terminal event — carries the complete `FlowOutcome`. Always last.
    Complete(FlowOutcome),
}

#[derive(Debug, Clone, Default)]
pub struct FlowOutcome {
    /// Final assistant-visible text for this flow run.
    pub final_text: String,
    /// Number of assistant turns (AssistantMessage events) produced.
    pub iterations: u32,
    /// Number of tool dispatches (ToolCallRequested events).
    pub tool_calls_made: u32,
    /// Sum of provider-reported token usage across the flow run.
    /// Populated once the LLM layer surfaces usage through the session log;
    /// today the field is present for Gateway parity with the retiring
    /// `LoopRunResult` and reads as `0` until Phase 6a task 6 wires
    /// `ContextBudget` observations into the report.
    pub total_tokens: u32,
    /// `true` if the run stopped because it hit an iteration or token cap.
    /// Defaults to `false`; actual cap tracking lands in Phase 6a task 6.
    pub hit_limit: bool,
}

#[derive(Clone)]
pub struct FlowRequest {
    pub flow_id: Option<FlowId>,
    pub agent_id: AgentId,
    pub input: FlowInput,
    pub channel: Option<String>,
    pub session_hint: Option<String>,
    pub parent_session: Option<String>,
    pub depth: u8,
    /// Per-request tool service override. `None` causes `AgentHarnessRunner`
    /// to fall back to its default `tool_service`. Gateway production path
    /// must supply `Some` to carry per-request dynamic tools.
    /// Not included in `Debug` output because `Arc<dyn ToolService>` is not `Debug`.
    pub tool_service: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
    /// Gateway-side observability sink. `None` is a no-op.
    /// Not included in `Debug` output because `Arc<dyn TraceSink>` is not `Debug`.
    pub trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
}

impl std::fmt::Debug for FlowRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowRequest")
            .field("flow_id", &self.flow_id)
            .field("agent_id", &self.agent_id)
            .field("input", &self.input)
            .field("channel", &self.channel)
            .field("session_hint", &self.session_hint)
            .field("parent_session", &self.parent_session)
            .field("depth", &self.depth)
            .field("tool_service", &self.tool_service.as_ref().map(|_| "<dyn ToolService>"))
            .field("trace_sink", &self.trace_sink.as_ref().map(|_| "<dyn TraceSink>"))
            .finish()
    }
}

/// Orchestrator dependencies. Most are behind `Arc<dyn Trait>` so the struct
/// itself is cheap to share. Per-session lock is an internal `Mutex<HashSet>`.
pub struct Orchestrator {
    pub flow_registry: Arc<FlowRegistry>,
    pub routing_overrides: Arc<RoutingOverrides>,
    pub default_routing: Arc<HashMap<AgentId, FlowId>>,
    pub session_service: Arc<dyn crate::session::service::SessionService>,
    pub sandbox_factory: SandboxFactory,
    /// Harness runner injected at construction — test mocks can swap this out.
    pub harness: Arc<dyn HarnessRunner>,
    active_sessions: Arc<Mutex<HashSet<String>>>,
}

/// Removes `key` from `active` on drop — runs even if the spawned
/// harness task panics.
struct SessionLockGuard {
    active: Arc<Mutex<HashSet<String>>>,
    key: String,
}

impl Drop for SessionLockGuard {
    fn drop(&mut self) {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(&self.key);
    }
}

#[async_trait::async_trait]
pub trait HarnessRunner: Send + Sync {
    async fn run(
        &self,
        session_key: String,
        spec: Arc<FlowSpec>,
        input: FlowInput,
        sandbox: Arc<dyn crate::sandbox::Sandbox>,
        events: broadcast::Sender<FlowStreamEvent>,
        cancel: CancellationToken,
        tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
        trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
    ) -> Result<FlowOutcome, FlowError>;
}

impl Orchestrator {
    pub fn new(
        flow_registry: Arc<FlowRegistry>,
        routing_overrides: Arc<RoutingOverrides>,
        default_routing: Arc<HashMap<AgentId, FlowId>>,
        session_service: Arc<dyn crate::session::service::SessionService>,
        sandbox_factory: SandboxFactory,
        harness: Arc<dyn HarnessRunner>,
    ) -> Self {
        Self {
            flow_registry,
            routing_overrides,
            default_routing,
            session_service,
            sandbox_factory,
            harness,
            active_sessions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Seven-step dispatch. See design §6.
    pub async fn dispatch(&self, req: FlowRequest) -> Result<FlowHandle, FlowError> {
        // Step 2: depth guard (cheap, first, to reject runaway callers).
        depth_guard(req.depth)?;

        // Step 1: resolve flow_id → FlowSpec.
        let flow_id = match &req.flow_id {
            Some(id) => id.clone(),
            None => resolve_flow_id(
                &req.agent_id,
                req.channel.as_deref(),
                &self.routing_overrides,
                &self.default_routing,
            )?,
        };
        let spec = self
            .flow_registry
            .resolve(&flow_id)
            .ok_or_else(|| FlowError::UnknownFlow(flow_id.clone()))?;

        // Step 3: agent lookup deferred to harness (it holds the AgentRegistry).

        // Step 4: session resolve + per-session lock.
        let session_input = SessionResolveInput {
            strategy: spec.session_strategy.clone(),
            session_hint: req.session_hint.clone(),
            parent_session: req.parent_session.clone(),
            fresh_key_fn: || uuid::Uuid::new_v4().to_string(),
        };
        let session_res = resolve_session(session_input)?;
        {
            let mut guard = self
                .active_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !guard.insert(session_res.session_key.clone()) {
                return Err(FlowError::SessionConflict(session_res.session_key));
            }
        }

        // Step 5: brain pick deferred to HarnessRunner (ProviderRegistry lives there).

        // Step 6: sandbox provision.
        let sandbox = (self.sandbox_factory)(spec.sandbox_kind, &session_res.session_key)?;

        // Step 7: spawn harness, plumbing events + completion + cancel.
        let (event_tx, event_rx) = broadcast::channel::<FlowStreamEvent>(256);
        let (done_tx, done_rx) = oneshot::channel();
        let cancel = CancellationToken::new();

        let harness = self.harness.clone();
        let spec_clone = spec.clone();
        let input_clone = req.input.clone();
        let sandbox_clone = sandbox.clone();
        let cancel_clone = cancel.clone();
        let session_key = session_res.session_key.clone();
        let active = self.active_sessions.clone();
        let session_for_release = session_res.session_key.clone();
        let tool_service_override = req.tool_service.clone();
        let trace_sink = req.trace_sink.clone();

        tokio::spawn(async move {
            let _lock = SessionLockGuard {
                active,
                key: session_for_release,
            };
            let outcome = harness
                .run(
                    session_key,
                    spec_clone,
                    input_clone,
                    sandbox_clone,
                    event_tx,
                    cancel_clone,
                    tool_service_override,
                    trace_sink,
                )
                .await;
            let _ = done_tx.send(outcome);
            // _lock drops here, releasing the session key regardless of panic.
        });

        Ok(FlowHandle {
            session_key: session_res.session_key,
            events: event_rx,
            completion: done_rx,
            cancel,
        })
    }

    pub async fn reload_flows(
        &self,
        new_set: crate::orchestrator::flow_registry::FlowSet,
    ) -> Result<(), FlowError> {
        self.flow_registry.replace(new_set);
        debug!(count = self.flow_registry.len(), "flow registry reloaded");
        Ok(())
    }
}
