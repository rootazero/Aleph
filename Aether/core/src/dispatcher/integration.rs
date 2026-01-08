//! Dispatcher Integration - Bridge between Dispatcher Layer and AetherCore
//!
//! This module provides the integration layer that connects the Dispatcher
//! components (L3Router, ToolConfirmation) with the AetherCore processing flow.
//!
//! # Integration Flow
//!
//! ```text
//! User Input
//!      ↓
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    AetherCore.process_input()                    │
//! │                              ↓                                   │
//! │  ┌─────────────────────────────────────────────────────────────┐│
//! │  │ DispatcherIntegration.route_with_confirmation()             ││
//! │  │                                                              ││
//! │  │  1. L1 Regex Match (Router.match_rules)                     ││
//! │  │      └→ confidence = 1.0, skip confirmation                 ││
//! │  │                                                              ││
//! │  │  2. L2 Semantic Match (SemanticMatcher.match_input)         ││
//! │  │      └→ confidence varies, may need confirmation            ││
//! │  │                                                              ││
//! │  │  3. L3 AI Inference (L3Router.route)                        ││
//! │  │      └→ confidence varies, may need confirmation            ││
//! │  │                                                              ││
//! │  │  4. Confirmation Check (if confidence < threshold)          ││
//! │  │      └→ on_clarification_needed(request)                    ││
//! │  │      └→ handle_result(result) → Execute/Cancel/Timeout      ││
//! │  └─────────────────────────────────────────────────────────────┘│
//! │                              ↓                                   │
//! │              DispatcherResult (tool, params, action)            │
//! └─────────────────────────────────────────────────────────────────┘
//!      ↓
//! Execute Capability / General Chat / Error
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::{DispatcherIntegration, DispatcherConfig};
//!
//! // In AetherCore initialization
//! let dispatcher = DispatcherIntegration::new(config, provider.clone());
//!
//! // In process_input
//! let result = dispatcher.route_with_confirmation(
//!     &input,
//!     &tools,
//!     &event_handler,
//!     context,
//! ).await?;
//!
//! match result.action {
//!     DispatcherAction::ExecuteTool => {
//!         // Execute the tool with result.tool and result.parameters
//!     }
//!     DispatcherAction::GeneralChat => {
//!         // Fall back to general AI conversation
//!     }
//!     DispatcherAction::Error(msg) => {
//!         // Handle error
//!     }
//! }
//! ```

use crate::clarification::ClarificationResult;
use crate::dispatcher::{
    ConfirmationAction, ConfirmationConfig, L3Router, L3RoutingOptions, L3RoutingResponse,
    RoutingLayer, ToolConfirmation, UnifiedTool,
};
use crate::error::Result;
use crate::event_handler::AetherEventHandler;
use crate::providers::AiProvider;
use crate::semantic::context::ConversationContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the Dispatcher integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatcherConfig {
    /// Whether the dispatcher is enabled
    pub enabled: bool,

    /// L3 routing configuration
    pub l3_enabled: bool,
    pub l3_timeout_ms: u64,
    pub l3_confidence_threshold: f32,

    /// Confirmation configuration
    pub confirmation: ConfirmationConfig,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            l3_enabled: true,
            l3_timeout_ms: 5000,
            l3_confidence_threshold: 0.3,
            confirmation: ConfirmationConfig::default(),
        }
    }
}

impl DispatcherConfig {
    /// Create a minimal config (L3 disabled, no confirmation)
    pub fn minimal() -> Self {
        Self {
            enabled: true,
            l3_enabled: false,
            l3_timeout_ms: 5000,
            l3_confidence_threshold: 0.3,
            confirmation: ConfirmationConfig::disabled(),
        }
    }

    /// Create a full config with all features
    pub fn full() -> Self {
        Self::default()
    }
}

// =============================================================================
// Dispatcher Action
// =============================================================================

/// Action to take after dispatcher routing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatcherAction {
    /// Execute a tool with the given parameters
    ExecuteTool,

    /// No tool matched - proceed with general chat
    GeneralChat,

    /// User cancelled tool execution - fall back to chat
    Cancelled,

    /// Error occurred during routing/confirmation
    Error(String),
}

impl DispatcherAction {
    /// Check if this action should execute a tool
    pub fn should_execute_tool(&self) -> bool {
        matches!(self, DispatcherAction::ExecuteTool)
    }

    /// Check if this action should fall back to chat
    pub fn should_chat(&self) -> bool {
        matches!(
            self,
            DispatcherAction::GeneralChat | DispatcherAction::Cancelled
        )
    }

    /// Check if this action is an error
    pub fn is_error(&self) -> bool {
        matches!(self, DispatcherAction::Error(_))
    }
}

