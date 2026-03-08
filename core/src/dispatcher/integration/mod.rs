//! Dispatcher Integration - Confirmation Handler for Tool Execution
//!
//! This module provides the integration layer for tool execution confirmation.
//! All routing is now handled by rig-core agents directly.
//!
//! # Integration Flow
//!
//! ```text
//! rig-core Agent Tool Call
//!      ↓
//! ┌─────────────────────────────────────────────────────────────────┐
//! │              DispatcherIntegration.handle_confirmation()         │
//! │                                                                  │
//! │  Check if confirmation needed (based on confidence threshold)    │
//! │      └→ on_confirmation_needed(request)                          │
//! │      └→ handle_result(result) → Execute/Cancel/Timeout           │
//! └─────────────────────────────────────────────────────────────────┘
//!      ↓
//! Execute Tool / Cancel / Error
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::dispatcher::{DispatcherIntegration, DispatcherConfig};
//!
//! let dispatcher = DispatcherIntegration::new(config);
//!
//! // Check if confirmation is needed
//! if dispatcher.needs_confirmation(&tool, confidence) {
//!     let result = dispatcher.handle_unified_confirmation_async(
//!         tool, confidence, params, reason, routing_layer, &event_handler
//!     )?;
//! }
//! ```

mod action;
mod config;
mod handler;
mod result;
mod thresholds;

