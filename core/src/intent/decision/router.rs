//! Intent Router for Agent Loop
//!
//! This module provides a simplified routing interface that wraps the
//! ExecutionIntentDecider's L0-L2 fast routing capabilities for use
//! by the Agent Loop.
//!
//! # Architecture
//!
//! ```text
//! User Input
//!     ↓
//! ┌─────────────────────────────────────────────────────────────┐
//! │ IntentRouter (fast routing, <20ms)                          │
//! │                                                             │
//! │  L0: Slash Commands → DirectRoute (skip LLM thinking)       │
//! │      ↓ (no match)                                           │
//! │  L1: Regex Patterns → CategoryHint (guide LLM)              │
//! │      ↓ (no match)                                           │
//! │  L2: Context Signals → CategoryHint (guide LLM)             │
//! │      ↓ (no match)                                           │
//! │  NeedsThinking → Let Agent Loop's Thinker decide            │
//! │                                                             │
//! └─────────────────────────────────────────────────────────────┘
//!         ↓
//!     RouteResult
//! ```
//!
//! # Key Differences from ExecutionIntentDecider
//!
//! - No L3/L4 (LLM decision): These are now handled by Agent Loop's Thinker
//! - Simplified output: DirectRoute (skip thinking) vs NeedsThinking
//! - Category hints for LLM guidance when direct routing isn't possible

use super::execution_decider::{
    ContextSignals, CustomInvocation, DeciderConfig, ExecutionIntentDecider, ExecutionMode,
    IntentLayer, McpInvocation, SkillInvocation, ToolInvocation,
};
use crate::intent::types::TaskCategory;
use crate::command::CommandParser;
use std::sync::Arc;

/// Routing result from IntentRouter
#[derive(Debug, Clone)]
pub enum RouteResult {
    /// Direct route - skip LLM thinking, execute immediately
    /// Used for slash commands and direct tool invocations
    DirectRoute(DirectRouteInfo),

    /// Needs LLM thinking to decide next action
    /// May include category hints from L1/L2 matching
    NeedsThinking(ThinkingContext),
}

/// Information for direct routing (bypass LLM thinking)
#[derive(Debug, Clone)]
pub struct DirectRouteInfo {
    /// The execution mode determined by L0 routing
    pub mode: DirectMode,
    /// Layer that made the decision
    pub layer: IntentLayer,
    /// Processing time in microseconds
    pub latency_us: u64,
}

/// Direct execution modes (no LLM thinking required)
#[derive(Debug, Clone)]
pub enum DirectMode {
    /// Built-in tool invocation (e.g., /screenshot, /search)
    Tool(ToolInvocation),
    /// Skill-based execution with instructions
    Skill(SkillInvocation),
    /// MCP server tool execution
    Mcp(McpInvocation),
    /// Custom command with system prompt
    Custom(CustomInvocation),
}

/// Context for LLM thinking
#[derive(Debug, Clone, Default)]
pub struct ThinkingContext {
    /// Category hint from L1/L2 matching (if any)
    pub category_hint: Option<TaskCategory>,
    /// Whether to bias toward execution (vs conversation)
    pub bias_execute: bool,
    /// Layer that provided the hint (if any)
    pub hint_layer: Option<IntentLayer>,
    /// Processing time in microseconds
    pub latency_us: u64,
}

/// Intent Router for Agent Loop
///
/// Provides fast L0-L2 routing to determine if a request can be
/// directly executed or needs LLM thinking.
pub struct IntentRouter {
    decider: ExecutionIntentDecider,
}

impl IntentRouter {
    /// Create a new intent router with default config
    pub fn new() -> Self {
        Self {
            decider: ExecutionIntentDecider::new(),
        }
    }

    /// Create with custom config
    pub fn with_config(config: DeciderConfig) -> Self {
        Self {
            decider: ExecutionIntentDecider::with_config(config),
        }
    }

    /// Set the command parser for dynamic command resolution
    pub fn with_command_parser(mut self, parser: Arc<CommandParser>) -> Self {
        self.decider = self.decider.with_command_parser(parser);
        self
    }

