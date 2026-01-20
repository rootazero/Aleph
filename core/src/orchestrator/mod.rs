//! Request Orchestrator - Unified entry point for request processing
//!
//! This module coordinates the ExecutionIntentDecider (Phase 1) and Dispatcher (Phase 2)
//! to provide a clean separation of concerns:
//!
//! - **Phase 1 (ExecutionIntentDecider)**: Decides "execute vs converse"
//! - **Phase 2 (Dispatcher)**: Decides "which tool and model"
//!
//! # Architecture
//!
//! ```text
//! User Input
//!     ↓
//! ┌─────────────────────────────────────────────────────────────────┐
//! │ RequestOrchestrator                                             │
//! ├─────────────────────────────────────────────────────────────────┤
//! │                                                                 │
//! │  Phase 1: ExecutionIntentDecider                                │
//! │  ├─ DirectTool → Skip Phase 2, execute directly                 │
//! │  ├─ Converse → Use conversational prompt (no tools)             │
//! │  └─ Execute(category) → Continue to Phase 2                     │
//! │                                                                 │
//! │  Phase 2: Dispatcher (only for Execute mode)                    │
//! │  ├─ Tool selection based on category                            │
//! │  ├─ Model routing via TaskIntent                                │
//! │  └─ Skip slash command processing (already handled in Phase 1)  │
//! │                                                                 │
//! └─────────────────────────────────────────────────────────────────┘
//!     ↓
//! Response (tool execution / conversation / direct tool)
//! ```
//!
//! # Benefits
//!
//! 1. **No duplicate slash command processing**: Phase 1 handles all `/command`
//! 2. **Clear separation**: "what to do" vs "how to do"
//! 3. **Simplified prompts**: No decision logic in prompts

mod request;
mod result;

pub use request::{OrchestratorRequest, RequestContext};
pub use result::{OrchestratorResult, ProcessingPhase};

use crate::dispatcher::model_router::TaskIntent;
use crate::dispatcher::{DispatcherConfig, DispatcherIntegration, UnifiedTool};
use crate::error::Result;
use crate::intent::{
    ContextSignals, DeciderConfig, DecisionResult, ExecutionIntentDecider, ExecutionMode,
    TaskCategory,
};
use crate::prompt::{PromptBuilder, PromptConfig, ToolInfo};
use tracing::{debug, info};

/// Routing options for Phase 2 dispatcher
#[derive(Debug, Clone, Default)]
pub struct RoutingOptions {
    /// Skip slash command processing (already handled in Phase 1)
    pub skip_slash_commands: bool,
    /// Preferred model override
    pub preferred_model: Option<String>,
}

/// Unified request orchestrator
///
/// Coordinates the two-phase processing:
/// 1. ExecutionIntentDecider: Decides execution mode
/// 2. Dispatcher: Routes to tools and models (only for Execute mode)
pub struct RequestOrchestrator {
    /// Phase 1: Intent decision
    intent_decider: ExecutionIntentDecider,
    /// Phase 2: Tool/model routing
    dispatcher: DispatcherIntegration,
    /// Prompt configuration
    prompt_config: PromptConfig,
}

impl RequestOrchestrator {
    /// Create a new orchestrator with default configuration
    pub fn new() -> Self {
        Self {
            intent_decider: ExecutionIntentDecider::new(),
            dispatcher: DispatcherIntegration::with_defaults(),
            prompt_config: PromptConfig::default(),
        }
    }

    /// Create with custom configurations
    pub fn with_config(
        decider_config: DeciderConfig,
        dispatcher_config: DispatcherConfig,
        prompt_config: PromptConfig,
    ) -> Self {
        Self {
            intent_decider: ExecutionIntentDecider::with_config(decider_config),
            dispatcher: DispatcherIntegration::new(dispatcher_config),
            prompt_config,
        }
    }