// =============================================================================
// Dispatcher Result
// =============================================================================

/// Result of dispatcher routing
#[derive(Debug, Clone)]
pub struct DispatcherResult {
    /// Action to take
    pub action: DispatcherAction,

    /// Matched tool (if any)
    pub tool: Option<UnifiedTool>,

    /// Extracted parameters (if any)
    pub parameters: Option<serde_json::Value>,

    /// Routing layer that produced this result
    pub routing_layer: RoutingLayer,

    /// Routing confidence score
    pub confidence: f32,

    /// Routing reason/explanation
    pub reason: Option<String>,

    /// Whether confirmation was shown
    pub confirmation_shown: bool,
}

impl DispatcherResult {
    /// Create a result for tool execution
    pub fn execute(
        tool: UnifiedTool,
        parameters: serde_json::Value,
        layer: RoutingLayer,
        confidence: f32,
    ) -> Self {
        Self {
            action: DispatcherAction::ExecuteTool,
            tool: Some(tool),
            parameters: Some(parameters),
            routing_layer: layer,
            confidence,
            reason: None,
            confirmation_shown: false,
        }
    }

    /// Create a result for general chat
    pub fn general_chat() -> Self {
        Self {
            action: DispatcherAction::GeneralChat,
            tool: None,
            parameters: None,
            routing_layer: RoutingLayer::Default,
            confidence: 0.0,
            reason: Some("No tool matched".to_string()),
            confirmation_shown: false,
        }
    }

    /// Create a result for cancelled action
    pub fn cancelled() -> Self {
        Self {
            action: DispatcherAction::Cancelled,
            tool: None,
            parameters: None,
            routing_layer: RoutingLayer::Default,
            confidence: 0.0,
            reason: Some("User cancelled".to_string()),
            confirmation_shown: true,
        }
    }

    /// Create an error result
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            action: DispatcherAction::Error(message.into()),
            tool: None,
            parameters: None,
            routing_layer: RoutingLayer::Default,
            confidence: 0.0,
            reason: None,
            confirmation_shown: false,
        }
    }

    /// Set the reason
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Mark as confirmation shown
    pub fn with_confirmation(mut self) -> Self {
        self.confirmation_shown = true;
        self
    }
}

// =============================================================================
// Dispatcher Integration
// =============================================================================

/// Integration layer between Dispatcher and AetherCore
pub struct DispatcherIntegration {
    /// Configuration
    config: DispatcherConfig,

    /// L3 Router (optional, created lazily)
    l3_router: Option<L3Router>,

    /// Tool confirmation handler
    confirmation: ToolConfirmation,
}

impl DispatcherIntegration {
    /// Create a new dispatcher integration
    pub fn new(config: DispatcherConfig, provider: Option<Arc<dyn AiProvider>>) -> Self {
        let l3_router = if config.l3_enabled {
            provider.map(|p| {
                L3Router::new(p)
                    .with_timeout(Duration::from_millis(config.l3_timeout_ms))
                    .with_confidence_threshold(config.l3_confidence_threshold)
            })
        } else {
            None
        };

        let confirmation = ToolConfirmation::new(config.confirmation.clone());

        Self {
            config,
            l3_router,
            confirmation,
        }
    }

    /// Create with default configuration
    pub fn with_defaults(provider: Arc<dyn AiProvider>) -> Self {
        Self::new(DispatcherConfig::default(), Some(provider))
    }

    /// Check if the dispatcher is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Check if L3 routing is available
    pub fn has_l3_router(&self) -> bool {
        self.l3_router.is_some()
    }

    /// Route input and handle confirmation if needed
    ///
    /// This is the main entry point for dispatcher integration.
    ///
    /// # Arguments
    ///
    /// * `input` - User input to route
    /// * `tools` - Available tools
    /// * `event_handler` - Event handler for confirmation callbacks
    /// * `conversation` - Optional conversation context
    ///
    /// # Returns
    ///
    /// A `DispatcherResult` indicating the action to take
    pub async fn route_with_confirmation(
        &self,
        input: &str,
        tools: &[UnifiedTool],
        event_handler: &dyn AetherEventHandler,
        conversation: Option<&ConversationContext>,
    ) -> Result<DispatcherResult> {
        if !self.config.enabled {
            debug!("Dispatcher disabled, falling back to general chat");
            return Ok(DispatcherResult::general_chat());
        }

        // Try L3 routing if available
        let l3_result = if let Some(ref router) = self.l3_router {
            let context = conversation.and_then(|c| c.build_l3_context_summary(3));
            let entity_hints = conversation
                .map(|c| c.extract_entity_hints())
                .unwrap_or_default();

            let options = L3RoutingOptions {
                conversation_context: context,
                entity_hints,
                timeout: None,
                confidence_threshold: None,
            };

            router.route_with_options(input, tools, options).await?
        } else {
            None
        };

        // Process L3 result
        if let Some(response) = l3_result {
            self.process_l3_result(response, tools, event_handler).await
        } else {
            // No L3 result - fall back to general chat
            debug!("No L3 routing result, falling back to general chat");
            Ok(DispatcherResult::general_chat())
        }
    }

