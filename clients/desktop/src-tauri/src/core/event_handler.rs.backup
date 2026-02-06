//! TauriEventHandler - bridges alephcore callbacks to Tauri window events
//!
//! This handler implements the AlephEventHandler trait from alephcore
//! and forwards all callbacks to the Tauri frontend via window.emit().

use tauri::{AppHandle, Emitter, Runtime};
use tracing::{debug, error, info};

/// Event handler that forwards alephcore callbacks to Tauri frontend
pub struct TauriEventHandler<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriEventHandler<R> {
    /// Create a new TauriEventHandler
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }

    /// Emit an event to all windows
    fn emit_all(&self, event: &str, payload: impl serde::Serialize + Clone) {
        if let Err(e) = self.app.emit(event, payload) {
            error!(event = event, error = %e, "Failed to emit event");
        }
    }
}

// Implement AlephEventHandler for any Runtime
impl<R: Runtime + 'static> alephcore::ffi::AlephEventHandler for TauriEventHandler<R> {
    /// Called when AI starts processing (thinking)
    fn on_thinking(&self) {
        debug!("AI thinking started");
        self.emit_all("aleph:thinking", ());
    }

    /// Called when a tool execution starts
    fn on_tool_start(&self, tool_name: String) {
        debug!(tool = %tool_name, "Tool started");
        self.emit_all("aleph:tool-start", ToolStartPayload { tool_name });
    }

    /// Called when a tool execution completes
    fn on_tool_result(&self, tool_name: String, result: String) {
        debug!(tool = %tool_name, "Tool completed");
        self.emit_all(
            "aleph:tool-result",
            ToolResultPayload { tool_name, result },
        );
    }

    /// Called for each streaming chunk of the response
    fn on_stream_chunk(&self, text: String) {
        self.emit_all("aleph:stream-chunk", StreamChunkPayload { text });
    }

    /// Called when processing completes with the full response
    fn on_complete(&self, response: String) {
        info!("Processing completed");
        self.emit_all("aleph:complete", CompletePayload { response });
    }

    /// Called when an error occurs
    fn on_error(&self, message: String) {
        error!(message = %message, "Processing error");
        self.emit_all("aleph:error", ErrorPayload { message });
    }

    /// Called when a memory entry is stored
    fn on_memory_stored(&self) {
        debug!("Memory stored");
        self.emit_all("aleph:memory-stored", ());
    }

    /// Called when agent execution mode is detected
    fn on_agent_mode_detected(&self, task: alephcore::intent::ExecutableTaskFFI) {
        info!(task_category = ?task.category, "Agent mode detected");
        self.emit_all(
            "aleph:agent-mode-detected",
            AgentModePayload {
                task_category: format!("{:?}", task.category),
                action: task.action,
                target: task.target,
                confidence: task.confidence,
            },
        );
    }

    // ========================================================================
    // HOT-RELOAD CALLBACKS
    // ========================================================================

    /// Called when tool registry is updated
    fn on_tools_changed(&self, tool_count: u32) {
        info!(tool_count, "Tools changed");
        self.emit_all("aleph:tools-changed", ToolsChangedPayload { tool_count });
    }

    /// Called when MCP servers have finished starting
    fn on_mcp_startup_complete(&self, report: alephcore::McpStartupReportFFI) {
        info!(
            succeeded = report.succeeded_servers.len(),
            failed = report.failed_servers.len(),
            "MCP startup complete"
        );
        self.emit_all(
            "aleph:mcp-startup-complete",
            McpStartupPayload {
                succeeded: report.succeeded_servers,
                failed: report
                    .failed_servers
                    .into_iter()
                    .map(|f| McpServerError {
                        server_name: f.server_name,
                        error_message: f.error_message,
                    })
                    .collect(),
            },
        );
    }

    /// Called when runtime updates are available
    fn on_runtime_updates_available(&self, updates: Vec<alephcore::RuntimeUpdateInfo>) {
        info!(count = updates.len(), "Runtime updates available");
        self.emit_all(
            "aleph:runtime-updates",
            RuntimeUpdatesPayload {
                updates: updates
                    .into_iter()
                    .map(|u| RuntimeUpdate {
                        runtime_id: u.runtime_id,
                        current_version: u.current_version,
                        latest_version: u.latest_version,
                    })
                    .collect(),
            },
        );
    }

    // ========================================================================
    // AGENTIC LOOP CALLBACKS
    // ========================================================================

    /// Called when a new session is created
    fn on_session_started(&self, session_id: String) {
        info!(session_id = %session_id, "Session started");
        self.emit_all("aleph:session-started", SessionPayload { session_id });
    }

    /// Called when tool execution starts (with call_id for tracking)
    fn on_tool_call_started(&self, call_id: String, tool_name: String) {
        debug!(call_id = %call_id, tool = %tool_name, "Tool call started");
        self.emit_all(
            "aleph:tool-call-started",
            ToolCallStartPayload { call_id, tool_name },
        );
    }

    /// Called when tool execution completes
    fn on_tool_call_completed(&self, call_id: String, output: String) {
        debug!(call_id = %call_id, "Tool call completed");
        self.emit_all(
            "aleph:tool-call-completed",
            ToolCallCompletePayload { call_id, output },
        );
    }

    /// Called when tool execution fails
    fn on_tool_call_failed(&self, call_id: String, error: String, is_retryable: bool) {
        error!(call_id = %call_id, error = %error, "Tool call failed");
        self.emit_all(
            "aleph:tool-call-failed",
            ToolCallFailedPayload {
                call_id,
                error,
                is_retryable,
            },
        );
    }

    /// Called on each loop iteration with progress update
    fn on_loop_progress(&self, session_id: String, iteration: u32, status: String) {
        debug!(session_id = %session_id, iteration, "Loop progress");
        self.emit_all(
            "aleph:loop-progress",
            LoopProgressPayload {
                session_id,
                iteration,
                status,
            },
        );
    }

    /// Called when a plan is created for multi-step task
    fn on_plan_created(&self, session_id: String, steps: Vec<String>) {
        info!(session_id = %session_id, steps = steps.len(), "Plan created");
        self.emit_all(
            "aleph:plan-created",
            PlanCreatedPayload { session_id, steps },
        );
    }

    /// Called when session completes
    fn on_session_completed(&self, session_id: String, summary: String) {
        info!(session_id = %session_id, "Session completed");
        self.emit_all(
            "aleph:session-completed",
            SessionCompletedPayload {
                session_id,
                summary,
            },
        );
    }

    /// Called when sub-agent is started
    fn on_subagent_started(
        &self,
        parent_session_id: String,
        child_session_id: String,
        agent_id: String,
    ) {
        debug!(
            parent = %parent_session_id,
            child = %child_session_id,
            agent = %agent_id,
            "Sub-agent started"
        );
        self.emit_all(
            "aleph:subagent-started",
            SubagentStartedPayload {
                parent_session_id,
                child_session_id,
                agent_id,
            },
        );
    }

    /// Called when sub-agent completes
    fn on_subagent_completed(&self, child_session_id: String, success: bool, summary: String) {
        debug!(
            session_id = %child_session_id,
            success,
            "Sub-agent completed"
        );
        self.emit_all(
            "aleph:subagent-completed",
            SubagentCompletedPayload {
                child_session_id,
                success,
                summary,
            },
        );
    }

    // ========================================================================
    // DAG PLAN CONFIRMATION CALLBACKS
    // ========================================================================

    /// Called when a DAG task plan requires user confirmation
    fn on_plan_confirmation_required(
        &self,
        plan_id: String,
        plan: alephcore::dispatcher::DagTaskPlan,
    ) {
        info!(plan_id = %plan_id, tasks = plan.tasks.len(), "Plan confirmation required");
        self.emit_all(
            "aleph:plan-confirmation-required",
            PlanConfirmationPayload {
                plan_id,
                title: plan.title,
                tasks: plan
                    .tasks
                    .into_iter()
                    .map(|t| PlanTask {
                        id: t.id,
                        name: t.name,
                        status: format!("{:?}", t.status),
                        risk_level: t.risk_level,
                    })
                    .collect(),
            },
        );
    }
}

