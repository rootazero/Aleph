//! Result types for the orchestrator
//!
//! # Deprecated
//!
//! This module is being replaced by the Agent Loop architecture.
//! See `crate::agent_loop` for the new implementation.

#![allow(deprecated)]

use crate::dispatcher::model_router::TaskIntent;
use crate::dispatcher::UnifiedTool;
use crate::intent::{DecisionResult, TaskCategory};

/// Which phases were executed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingPhase {
    /// Only Phase 1 was needed (DirectTool or Converse)
    Phase1Only,
    /// Both Phase 1 and Phase 2 were executed (Execute mode)
    Phase1And2,
}

impl ProcessingPhase {
    /// Check if Phase 2 was executed
    pub fn includes_phase2(&self) -> bool {
        matches!(self, Self::Phase1And2)
    }
}

/// Execution mode determined by the orchestrator
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorMode {
    /// Direct tool invocation from slash command (builtin tools)
    DirectTool {
        /// Tool ID to invoke
        tool_id: String,
        /// Arguments for the tool
        args: String,
    },
    /// Skill-based execution with injected instructions
    Skill {
        /// Skill identifier
        skill_id: String,
        /// Display name
        display_name: String,
        /// Skill instructions to inject
        instructions: String,
        /// Arguments
        args: String,
    },
    /// MCP server tool execution
    Mcp {
        /// MCP server name
        server_name: String,
        /// Specific tool name (if any)
        tool_name: Option<String>,
        /// Arguments
        args: String,
    },
    /// Custom command with system prompt
    Custom {
        /// Command name
        command_name: String,
        /// System prompt to inject
        system_prompt: Option<String>,
        /// Provider override
        provider: Option<String>,
        /// Arguments
        args: String,
    },
    /// Execute mode with task category
    Execute {
        /// Task category
        category: TaskCategory,
        /// Mapped task intent for model routing
        task_intent: TaskIntent,
    },
    /// Conversation mode
    Converse,
}

/// Result from the orchestrator
#[derive(Debug, Clone)]
pub struct OrchestratorResult {
    /// The determined execution mode
    pub mode: OrchestratorMode,

    /// Which phases were executed
    pub phase: ProcessingPhase,

    /// System prompt to use (for Execute and Converse modes)
    pub prompt: Option<String>,

    /// Task category (for Execute mode)
    pub category: Option<TaskCategory>,

    /// Task intent for model routing (for Execute mode)
    pub task_intent: Option<TaskIntent>,

    /// Filtered tools for this request (for Execute mode)
    pub tools: Vec<UnifiedTool>,

    /// Phase 1 decision details
    pub decision: DecisionResult,
}

impl OrchestratorResult {
    /// Create a result for direct tool invocation
    pub fn direct_tool(tool_id: String, args: String, decision: DecisionResult) -> Self {
        Self {
            mode: OrchestratorMode::DirectTool {
                tool_id: tool_id.clone(),
                args,
            },
            phase: ProcessingPhase::Phase1Only,
            prompt: None,
            category: None,
            task_intent: None,
            tools: Vec::new(),
            decision,
        }
    }

    /// Create a result for conversation mode
    pub fn converse(prompt: String, decision: DecisionResult) -> Self {
        Self {
            mode: OrchestratorMode::Converse,
            phase: ProcessingPhase::Phase1Only,
            prompt: Some(prompt),
            category: None,
            task_intent: None,
            tools: Vec::new(),
            decision,
        }
    }

    /// Create a result for execute mode
    pub fn execute(
        prompt: String,
        category: TaskCategory,
        task_intent: TaskIntent,
        tools: Vec<UnifiedTool>,
        decision: DecisionResult,
    ) -> Self {
        Self {
            mode: OrchestratorMode::Execute {
                category,
                task_intent: task_intent.clone(),
            },
            phase: ProcessingPhase::Phase1And2,
            prompt: Some(prompt),
            category: Some(category),
            task_intent: Some(task_intent),
            tools,
            decision,
        }
    }

    /// Create a result for skill execution
    pub fn skill(
        skill_id: String,
        display_name: String,
        instructions: String,
        args: String,
        prompt: String,
        tools: Vec<UnifiedTool>,
        decision: DecisionResult,
    ) -> Self {
        Self {
            mode: OrchestratorMode::Skill {
                skill_id,
                display_name,
                instructions,
                args,
            },
            phase: ProcessingPhase::Phase1Only,
            prompt: Some(prompt),
            category: None,
            task_intent: None,
            tools,
            decision,
        }
    }