    /// Update the command parser
    pub fn set_command_parser(&mut self, parser: Arc<CommandParser>) {
        self.decider.set_command_parser(parser);
    }

    /// Route user input through L0-L2 layers
    ///
    /// Returns DirectRoute if request can be handled without LLM thinking,
    /// otherwise returns NeedsThinking with optional category hints.
    pub fn route(&self, input: &str, context: Option<&ContextSignals>) -> RouteResult {
        let result = self.decider.decide(input, context);

        match result.mode {
            // L0: Direct tool/skill/mcp/custom execution
            ExecutionMode::DirectTool(tool) => RouteResult::DirectRoute(DirectRouteInfo {
                mode: DirectMode::Tool(tool),
                layer: result.metadata.layer,
                latency_us: result.metadata.latency_us,
            }),
            ExecutionMode::Skill(skill) => RouteResult::DirectRoute(DirectRouteInfo {
                mode: DirectMode::Skill(skill),
                layer: result.metadata.layer,
                latency_us: result.metadata.latency_us,
            }),
            ExecutionMode::Mcp(mcp) => RouteResult::DirectRoute(DirectRouteInfo {
                mode: DirectMode::Mcp(mcp),
                layer: result.metadata.layer,
                latency_us: result.metadata.latency_us,
            }),
            ExecutionMode::Custom(custom) => RouteResult::DirectRoute(DirectRouteInfo {
                mode: DirectMode::Custom(custom),
                layer: result.metadata.layer,
                latency_us: result.metadata.latency_us,
            }),

            // L1/L2: Category hint for LLM thinking
            ExecutionMode::Execute(category) => {
                let is_pattern_or_context = matches!(
                    result.metadata.layer,
                    IntentLayer::PatternMatch | IntentLayer::ContextSignal
                );

                RouteResult::NeedsThinking(ThinkingContext {
                    category_hint: if is_pattern_or_context {
                        Some(category)
                    } else {
                        None
                    },
                    bias_execute: true,
                    hint_layer: if is_pattern_or_context {
                        Some(result.metadata.layer)
                    } else {
                        None
                    },
                    latency_us: result.metadata.latency_us,
                })
            }

            // L4: Pure conversation or default - let LLM decide
            ExecutionMode::Converse => RouteResult::NeedsThinking(ThinkingContext {
                category_hint: None,
                bias_execute: false,
                hint_layer: None,
                latency_us: result.metadata.latency_us,
            }),
        }
    }

    /// Check if input is a slash command (L0 only)
    pub fn is_slash_command(&self, input: &str) -> bool {
        input.trim().starts_with('/')
    }
}

impl Default for IntentRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slash_command_direct_route() {
        let router = IntentRouter::new();

        let result = router.route("/screenshot", None);
        assert!(matches!(result, RouteResult::DirectRoute(_)));

        if let RouteResult::DirectRoute(info) = result {
            assert!(matches!(info.mode, DirectMode::Tool(_)));
            assert_eq!(info.layer, IntentLayer::SlashCommand);
        }
    }

    #[test]
    fn test_pattern_match_hint() {
        let router = IntentRouter::new();

        // This should match the FileOrganize pattern
        let result = router.route("整理下载文件夹里的文件", None);

        if let RouteResult::NeedsThinking(ctx) = result {
            assert!(ctx.category_hint.is_some());
            assert_eq!(ctx.category_hint, Some(TaskCategory::FileOrganize));
            assert_eq!(ctx.hint_layer, Some(IntentLayer::PatternMatch));
        } else {
            panic!("Expected NeedsThinking result");
        }
    }

    #[test]
    fn test_conversation_no_hint() {
        let router = IntentRouter::new();

        // This should match conversation pattern
        let result = router.route("什么是机器学习？", None);

        if let RouteResult::NeedsThinking(ctx) = result {
            // Conversation mode - no execute bias
            assert!(!ctx.bias_execute);
        } else {
            panic!("Expected NeedsThinking result for question");
        }
    }

    #[test]
    fn test_default_fallback() {
        let router = IntentRouter::new();

        // Ambiguous input - should fallback
        let result = router.route("hello", None);

        assert!(matches!(result, RouteResult::NeedsThinking(_)));
    }
}
