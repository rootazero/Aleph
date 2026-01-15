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
//! use aethecore::dispatcher::{DispatcherIntegration, DispatcherConfig};
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

use crate::dispatcher::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationConfig, ConfirmationState,
    PendingConfirmationInfo, RoutingLayer, ToolConfirmation, UnifiedTool, UserConfirmationDecision,
};
use crate::error::Result;
use crate::event_handler::InternalEventHandler;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

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

    /// Get the confidence thresholds from this config
    pub fn confidence_thresholds(&self) -> ConfidenceThresholds {
        ConfidenceThresholds {
            no_match: self.l3_confidence_threshold,
            requires_confirmation: self.confirmation.threshold,
            auto_execute: 0.9,
        }
    }
}

// =============================================================================
// Confidence Thresholds
// =============================================================================

/// Action to take based on confidence level
///
/// The confidence score determines what action the dispatcher should take:
/// - Very low confidence: No tool match, fall back to general chat
/// - Low confidence: Tool match but requires user confirmation
/// - Medium confidence: Tool match with optional confirmation (based on config)
/// - High confidence: Auto-execute without confirmation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceAction {
    /// Confidence too low - no tool matched, fall back to general chat
    NoMatch,

    /// Tool matched but confidence is low - requires user confirmation
    RequiresConfirmation,

    /// Tool matched with medium confidence - confirmation is optional
    OptionalConfirmation,

    /// Tool matched with high confidence - auto-execute without confirmation
    AutoExecute,
}

/// Unified confidence threshold configuration
///
/// Provides a single source of truth for all confidence thresholds used
/// in the Dispatcher Layer. This eliminates scattered threshold definitions
/// and ensures consistent behavior across L1/L2/L3 routing.
///
/// # Threshold Ordering
///
/// The thresholds must be ordered: `no_match < requires_confirmation <= auto_execute`
///
/// # Default Values
///
/// - `no_match`: 0.3 - Below this, no tool is considered matched
/// - `requires_confirmation`: 0.7 - Below this, confirmation is required
/// - `auto_execute`: 0.9 - Above this, auto-execute without confirmation
///
/// # Confidence Ranges
///
/// ```text
/// 0.0 ─────────── no_match ─────────── requires_confirmation ─────────── auto_execute ─────────── 1.0
///      NoMatch              RequiresConfirmation              OptionalConfirmation       AutoExecute
/// ```
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::dispatcher::ConfidenceThresholds;
///
/// let thresholds = ConfidenceThresholds::default();
///
/// // Classify confidence scores
/// assert_eq!(thresholds.classify(0.2), ConfidenceAction::NoMatch);
/// assert_eq!(thresholds.classify(0.5), ConfidenceAction::RequiresConfirmation);
/// assert_eq!(thresholds.classify(0.8), ConfidenceAction::OptionalConfirmation);
/// assert_eq!(thresholds.classify(0.95), ConfidenceAction::AutoExecute);
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ConfidenceThresholds {
    /// Minimum confidence for a tool to be considered matched (default: 0.3)
    /// Below this threshold, the input falls back to general chat.
    pub no_match: f32,

    /// Confidence below which confirmation is always required (default: 0.7)
    /// Between `no_match` and this threshold, confirmation is mandatory.
    pub requires_confirmation: f32,

    /// Confidence above which auto-execute is allowed (default: 0.9)
    /// Above this threshold, tools execute without confirmation.
    pub auto_execute: f32,
}

impl Default for ConfidenceThresholds {
    fn default() -> Self {
        Self {
            no_match: 0.3,
            requires_confirmation: 0.7,
            auto_execute: 0.9,
        }
    }
}

impl ConfidenceThresholds {
    /// Create thresholds with custom values
    pub fn new(no_match: f32, requires_confirmation: f32, auto_execute: f32) -> Self {
        Self {
            no_match,
            requires_confirmation,
            auto_execute,
        }
    }