    /// Process a user request through the two-phase pipeline
    ///
    /// # Arguments
    ///
    /// * `request` - The user request to process
    ///
    /// # Returns
    ///
    /// An `OrchestratorResult` containing the processing outcome and metadata
    pub fn process(&self, request: &OrchestratorRequest) -> Result<OrchestratorResult> {
        // Phase 1: Intent decision
        let context_signals = request.context.as_ref().map(|ctx| ContextSignals {
            selected_file: ctx.selected_file.clone(),
            active_app: ctx.active_app.clone(),
            ui_mode: ctx.ui_mode.clone(),
            clipboard_type: ctx.clipboard_type.clone(),
        });

        let decision = self
            .intent_decider
            .decide(&request.input, context_signals.as_ref());

        info!(
            layer = ?decision.metadata.layer,
            confidence = decision.metadata.confidence,
            latency_us = decision.metadata.latency_us,
            "Phase 1 decision complete"
        );

        // Route based on execution mode
        match &decision.mode {
            ExecutionMode::DirectTool(invocation) => {
                // Direct tool invocation - skip Phase 2 entirely
                debug!(
                    tool_id = %invocation.tool_id,
                    args = %invocation.args,
                    "Direct tool invocation, skipping Phase 2"
                );

                Ok(OrchestratorResult::direct_tool(
                    invocation.tool_id.clone(),
                    invocation.args.clone(),
                    decision,
                ))
            }

            ExecutionMode::Converse => {
                // Conversation mode - generate conversational prompt
                let prompt =
                    PromptBuilder::conversational_prompt(Some(&self.prompt_config));

                debug!("Conversation mode, using conversational prompt");

                Ok(OrchestratorResult::converse(prompt, decision))
            }

            ExecutionMode::Execute(category) => {
                // Execution mode - proceed to Phase 2
                self.process_execute_mode(*category, &request.input, decision, &request.tools)
            }
        }
    }

    /// Process Execute mode through Phase 2 (Dispatcher)
    fn process_execute_mode(
        &self,
        category: TaskCategory,
        input: &str,
        decision: DecisionResult,
        available_tools: &[UnifiedTool],
    ) -> Result<OrchestratorResult> {
        debug!(
            category = ?category,
            "Phase 2: Processing execute mode"
        );

        // Convert TaskCategory to TaskIntent for model routing
        let task_intent = TaskIntent::from_category(category);

        debug!(
            task_intent = %task_intent,
            "Mapped TaskCategory to TaskIntent"
        );

        // Filter tools for this category
        let category_tools = self.filter_tools_for_category(available_tools, category);

        // Convert to ToolInfo for prompt building
        let tool_infos: Vec<ToolInfo> = category_tools
            .iter()
            .map(|t| ToolInfo::new(&t.id, &t.name, &t.description))
            .collect();

        // Generate executor prompt
        let prompt =
            PromptBuilder::executor_prompt(category, &tool_infos, Some(&self.prompt_config));

        Ok(OrchestratorResult::execute(
            prompt,
            category,
            task_intent,
            category_tools,
            decision,
        ))
    }

    /// Filter tools relevant to a task category
    ///
    /// This reduces tool list noise by only showing tools relevant to the task.
    fn filter_tools_for_category(
        &self,
        tools: &[UnifiedTool],
        category: TaskCategory,
    ) -> Vec<UnifiedTool> {
        // Category-to-tool keyword mapping
        let keywords: Vec<&str> = match category {
            TaskCategory::FileOrganize | TaskCategory::FileOperation | TaskCategory::FileTransfer | TaskCategory::FileCleanup => {
                vec!["file", "folder", "directory", "organize", "move", "copy", "delete"]
            }
            TaskCategory::ImageGeneration => {
                vec!["image", "generate", "draw", "picture", "dalle", "midjourney"]
            }
            TaskCategory::WebSearch => {
                vec!["search", "web", "google", "find"]
            }
            TaskCategory::WebFetch => {
                vec!["fetch", "web", "url", "page", "content"]
            }
            TaskCategory::CodeExecution => {
                vec!["code", "run", "execute", "script", "shell"]
            }
            TaskCategory::DocumentGeneration | TaskCategory::DocumentGenerate => {
                vec!["document", "pdf", "word", "excel", "write"]
            }
            TaskCategory::MediaDownload => {
                vec!["download", "video", "youtube", "media"]
            }
            TaskCategory::AppLaunch | TaskCategory::AppAutomation => {
                vec!["app", "launch", "open", "automation", "applescript"]
            }
            TaskCategory::TextProcessing | TaskCategory::DataProcess => {
                vec!["text", "translate", "summarize", "process", "data"]
            }
            TaskCategory::SystemInfo => {
                vec!["system", "info", "status", "disk", "memory"]
            }
            _ => vec![], // General: return all tools
        };

        if keywords.is_empty() {
            return tools.to_vec();
        }

        // Filter tools that match any keyword
        tools
            .iter()
            .filter(|tool| {
                let name_lower = tool.name.to_lowercase();
                let desc_lower = tool.description.to_lowercase();
                keywords.iter().any(|kw| {
                    name_lower.contains(kw) || desc_lower.contains(kw)
                })
            })
            .cloned()
            .collect()
    }