    /// Process L3 routing result with optional confirmation
    async fn process_l3_result(
        &self,
        response: L3RoutingResponse,
        tools: &[UnifiedTool],
        event_handler: &dyn AetherEventHandler,
    ) -> Result<DispatcherResult> {
        // Check if we have a match
        if !response.has_match() {
            return Ok(DispatcherResult::general_chat()
                .with_reason(response.reason.clone()));
        }

        // Find the matched tool
        let tool_name = response.tool.as_ref().unwrap();
        let tool = tools
            .iter()
            .find(|t| &t.name == tool_name)
            .cloned();

        let tool = match tool {
            Some(t) => t,
            None => {
                warn!(tool_name = %tool_name, "L3 matched non-existent tool");
                return Ok(DispatcherResult::general_chat()
                    .with_reason(format!("Tool '{}' not found", tool_name)));
            }
        };

        info!(
            tool = %tool.name,
            confidence = response.confidence,
            "L3 routing matched tool"
        );

        // Check if confirmation is needed
        let needs_confirmation = self.confirmation.should_confirm(response.confidence)
            && !self.confirmation.should_skip_for_tool(&tool);

        if needs_confirmation {
            self.handle_confirmation(tool, response, event_handler)
        } else {
            // High confidence - execute directly
            Ok(DispatcherResult::execute(
                tool,
                response.parameters.clone(),
                RoutingLayer::L3Inference,
                response.confidence,
            )
            .with_reason(response.reason))
        }
    }

    /// Handle the confirmation flow
    fn handle_confirmation(
        &self,
        tool: UnifiedTool,
        response: L3RoutingResponse,
        event_handler: &dyn AetherEventHandler,
    ) -> Result<DispatcherResult> {
        info!(
            tool = %tool.name,
            confidence = response.confidence,
            threshold = self.confirmation.threshold(),
            "Showing confirmation for low-confidence match"
        );

        // Build and show confirmation request
        let request = self.confirmation.build_request(
            &tool,
            Some(&response.parameters),
            Some(&response.reason),
        );

        // This is a blocking call - waits for user response
        let result: ClarificationResult = event_handler.on_clarification_needed(request);

        // Handle the result
        let action = self.confirmation.handle_result(result);

        match action {
            ConfirmationAction::Execute => {
                info!("User confirmed tool execution");
                Ok(DispatcherResult::execute(
                    tool,
                    response.parameters,
                    RoutingLayer::L3Inference,
                    response.confidence,
                )
                .with_confirmation()
                .with_reason("User confirmed execution"))
            }
            ConfirmationAction::Cancel => {
                info!("User cancelled tool execution");
                Ok(DispatcherResult::cancelled())
            }
            ConfirmationAction::Timeout => {
                warn!("Confirmation timed out");
                Ok(DispatcherResult::error("Confirmation timed out"))
            }
            ConfirmationAction::EditParameters => {
                // For now, treat edit as cancel (parameter editing not implemented)
                info!("User requested parameter edit (not implemented, treating as cancel)");
                Ok(DispatcherResult::cancelled()
                    .with_reason("Parameter editing not yet implemented"))
            }
        }
    }

    /// Get the confirmation configuration
    pub fn confirmation_config(&self) -> &ConfirmationConfig {
        &self.config.confirmation
    }

    /// Get the dispatcher configuration
    pub fn config(&self) -> &DispatcherConfig {
        &self.config
    }
}