    /// Validate the threshold ordering
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Thresholds are valid
    /// * `Err(String)` - Validation error message
    ///
    /// # Validation Rules
    ///
    /// 1. All thresholds must be in range [0.0, 1.0]
    /// 2. no_match < requires_confirmation <= auto_execute
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Check range
        if self.no_match < 0.0 || self.no_match > 1.0 {
            return Err(format!(
                "no_match threshold must be in [0.0, 1.0], got {}",
                self.no_match
            ));
        }
        if self.requires_confirmation < 0.0 || self.requires_confirmation > 1.0 {
            return Err(format!(
                "requires_confirmation threshold must be in [0.0, 1.0], got {}",
                self.requires_confirmation
            ));
        }
        if self.auto_execute < 0.0 || self.auto_execute > 1.0 {
            return Err(format!(
                "auto_execute threshold must be in [0.0, 1.0], got {}",
                self.auto_execute
            ));
        }

        // Check ordering
        if self.no_match >= self.requires_confirmation {
            return Err(format!(
                "no_match ({}) must be less than requires_confirmation ({})",
                self.no_match, self.requires_confirmation
            ));
        }
        if self.requires_confirmation > self.auto_execute {
            return Err(format!(
                "requires_confirmation ({}) must not exceed auto_execute ({})",
                self.requires_confirmation, self.auto_execute
            ));
        }

        Ok(())
    }

    /// Classify a confidence score into an action
    ///
    /// # Arguments
    ///
    /// * `confidence` - The confidence score (0.0 to 1.0)
    ///
    /// # Returns
    ///
    /// The appropriate `ConfidenceAction` for the given confidence level.
    pub fn classify(&self, confidence: f32) -> ConfidenceAction {
        if confidence < self.no_match {
            ConfidenceAction::NoMatch
        } else if confidence < self.requires_confirmation {
            ConfidenceAction::RequiresConfirmation
        } else if confidence < self.auto_execute {
            ConfidenceAction::OptionalConfirmation
        } else {
            ConfidenceAction::AutoExecute
        }
    }

    /// Check if confirmation is needed for a given confidence
    ///
    /// This is a convenience method that returns true if the confidence
    /// falls in the RequiresConfirmation or OptionalConfirmation range.
    ///
    /// # Arguments
    ///
    /// * `confidence` - The confidence score
    /// * `confirmation_enabled` - Whether confirmation is enabled in config
    ///
    /// # Returns
    ///
    /// `true` if confirmation should be shown, `false` otherwise
    pub fn needs_confirmation(&self, confidence: f32, confirmation_enabled: bool) -> bool {
        if !confirmation_enabled {
            return false;
        }

        match self.classify(confidence) {
            ConfidenceAction::NoMatch => false, // No match = no confirmation (fall back to chat)
            ConfidenceAction::RequiresConfirmation => true,
            ConfidenceAction::OptionalConfirmation => false, // Optional = don't require
            ConfidenceAction::AutoExecute => false,
        }
    }
}

// =============================================================================
// Dispatcher Action
// =============================================================================

/// Action to take after dispatcher routing
#[derive(Debug, Clone, PartialEq)]
pub enum DispatcherAction {
    /// Execute a tool with the given parameters
    ExecuteTool,

    /// No tool matched - proceed with general chat
    GeneralChat,

    /// User cancelled tool execution - fall back to chat
    Cancelled,

    /// Waiting for user confirmation (async flow)
    /// Contains the confirmation_id to track the pending confirmation
    PendingConfirmation(String),

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

    /// Check if this action is pending confirmation
    pub fn is_pending(&self) -> bool {
        matches!(self, DispatcherAction::PendingConfirmation(_))
    }