    /// Create a result for MCP execution
    pub fn mcp(
        server_name: String,
        tool_name: Option<String>,
        args: String,
        prompt: String,
        tools: Vec<UnifiedTool>,
        decision: DecisionResult,
    ) -> Self {
        Self {
            mode: OrchestratorMode::Mcp {
                server_name,
                tool_name,
                args,
            },
            phase: ProcessingPhase::Phase1Only,
            prompt: Some(prompt),
            category: None,
            task_intent: None,
            tools,
            decision,
        }
    }

    /// Create a result for custom command execution
    pub fn custom(
        command_name: String,
        system_prompt: Option<String>,
        provider: Option<String>,
        args: String,
        prompt: String,
        tools: Vec<UnifiedTool>,
        decision: DecisionResult,
    ) -> Self {
        Self {
            mode: OrchestratorMode::Custom {
                command_name,
                system_prompt,
                provider,
                args,
            },
            phase: ProcessingPhase::Phase1Only,
            prompt: Some(prompt),
            category: None,
            task_intent: None,
            tools,
            decision,
        }
    }

    /// Check if this is a direct tool invocation
    pub fn is_direct_tool(&self) -> bool {
        matches!(self.mode, OrchestratorMode::DirectTool { .. })
    }

    /// Check if this is execute mode
    pub fn is_execute(&self) -> bool {
        matches!(self.mode, OrchestratorMode::Execute { .. })
    }

    /// Check if this is conversation mode
    pub fn is_converse(&self) -> bool {
        matches!(self.mode, OrchestratorMode::Converse)
    }

    /// Get tool ID if this is a direct tool invocation
    pub fn tool_id(&self) -> Option<&str> {
        match &self.mode {
            OrchestratorMode::DirectTool { tool_id, .. } => Some(tool_id),
            _ => None,
        }
    }

    /// Get tool arguments if this is a direct tool invocation
    pub fn tool_args(&self) -> Option<&str> {
        match &self.mode {
            OrchestratorMode::DirectTool { args, .. } => Some(args),
            _ => None,
        }
    }

    /// Get the decision confidence
    pub fn confidence(&self) -> f32 {
        self.decision.metadata.confidence
    }

    /// Get the decision latency in microseconds
    pub fn latency_us(&self) -> u64 {
        self.decision.metadata.latency_us
    }

    /// Check if Phase 2 was executed
    pub fn used_phase2(&self) -> bool {
        self.phase.includes_phase2()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{DecisionMetadata, ExecutionMode, IntentLayer};

    fn create_mock_decision(mode: ExecutionMode) -> DecisionResult {
        DecisionResult {
            mode,
            metadata: DecisionMetadata {
                layer: IntentLayer::PatternMatch,
                confidence: 0.95,
                latency_us: 100,
                matched_pattern: None,
            },
        }
    }

    #[test]
    fn test_result_direct_tool() {
        let decision = create_mock_decision(ExecutionMode::Converse);
        let result = OrchestratorResult::direct_tool(
            "screenshot".to_string(),
            "".to_string(),
            decision,
        );

        assert!(result.is_direct_tool());
        assert!(!result.is_execute());
        assert!(!result.is_converse());
        assert_eq!(result.tool_id(), Some("screenshot"));
        assert_eq!(result.phase, ProcessingPhase::Phase1Only);
    }

    #[test]
    fn test_result_converse() {
        let decision = create_mock_decision(ExecutionMode::Converse);
        let result = OrchestratorResult::converse("test prompt".to_string(), decision);

        assert!(result.is_converse());
        assert!(!result.is_direct_tool());
        assert!(!result.is_execute());
        assert!(result.prompt.is_some());
        assert_eq!(result.phase, ProcessingPhase::Phase1Only);
    }

    #[test]
    fn test_result_execute() {
        let decision = create_mock_decision(ExecutionMode::Execute(TaskCategory::FileOrganize));
        let result = OrchestratorResult::execute(
            "test prompt".to_string(),
            TaskCategory::FileOrganize,
            TaskIntent::QuickTask,
            vec![],
            decision,
        );

        assert!(result.is_execute());
        assert!(!result.is_direct_tool());
        assert!(!result.is_converse());
        assert_eq!(result.category, Some(TaskCategory::FileOrganize));
        assert_eq!(result.task_intent, Some(TaskIntent::QuickTask));
        assert_eq!(result.phase, ProcessingPhase::Phase1And2);
        assert!(result.used_phase2());
    }

    #[test]
    fn test_processing_phase() {
        assert!(!ProcessingPhase::Phase1Only.includes_phase2());
        assert!(ProcessingPhase::Phase1And2.includes_phase2());
    }
}