// Re-exports
pub use action::DispatcherAction;
pub use config::DispatcherConfig;
pub use handler::DispatcherIntegration;
pub use result::DispatcherResult;
pub use thresholds::{ConfidenceAction, ConfidenceThresholds};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::{ConfirmationConfig, RoutingLayer, ToolSource, UnifiedTool};
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
                "native:web_fetch",
                "web_fetch",
                "Fetch web page content",
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

        let result =
            DispatcherResult::execute(tool.clone(), params, RoutingLayer::L3Inference, 0.9);

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
        let result = DispatcherResult::general_chat().with_reason("Custom reason");

        assert_eq!(result.reason, Some("Custom reason".to_string()));
    }

    #[test]
    fn test_dispatcher_result_with_confirmation() {
        let tool = create_test_tools().remove(0);
        let result = DispatcherResult::execute(tool, json!({}), RoutingLayer::L3Inference, 0.5)
            .with_confirmation();

        assert!(result.confirmation_shown);
    }

    #[test]
    fn test_dispatcher_integration_default() {
        let integration = DispatcherIntegration::default();

        assert!(integration.is_enabled());
    }

    #[test]
    fn test_dispatcher_integration_disabled() {
        let config = DispatcherConfig {
            enabled: false,
            ..Default::default()
        };
        let integration = DispatcherIntegration::new(config);

        assert!(!integration.is_enabled());
    }

    // ==========================================================================
    // Phase 9.2: Integration Tests
    // ==========================================================================

    /// Test 9.2.2: Confirmation trigger at threshold
    #[test]
    fn test_confirmation_trigger_at_threshold() {
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
        let integration = DispatcherIntegration::new(config);

        // Confidence 0.6 should trigger confirmation (< 0.7)
        let tool = create_test_tools().remove(0);
        assert!(integration.needs_confirmation(&tool, 0.6));

        // Confidence at threshold (0.7) should NOT trigger
        assert!(!integration.needs_confirmation(&tool, 0.7));

        // Confidence above threshold (0.8) should NOT trigger
        assert!(!integration.needs_confirmation(&tool, 0.8));
    }

    /// Test 9.2.3: Confirmation skip above threshold
    #[test]
    fn test_confirmation_skip_above_threshold() {
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
        let integration = DispatcherIntegration::new(config);
        let tool = create_test_tools().remove(0);

        // High confidence should skip confirmation
        assert!(!integration.needs_confirmation(&tool, 0.9));
        assert!(!integration.needs_confirmation(&tool, 1.0));
        assert!(!integration.needs_confirmation(&tool, 0.5)); // Equal to threshold

        // Low confidence should trigger
        assert!(integration.needs_confirmation(&tool, 0.4));
        assert!(integration.needs_confirmation(&tool, 0.3));
    }

    /// Test 9.2.3: Native tools should skip confirmation when configured
    #[test]
    fn test_confirmation_skip_native_tools() {
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
        let integration = DispatcherIntegration::new(config);

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

        // Native tool should be skipped (even with low confidence)
        assert!(!integration.needs_confirmation(&native_tool, 0.5));

        // MCP tool should NOT be skipped
        assert!(integration.needs_confirmation(&mcp_tool, 0.5));
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

    // ==========================================================================
    // Phase 6: Async Confirmation Tests
    // ==========================================================================

    /// Test DispatcherAction::PendingConfirmation variant
    #[test]
    fn test_dispatcher_action_pending() {
        let pending = DispatcherAction::PendingConfirmation("test-id-123".to_string());

        assert!(pending.is_pending());
        assert!(!pending.should_execute_tool());
        assert!(!pending.should_chat());
        assert!(!pending.is_error());
        assert_eq!(pending.confirmation_id(), Some("test-id-123"));
    }

    /// Test DispatcherResult::pending constructor
    #[test]
    fn test_dispatcher_result_pending() {
        let tool = create_test_tools().remove(0);
        let result = DispatcherResult::pending(
            "conf-123".to_string(),
            tool,
            json!({"query": "test search"}),
            RoutingLayer::L3Inference,
            0.6,
        );

        assert!(result.action.is_pending());
        assert_eq!(result.action.confirmation_id(), Some("conf-123"));
        assert!(result.tool.is_some());
        assert_eq!(result.tool.unwrap().name, "search");
        assert!(result.parameters.is_some());
        assert_eq!(result.confidence, 0.6);
        assert_eq!(result.routing_layer, RoutingLayer::L3Inference);
        assert!(result.confirmation_shown);
    }

    /// Test async confirmation handler integration
    #[test]
    fn test_async_confirmation_handler_integration() {
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
        let integration = DispatcherIntegration::new(config);

        // Verify async confirmation handler is initialized
        assert!(integration.async_confirmation().threshold() > 0.0);
        assert_eq!(integration.pending_confirmation_count(), 0);
    }

    /// Test resume_with_decision flow
    #[test]
    fn test_resume_with_decision_not_found() {
        use crate::dispatcher::UserConfirmationDecision;

        let integration = DispatcherIntegration::default();

        // Try to resume with non-existent confirmation
        let result =
            integration.resume_with_decision("non-existent-id", UserConfirmationDecision::Execute);

        assert!(result.is_ok());
        let dispatcher_result = result.unwrap();
        // Should return cancelled/general_chat since confirmation not found
        assert!(!dispatcher_result.action.should_execute_tool());
    }

    /// Test get_pending_confirmation with non-existent ID
    #[test]
    fn test_get_pending_confirmation_not_found() {
        let integration = DispatcherIntegration::default();

        let pending = integration.get_pending_confirmation("non-existent");
        assert!(pending.is_none());
    }

    /// Test is_confirmation_pending
    #[test]
    fn test_is_confirmation_pending() {
        let integration = DispatcherIntegration::default();

        assert!(!integration.is_confirmation_pending("non-existent"));
    }

    // =========================================================================
    // ConfidenceThresholds Tests
    // =========================================================================

    #[test]
    fn test_confidence_thresholds_default() {
        let thresholds = ConfidenceThresholds::default();
        assert!((thresholds.no_match - 0.3).abs() < 0.001);
        assert!((thresholds.requires_confirmation - 0.7).abs() < 0.001);
        assert!((thresholds.auto_execute - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_confidence_thresholds_validate_pass() {
        let thresholds = ConfidenceThresholds::new(0.3, 0.7, 0.9);
        assert!(thresholds.validate().is_ok());

        // Edge case: requires_confirmation == auto_execute
        let thresholds = ConfidenceThresholds::new(0.3, 0.9, 0.9);
        assert!(thresholds.validate().is_ok());
    }

    #[test]
    fn test_confidence_thresholds_validate_fail_reversed() {
        // no_match > requires_confirmation
        let thresholds = ConfidenceThresholds::new(0.8, 0.5, 0.9);
        let result = thresholds.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be less than"));
    }

    #[test]
    fn test_confidence_thresholds_validate_fail_out_of_range() {
        // no_match < 0
        let thresholds = ConfidenceThresholds::new(-0.1, 0.7, 0.9);
        assert!(thresholds.validate().is_err());

        // auto_execute > 1
        let thresholds = ConfidenceThresholds::new(0.3, 0.7, 1.1);
        assert!(thresholds.validate().is_err());
    }

    #[test]
    fn test_confidence_thresholds_classify_no_match() {
        let thresholds = ConfidenceThresholds::default();
        assert_eq!(thresholds.classify(0.0), ConfidenceAction::NoMatch);
        assert_eq!(thresholds.classify(0.2), ConfidenceAction::NoMatch);
        assert_eq!(thresholds.classify(0.29), ConfidenceAction::NoMatch);
    }

    #[test]
    fn test_confidence_thresholds_classify_requires_confirmation() {
        let thresholds = ConfidenceThresholds::default();
        assert_eq!(
            thresholds.classify(0.3),
            ConfidenceAction::RequiresConfirmation
        );
        assert_eq!(
            thresholds.classify(0.5),
            ConfidenceAction::RequiresConfirmation
        );
        assert_eq!(
            thresholds.classify(0.69),
            ConfidenceAction::RequiresConfirmation
        );
    }

    #[test]
    fn test_confidence_thresholds_classify_optional_confirmation() {
        let thresholds = ConfidenceThresholds::default();
        assert_eq!(
            thresholds.classify(0.7),
            ConfidenceAction::OptionalConfirmation
        );
        assert_eq!(
            thresholds.classify(0.8),
            ConfidenceAction::OptionalConfirmation
        );
        assert_eq!(
            thresholds.classify(0.89),
            ConfidenceAction::OptionalConfirmation
        );
    }

    #[test]
    fn test_confidence_thresholds_classify_auto_execute() {
        let thresholds = ConfidenceThresholds::default();
        assert_eq!(thresholds.classify(0.9), ConfidenceAction::AutoExecute);
        assert_eq!(thresholds.classify(0.95), ConfidenceAction::AutoExecute);
        assert_eq!(thresholds.classify(1.0), ConfidenceAction::AutoExecute);
    }

    #[test]
    fn test_confidence_thresholds_needs_confirmation() {
        let thresholds = ConfidenceThresholds::default();

        // When confirmation is enabled
        assert!(!thresholds.needs_confirmation(0.2, true)); // NoMatch - no confirmation
        assert!(thresholds.needs_confirmation(0.5, true)); // RequiresConfirmation - yes
        assert!(!thresholds.needs_confirmation(0.8, true)); // OptionalConfirmation - no
        assert!(!thresholds.needs_confirmation(0.95, true)); // AutoExecute - no

        // When confirmation is disabled
        assert!(!thresholds.needs_confirmation(0.5, false)); // Always false
    }

    #[test]
    fn test_dispatcher_config_confidence_thresholds() {
        let config = DispatcherConfig {
            enabled: true,
            l3_enabled: true,
            l3_timeout_ms: 5000,
            l3_confidence_threshold: 0.4,
            confirmation: ConfirmationConfig {
                enabled: true,
                threshold: 0.8,
                timeout_ms: 30000,
                show_parameters: true,
                skip_native_tools: false,
            },
        };

        let thresholds = config.confidence_thresholds();
        assert!((thresholds.no_match - 0.4).abs() < 0.001);
        assert!((thresholds.requires_confirmation - 0.8).abs() < 0.001);
        assert!((thresholds.auto_execute - 0.9).abs() < 0.001);
    }
}