    /// Get the confirmation ID if this is a pending confirmation
    pub fn confirmation_id(&self) -> Option<&str> {
        match self {
            DispatcherAction::PendingConfirmation(id) => Some(id),
            _ => None,
        }
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

    /// Create a pending confirmation result (async flow)
    pub fn pending(
        confirmation_id: String,
        tool: UnifiedTool,
        parameters: serde_json::Value,
        layer: RoutingLayer,
        confidence: f32,
    ) -> Self {
        Self {
            action: DispatcherAction::PendingConfirmation(confirmation_id),
            tool: Some(tool),
            parameters: Some(parameters),
            routing_layer: layer,
            confidence,
            reason: Some("Awaiting user confirmation".to_string()),
            confirmation_shown: true,
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

/// Integration layer for tool execution confirmation
pub struct DispatcherIntegration {
    /// Configuration
    config: DispatcherConfig,

    /// Tool confirmation handler (legacy blocking)
    confirmation: ToolConfirmation,

    /// Async confirmation handler (non-blocking)
    async_confirmation: Arc<AsyncConfirmationHandler>,
}

impl DispatcherIntegration {
    /// Create a new dispatcher integration
    pub fn new(config: DispatcherConfig) -> Self {
        let confirmation = ToolConfirmation::new(config.confirmation.clone());

        // Create async confirmation handler with config from ConfirmationConfig
        let async_config = AsyncConfirmationConfig {
            enabled: config.confirmation.enabled,
            timeout_ms: config.confirmation.timeout_ms,
            threshold: config.confirmation.threshold,
            skip_native_tools: config.confirmation.skip_native_tools,
        };
        let async_confirmation = Arc::new(AsyncConfirmationHandler::with_config(async_config));

        Self {
            config,
            confirmation,
            async_confirmation,
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(DispatcherConfig::default())
    }

    /// Check if the dispatcher is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Check if confirmation is needed for a tool execution
    pub fn needs_confirmation(&self, tool: &UnifiedTool, confidence: f32) -> bool {
        self.confirmation.should_confirm(confidence)
            && !self.confirmation.should_skip_for_tool(tool)
    }

    /// Get the confirmation configuration
    pub fn confirmation_config(&self) -> &ConfirmationConfig {
        &self.config.confirmation
    }

    /// Get the dispatcher configuration
    pub fn config(&self) -> &DispatcherConfig {
        &self.config
    }

    // =========================================================================
    // Async Confirmation Flow (Non-blocking)
    // =========================================================================

    /// Get the async confirmation handler
    pub fn async_confirmation(&self) -> &Arc<AsyncConfirmationHandler> {
        &self.async_confirmation
    }

    /// Handle confirmation asynchronously (non-blocking)
    ///
    /// Instead of blocking for user response, this method:
    /// 1. Creates a pending confirmation entry
    /// 2. Notifies Swift via `on_confirmation_needed` callback
    /// 3. Returns `DispatcherResult` with `PendingConfirmation` action
    ///
    /// The caller should:
    /// 1. Check if `result.action.is_pending()`
    /// 2. If pending, wait for user decision via `confirm_action()` / `cancel_confirmation()`
    /// 3. Then call `resume_with_decision()` to complete the flow
    pub fn handle_unified_confirmation_async(
        &self,
        tool: UnifiedTool,
        confidence: f32,
        parameters: Option<serde_json::Value>,
        reason: Option<String>,
        routing_layer: RoutingLayer,
        event_handler: &dyn InternalEventHandler,
    ) -> Result<DispatcherResult> {
        info!(
            tool = %tool.name,
            confidence = confidence,
            threshold = self.confirmation.threshold(),
            "Creating async confirmation for unified routing match"
        );

        let params = parameters.clone().unwrap_or_default();
        let reason_str = reason.clone().unwrap_or_else(|| "Low confidence match".to_string());

        // Create pending confirmation state
        let state = self.async_confirmation.create_pending(
            tool.clone(),
            params.clone(),
            confidence,
            reason_str,
            routing_layer,
        );

        // Extract pending confirmation and notify Swift
        match state {
            ConfirmationState::Pending(ref pending) => {
                let confirmation_id = pending.id.clone();
                let info = pending.to_ffi();
                event_handler.on_confirmation_needed(info);

                // Return pending result
                Ok(DispatcherResult::pending(
                    confirmation_id,
                    tool,
                    params,
                    routing_layer,
                    confidence,
                ))
            }
            ConfirmationState::Cancelled { reason } => {
                warn!(reason = %reason, "Failed to create pending confirmation");
                Ok(DispatcherResult::cancelled().with_reason(reason))
            }
            _ => {
                warn!("Unexpected state from create_pending");
                Ok(DispatcherResult::general_chat())
            }
        }
    }

    /// Resume processing after user confirmation decision
    ///
    /// This method is called after Swift receives user input and calls
    /// `confirm_action()` or `cancel_confirmation()`.
    ///
    /// # Arguments
    ///
    /// * `confirmation_id` - The confirmation ID from the pending result
    /// * `decision` - User's decision (Execute, Cancel, EditParameters)
    ///
    /// # Returns
    ///
    /// A `DispatcherResult` based on user's decision
    pub fn resume_with_decision(
        &self,
        confirmation_id: &str,
        decision: UserConfirmationDecision,
    ) -> Result<DispatcherResult> {
        // Process the decision via async confirmation handler
        let state = self.async_confirmation.resume_with_decision(confirmation_id, decision);

        match state {
            ConfirmationState::Confirmed {
                tool,
                parameters,
                routing_layer,
                confidence,
            } => {
                info!(tool = %tool.name, "User confirmed tool execution (async)");
                Ok(DispatcherResult::execute(tool, parameters, routing_layer, confidence)
                    .with_confirmation()
                    .with_reason("User confirmed execution"))
            }
            ConfirmationState::Cancelled { reason } => {
                info!(reason = %reason, "User cancelled tool execution (async)");
                Ok(DispatcherResult::cancelled().with_reason(reason))
            }
            ConfirmationState::TimedOut { confirmation_id: id } => {
                warn!(id = %id, "Confirmation timed out (async)");
                Ok(DispatcherResult::error("Confirmation timed out"))
            }
            ConfirmationState::NotRequired => {
                // This shouldn't happen if called correctly
                warn!("resume_with_decision called but confirmation not found or not required");
                Ok(DispatcherResult::general_chat()
                    .with_reason("Confirmation not found"))
            }
            ConfirmationState::Pending(_) => {
                // This shouldn't happen after resume_with_decision
                warn!("Confirmation still pending after resume_with_decision");
                Ok(DispatcherResult::error("Confirmation processing error"))
            }
        }
    }

    /// Get a pending confirmation by ID
    pub fn get_pending_confirmation(&self, confirmation_id: &str) -> Option<PendingConfirmationInfo> {
        self.async_confirmation
            .get_pending(confirmation_id)
            .map(|p| p.to_ffi())
    }

    /// Check if a confirmation is still pending
    pub fn is_confirmation_pending(&self, confirmation_id: &str) -> bool {
        self.async_confirmation.get_pending(confirmation_id).is_some()
    }

    /// Get the count of pending confirmations
    pub fn pending_confirmation_count(&self) -> usize {
        self.async_confirmation.pending_count()
    }

    /// Cleanup expired confirmations and notify via callback
    ///
    /// Returns the number of expired confirmations that were cleaned up
    pub fn cleanup_expired_confirmations(
        &self,
        event_handler: &dyn InternalEventHandler,
    ) -> usize {
        // Cleanup expired and get their IDs
        let expired_ids = self.async_confirmation.cleanup_expired();
        let count = expired_ids.len();

        // Notify Swift for each expired confirmation
        for id in expired_ids {
            event_handler.on_confirmation_expired(id);
        }

        count
    }
}

impl Default for DispatcherIntegration {
    fn default() -> Self {
        Self::new(DispatcherConfig::default())
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
                "native:youtube",
                "youtube",
                "Analyze YouTube video",
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
        let integration = DispatcherIntegration::new(config);

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
        let integration = DispatcherIntegration::new(config);

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
        let integration = DispatcherIntegration::default();

        // Try to resume with non-existent confirmation
        let result = integration.resume_with_decision("non-existent-id", UserConfirmationDecision::Execute);

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
        assert_eq!(thresholds.classify(0.3), ConfidenceAction::RequiresConfirmation);
        assert_eq!(thresholds.classify(0.5), ConfidenceAction::RequiresConfirmation);
        assert_eq!(thresholds.classify(0.69), ConfidenceAction::RequiresConfirmation);
    }

    #[test]
    fn test_confidence_thresholds_classify_optional_confirmation() {
        let thresholds = ConfidenceThresholds::default();
        assert_eq!(thresholds.classify(0.7), ConfidenceAction::OptionalConfirmation);
        assert_eq!(thresholds.classify(0.8), ConfidenceAction::OptionalConfirmation);
        assert_eq!(thresholds.classify(0.89), ConfidenceAction::OptionalConfirmation);
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
        assert!(thresholds.needs_confirmation(0.5, true));  // RequiresConfirmation - yes
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