// ============================================================================
// Event Payloads
// ============================================================================

#[derive(Debug, Clone, serde::Serialize)]
struct ToolStartPayload {
    tool_name: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ToolResultPayload {
    tool_name: String,
    result: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct StreamChunkPayload {
    text: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct CompletePayload {
    response: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ErrorPayload {
    message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AgentModePayload {
    task_category: String,
    action: String,
    target: Option<String>,
    confidence: f32,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ToolsChangedPayload {
    tool_count: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
struct McpStartupPayload {
    succeeded: Vec<String>,
    failed: Vec<McpServerError>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct McpServerError {
    server_name: String,
    error_message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct RuntimeUpdatesPayload {
    updates: Vec<RuntimeUpdate>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct RuntimeUpdate {
    runtime_id: String,
    current_version: String,
    latest_version: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SessionPayload {
    session_id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ToolCallStartPayload {
    call_id: String,
    tool_name: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ToolCallCompletePayload {
    call_id: String,
    output: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ToolCallFailedPayload {
    call_id: String,
    error: String,
    is_retryable: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
struct LoopProgressPayload {
    session_id: String,
    iteration: u32,
    status: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PlanCreatedPayload {
    session_id: String,
    steps: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SessionCompletedPayload {
    session_id: String,
    summary: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SubagentStartedPayload {
    parent_session_id: String,
    child_session_id: String,
    agent_id: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SubagentCompletedPayload {
    child_session_id: String,
    success: bool,
    summary: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PlanConfirmationPayload {
    plan_id: String,
    title: String,
    tasks: Vec<PlanTask>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct PlanTask {
    id: String,
    name: String,
    status: String,
    risk_level: String,
}