    /// Get a reference to the intent decider
    pub fn intent_decider(&self) -> &ExecutionIntentDecider {
        &self.intent_decider
    }

    /// Get a reference to the dispatcher
    pub fn dispatcher(&self) -> &DispatcherIntegration {
        &self.dispatcher
    }

    /// Update prompt configuration
    pub fn set_prompt_config(&mut self, config: PromptConfig) {
        self.prompt_config = config;
    }
}

impl Default for RequestOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;

    fn create_test_tools() -> Vec<UnifiedTool> {
        vec![
            UnifiedTool::new("native:search", "search", "Search the web", ToolSource::Native),
            UnifiedTool::new("native:file_ops", "file_ops", "File operations", ToolSource::Native),
            UnifiedTool::new("native:generate_image", "generate_image", "Generate images", ToolSource::Native),
        ]
    }

    #[test]
    fn test_orchestrator_slash_command() {
        let orchestrator = RequestOrchestrator::new();
        let request = OrchestratorRequest {
            input: "/screenshot".to_string(),
            context: None,
            tools: vec![],
        };

        let result = orchestrator.process(&request).unwrap();

        assert!(result.is_direct_tool());
        assert_eq!(result.tool_id(), Some("screenshot"));
        assert_eq!(result.phase, ProcessingPhase::Phase1Only);
    }

    #[test]
    fn test_orchestrator_conversation() {
        let orchestrator = RequestOrchestrator::new();
        let request = OrchestratorRequest {
            input: "什么是机器学习？".to_string(),
            context: None,
            tools: vec![],
        };

        let result = orchestrator.process(&request).unwrap();

        assert!(result.is_converse());
        assert!(result.prompt.is_some());
        assert_eq!(result.phase, ProcessingPhase::Phase1Only);
    }

    #[test]
    fn test_orchestrator_execute_mode() {
        let orchestrator = RequestOrchestrator::new();
        let tools = create_test_tools();
        let request = OrchestratorRequest {
            input: "整理我的下载文件夹".to_string(),
            context: None,
            tools,
        };

        let result = orchestrator.process(&request).unwrap();

        assert!(result.is_execute());
        assert_eq!(result.category, Some(TaskCategory::FileOrganize));
        assert!(result.task_intent.is_some());
        assert_eq!(result.phase, ProcessingPhase::Phase1And2);
    }

    #[test]
    fn test_filter_tools_for_category() {
        let orchestrator = RequestOrchestrator::new();
        let tools = create_test_tools();

        // File category should filter to file_ops
        let file_tools = orchestrator.filter_tools_for_category(&tools, TaskCategory::FileOrganize);
        assert!(file_tools.iter().any(|t| t.name == "file_ops"));
        assert!(!file_tools.iter().any(|t| t.name == "generate_image"));

        // Image category should filter to generate_image
        let image_tools = orchestrator.filter_tools_for_category(&tools, TaskCategory::ImageGeneration);
        assert!(image_tools.iter().any(|t| t.name == "generate_image"));

        // General category should return all tools
        let general_tools = orchestrator.filter_tools_for_category(&tools, TaskCategory::General);
        assert_eq!(general_tools.len(), tools.len());
    }

    #[test]
    fn test_orchestrator_with_context() {
        let orchestrator = RequestOrchestrator::new();
        let request = OrchestratorRequest {
            input: "处理这个".to_string(),
            context: Some(RequestContext {
                selected_file: Some("/path/to/photo.jpg".to_string()),
                active_app: None,
                ui_mode: None,
                clipboard_type: None,
            }),
            tools: vec![],
        };

        let result = orchestrator.process(&request).unwrap();

        // With image file selected, should be Execute mode with ImageGeneration category
        assert!(result.is_execute());
        assert_eq!(result.category, Some(TaskCategory::ImageGeneration));
    }
}