impl Default for DispatcherIntegration {
    fn default() -> Self {
        Self::new(DispatcherConfig::default(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;
    use serde_json::json;

    fn create_test_tools() -> Vec<UnifiedTool> {
        vec![
            UnifiedTool::new(
                "native:search",
                "search",
                "Search the web",
                ToolSource::Native,
            ),
            UnifiedTool::new(
                "native:video",
                "video",
                "Analyze video",
                ToolSource::Native,
            ),
        ]
    }

    #[test]
    fn test_dispatcher_config_default() {
        let config = DispatcherConfig::default();

        assert!(config.enabled);
        assert!(config.l3_enabled);
        assert_eq!(config.l3_timeout_ms, 5000);
        assert!(config.confirmation.enabled);
    }

    #[test]
    fn test_dispatcher_config_minimal() {
        let config = DispatcherConfig::minimal();

        assert!(config.enabled);
        assert!(!config.l3_enabled);
        assert!(!config.confirmation.enabled);
    }

    #[test]
    fn test_dispatcher_action() {
        assert!(DispatcherAction::ExecuteTool.should_execute_tool());
        assert!(!DispatcherAction::ExecuteTool.should_chat());

        assert!(!DispatcherAction::GeneralChat.should_execute_tool());
        assert!(DispatcherAction::GeneralChat.should_chat());

        assert!(!DispatcherAction::Cancelled.should_execute_tool());
        assert!(DispatcherAction::Cancelled.should_chat());

        assert!(DispatcherAction::Error("test".into()).is_error());
    }

    #[test]
    fn test_dispatcher_result_execute() {
        let tool = create_test_tools().remove(0);
        let params = json!({"query": "test"});

        let result = DispatcherResult::execute(
            tool.clone(),
            params,
            RoutingLayer::L3Inference,
            0.9,
        );

        assert!(result.action.should_execute_tool());
        assert_eq!(result.tool.unwrap().name, "search");
        assert_eq!(result.confidence, 0.9);
        assert!(!result.confirmation_shown);
    }

    #[test]
    fn test_dispatcher_result_general_chat() {
        let result = DispatcherResult::general_chat();

        assert!(result.action.should_chat());
        assert!(result.tool.is_none());
        assert_eq!(result.routing_layer, RoutingLayer::Default);
    }

    #[test]
    fn test_dispatcher_result_cancelled() {
        let result = DispatcherResult::cancelled();

        assert!(result.action.should_chat());
        assert!(result.confirmation_shown);
    }

    #[test]
    fn test_dispatcher_result_error() {
        let result = DispatcherResult::error("Test error");

        assert!(result.action.is_error());
        match result.action {
            DispatcherAction::Error(msg) => assert_eq!(msg, "Test error"),
            _ => panic!("Expected Error action"),
        }
    }

    #[test]
    fn test_dispatcher_result_with_reason() {
        let result = DispatcherResult::general_chat()
            .with_reason("Custom reason");

        assert_eq!(result.reason, Some("Custom reason".to_string()));
    }

    #[test]
    fn test_dispatcher_result_with_confirmation() {
        let tool = create_test_tools().remove(0);
        let result = DispatcherResult::execute(
            tool,
            json!({}),
            RoutingLayer::L3Inference,
            0.5,
        )
        .with_confirmation();

        assert!(result.confirmation_shown);
    }

    #[test]
    fn test_dispatcher_integration_default() {
        let integration = DispatcherIntegration::default();

        assert!(integration.is_enabled());
        assert!(!integration.has_l3_router()); // No provider provided
    }

    #[test]
    fn test_dispatcher_integration_disabled() {
        let config = DispatcherConfig {
            enabled: false,
            ..Default::default()
        };
        let integration = DispatcherIntegration::new(config, None);

        assert!(!integration.is_enabled());
    }

    // ==========================================================================
    // Phase 9.2: Integration Tests
    // ==========================================================================

    /// Test 9.2.2: Confirmation trigger at threshold
    #[test]
    fn test_confirmation_trigger_at_threshold() {
        use crate::dispatcher::ConfirmationConfig;

        // Set threshold to 0.7
        let config = DispatcherConfig {
            enabled: true,
            l3_enabled: true,
            l3_timeout_ms: 5000,
            l3_confidence_threshold: 0.3,
            confirmation: ConfirmationConfig {
                enabled: true,
                threshold: 0.7,
                timeout_ms: 30000,
                show_parameters: true,
                skip_native_tools: false,
            },
        };
        let integration = DispatcherIntegration::new(config, None);

        // Confidence 0.6 should trigger confirmation (< 0.7)
        assert!(integration.confirmation.should_confirm(0.6));

        // Confidence at threshold (0.7) should NOT trigger
        assert!(!integration.confirmation.should_confirm(0.7));

        // Confidence above threshold (0.8) should NOT trigger
        assert!(!integration.confirmation.should_confirm(0.8));
    }

    /// Test 9.2.3: Confirmation skip above threshold
    #[test]
    fn test_confirmation_skip_above_threshold() {
        use crate::dispatcher::ConfirmationConfig;

        let config = DispatcherConfig {
            enabled: true,
            l3_enabled: true,
            l3_timeout_ms: 5000,
            l3_confidence_threshold: 0.3,
            confirmation: ConfirmationConfig {
                enabled: true,
                threshold: 0.5,
                timeout_ms: 30000,
                show_parameters: true,
                skip_native_tools: false,
            },
        };
        let integration = DispatcherIntegration::new(config, None);

        // High confidence should skip confirmation
        assert!(!integration.confirmation.should_confirm(0.9));
        assert!(!integration.confirmation.should_confirm(1.0));
        assert!(!integration.confirmation.should_confirm(0.5)); // Equal to threshold

        // Low confidence should trigger
        assert!(integration.confirmation.should_confirm(0.4));
        assert!(integration.confirmation.should_confirm(0.3));
    }

    /// Test 9.2.3: Native tools should skip confirmation when configured
    #[test]
    fn test_confirmation_skip_native_tools() {
        use crate::dispatcher::ConfirmationConfig;

        let config = DispatcherConfig {
            enabled: true,
            l3_enabled: true,
            l3_timeout_ms: 5000,
            l3_confidence_threshold: 0.3,
            confirmation: ConfirmationConfig {
                enabled: true,
                threshold: 0.7,
                timeout_ms: 30000,
                show_parameters: true,
                skip_native_tools: true, // Skip native tools
            },
        };
        let integration = DispatcherIntegration::new(config, None);

        let native_tool = UnifiedTool::new(
            "native:search",
            "search",
            "Search the web",
            ToolSource::Native,
        );

        let mcp_tool = UnifiedTool::new(
            "mcp:github:git_status",
            "git_status",
            "Get git status",
            ToolSource::Mcp {
                server: "github".into(),
            },
        );

        // Native tool should be skipped
        assert!(integration.confirmation.should_skip_for_tool(&native_tool));

        // MCP tool should NOT be skipped
        assert!(!integration.confirmation.should_skip_for_tool(&mcp_tool));
    }

    /// Test 9.2.4: Dispatcher config validation
    #[test]
    fn test_dispatcher_config_validation() {
        // Valid config
        let config = DispatcherConfig::default();
        assert!(config.enabled);
        assert!(config.l3_enabled);
        assert_eq!(config.l3_timeout_ms, 5000);

        // Minimal config
        let minimal = DispatcherConfig::minimal();
        assert!(minimal.enabled);
        assert!(!minimal.l3_enabled);
        assert!(!minimal.confirmation.enabled);
    }

    /// Test routing layer confidence defaults
    #[test]
    fn test_routing_layer_confidence() {
        assert_eq!(RoutingLayer::L1Rule.default_confidence(), 1.0);
        assert_eq!(RoutingLayer::L2Semantic.default_confidence(), 0.7);
        assert_eq!(RoutingLayer::L3Inference.default_confidence(), 0.5);
        assert_eq!(RoutingLayer::Default.default_confidence(), 0.0);
    }

    /// Test routing layer latency hints
    #[test]
    fn test_routing_layer_latency() {
        assert_eq!(RoutingLayer::L1Rule.latency_hint(), "<10ms");
        assert_eq!(RoutingLayer::L2Semantic.latency_hint(), "200-500ms");
        assert_eq!(RoutingLayer::L3Inference.latency_hint(), ">1s");
        assert_eq!(RoutingLayer::Default.latency_hint(), "0ms");
    }

    /// Test dispatcher result combinations
    #[test]
    fn test_dispatcher_result_combinations() {
        let tools = create_test_tools();

        // L1 match: high confidence, no confirmation needed
        let l1_result = DispatcherResult::execute(
            tools[0].clone(),
            json!({"query": "test"}),
            RoutingLayer::L1Rule,
            1.0,
        );
        assert!(l1_result.action.should_execute_tool());
        assert!(!l1_result.confirmation_shown);
        assert_eq!(l1_result.confidence, 1.0);
        assert_eq!(l1_result.routing_layer, RoutingLayer::L1Rule);

        // L2 match: medium confidence
        let l2_result = DispatcherResult::execute(
            tools[0].clone(),
            json!({"query": "test"}),
            RoutingLayer::L2Semantic,
            0.75,
        );
        assert_eq!(l2_result.confidence, 0.75);
        assert_eq!(l2_result.routing_layer, RoutingLayer::L2Semantic);

        // L3 match: lower confidence, confirmation shown
        let l3_result = DispatcherResult::execute(
            tools[0].clone(),
            json!({"query": "test"}),
            RoutingLayer::L3Inference,
            0.6,
        )
        .with_confirmation();
        assert!(l3_result.confirmation_shown);
        assert_eq!(l3_result.confidence, 0.6);
        assert_eq!(l3_result.routing_layer, RoutingLayer::L3Inference);
    }
}
