//! `flow_run` LLM tool — opt-in recursive dispatch. See design §7.
//!
//! Registration wiring with the global `ToolService` is a Phase 6 follow-up
//! (requires an `AlephTool` adapter). Phase 5 defines the tool's shape and
//! dispatch logic so downstream wiring lands on a stable API.

use crate::sync_primitives::Arc;

use serde::Deserialize;

use crate::orchestrator::dispatch::{FlowRequest, Orchestrator};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::FlowInput;
use crate::orchestrator::resolver::MAX_FLOW_DEPTH;

/// Handle used to invoke sub-flows. Holds the shared Orchestrator so every
/// call routes through the same flow registry + session / sandbox state.
pub struct FlowRunTool {
    pub orchestrator: Arc<Orchestrator>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlowRunInput {
    pub flow_id: String,
    pub input: String,
}

/// Per-call dispatch context supplied by the enclosing harness.
#[derive(Debug, Clone)]
pub struct FlowRunContext {
    pub parent_session_key: String,
    pub current_depth: u8,
}

/// Descriptor shape exposed to the LLM via the tool catalog.
#[derive(Debug, Clone)]
pub struct FlowRunDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: serde_json::Value,
}

impl FlowRunTool {
    #[must_use]
    pub fn descriptor() -> FlowRunDescriptor {
        FlowRunDescriptor {
            name: "flow_run",
            description: "Invoke a sub-flow and return its final text output.",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "flow_id": { "type": "string" },
                    "input":   { "type": "string" }
                },
                "required": ["flow_id", "input"]
            }),
        }
    }

    /// Dispatch a sub-flow. Enforces `MAX_FLOW_DEPTH` before reaching the
    /// orchestrator so we fail fast with a typed error. `ctx.current_depth`
    /// is the depth of the ENCLOSING flow — this function bumps by 1.
    pub async fn execute(
        &self,
        input: FlowRunInput,
        ctx: FlowRunContext,
    ) -> Result<String, FlowError> {
        // Consistent with resolver::depth_guard: depth > MAX_FLOW_DEPTH is rejected.
        if ctx.current_depth > MAX_FLOW_DEPTH {
            return Err(FlowError::RecursionLimit {
                max: MAX_FLOW_DEPTH,
            });
        }
        let req = FlowRequest {
            flow_id: Some(input.flow_id),
            agent_id: String::new(), // ignored when flow_id is explicit
            input: FlowInput::Prompt(input.input),
            channel: None,
            session_hint: None,
            parent_session: Some(ctx.parent_session_key),
            depth: ctx.current_depth.saturating_add(1),
            tool_service: None,
            trace_sink: None,
            // Subagent flows have no originating channel; fall back to
            // the harness bridge's Background paradigm default.
            interaction_manifest: None,
            sandbox_override: None,
            // Subagent sub-flow inherits the parent's project root via the
            // surrounding RunRequest path (see send_tool / teams dispatcher).
            workspace_override: None,
            max_iterations_override: None,
            // Subagent sub-flows carry no gateway per-turn reminders.
            transient_context: None,
            // No per-run thinking directive on this path — the provider keeps its
            // own default, which is what every release before this field sent.
            think_level: None,
            // Subagent sub-flows carry no resolved exec tier — no approval line.
            exec_tier: None,
            session_mode: None,
        };
        let handle = self.orchestrator.dispatch(req).await?;
        let outcome = handle
            .completion
            .await
            .map_err(|e| FlowError::Internal(format!("completion dropped: {e}")))??;
        Ok(outcome.final_text)
    }
}
