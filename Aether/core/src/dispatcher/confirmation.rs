//! Tool Confirmation Flow for Dispatcher Layer
//!
//! This module implements the Halo confirmation flow for tool execution:
//!
//! - Confidence-based confirmation triggering
//! - Tool preview formatting
//! - Execute/Cancel option handling
//! - Timeout management
//!
//! # Architecture
//!
//! ```text
//! L3 Routing Result (confidence < threshold)
//!       ↓
//! ┌───────────────────────────────────────┐
//! │      Confirmation Flow                │
//! │                                       │
//! │  should_confirm(confidence)           │
//! │           ↓                           │
//! │  build_confirmation_request(tool)     │
//! │           ↓                           │
//! │  on_clarification_needed(request)     │
//! │           ↓                           │
//! │  handle_confirmation_result(result)   │
//! └───────────────────────────────────────┘
//!       ↓
//! Execute Tool / Fallback to Chat / Error
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::{ToolConfirmation, ConfirmationConfig};
//!
//! let config = ConfirmationConfig::default();
//! let confirmation = ToolConfirmation::new(config);
//!
//! // Check if confirmation is needed
//! if confirmation.should_confirm(routing_result.confidence) {
//!     let request = confirmation.build_request(&tool, &params);
//!     let result = event_handler.on_clarification_needed(request);
//!     let action = confirmation.handle_result(result);
//! }
//! ```

use crate::clarification::{
    ClarificationOption, ClarificationRequest, ClarificationResult, ClarificationResultType,
};
use crate::dispatcher::{RoutingLayer, UnifiedTool};
use serde::{Deserialize, Serialize};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for tool confirmation behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationConfig {
    /// Confidence threshold below which confirmation is required
    /// Range: 0.0 - 1.0 (default: 0.7)
    pub threshold: f32,

    /// Whether confirmation is enabled globally
    pub enabled: bool,

    /// Timeout for confirmation in milliseconds
    /// 0 = no timeout (wait indefinitely)
    pub timeout_ms: u64,

    /// Whether to show detailed parameters in confirmation
    pub show_parameters: bool,

    /// Whether to skip confirmation for certain tool sources
    pub skip_native_tools: bool,
}

impl Default for ConfirmationConfig {
    fn default() -> Self {
        Self {
            threshold: 0.7,
            enabled: true,
            timeout_ms: 30000, // 30 seconds default
            show_parameters: true,
            skip_native_tools: false,
        }
    }
}

impl ConfirmationConfig {
    /// Create config with custom threshold
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Disable confirmation
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

// =============================================================================
// Confirmation Actions
// =============================================================================

/// Action to take after confirmation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationAction {
    /// User confirmed - proceed with tool execution
    Execute,

    /// User cancelled - fallback to general chat
    Cancel,

    /// Confirmation timed out - abort with error
    Timeout,

    /// User wants to edit parameters before execution
    EditParameters,
}

impl ConfirmationAction {
    /// Check if action is positive (should proceed)
    pub fn should_proceed(&self) -> bool {
        matches!(self, ConfirmationAction::Execute)
    }

    /// Check if action requires fallback
    pub fn should_fallback(&self) -> bool {
        matches!(self, ConfirmationAction::Cancel)
    }

    /// Check if action is an error condition
    pub fn is_error(&self) -> bool {
        matches!(self, ConfirmationAction::Timeout)
    }
}

// =============================================================================
// Tool Confirmation
// =============================================================================

/// Tool confirmation handler
pub struct ToolConfirmation {
    /// Configuration
    config: ConfirmationConfig,
}

impl ToolConfirmation {
    /// Create a new tool confirmation handler
    pub fn new(config: ConfirmationConfig) -> Self {
        Self { config }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ConfirmationConfig::default())
    }

    /// Check if confirmation is needed based on confidence
    ///
    /// # Arguments
    ///
    /// * `confidence` - Routing confidence score (0.0 - 1.0)
    ///
    /// # Returns
    ///
    /// `true` if confirmation should be shown
    pub fn should_confirm(&self, confidence: f32) -> bool {
        self.config.enabled && confidence < self.config.threshold
    }

    /// Check if confirmation should be skipped for this tool
    pub fn should_skip_for_tool(&self, tool: &UnifiedTool) -> bool {
        if self.config.skip_native_tools {
            matches!(tool.source, crate::dispatcher::ToolSource::Native)
        } else {
            false
        }
    }

    /// Build a confirmation request for tool execution
    ///
    /// # Arguments
    ///
    /// * `tool` - The tool to be executed
    /// * `parameters` - Extracted parameters (optional)
    /// * `reason` - Routing reason (optional)
    ///
    /// # Returns
    ///
    /// A `ClarificationRequest` for the Halo overlay
    pub fn build_request(
        &self,
        tool: &UnifiedTool,
        parameters: Option<&serde_json::Value>,
        reason: Option<&str>,
    ) -> ClarificationRequest {
        let prompt = self.format_prompt(tool, parameters, reason);

        let options = vec![
            ClarificationOption::with_description(
                OPTION_EXECUTE,
                "Execute",
                &format!("Run {} with these parameters", tool.name),
            ),
            ClarificationOption::with_description(
                OPTION_CANCEL,
                "Cancel",
                "Skip tool and continue as chat",
            ),
        ];

        ClarificationRequest::select(&format!("tool-confirm:{}", tool.id), &prompt, options)
            .with_source(&format!("dispatcher:{}", tool.source.label()))
    }

    /// Build a confirmation request with edit option
    pub fn build_request_with_edit(
        &self,
        tool: &UnifiedTool,
        parameters: Option<&serde_json::Value>,
        reason: Option<&str>,
    ) -> ClarificationRequest {
        let prompt = self.format_prompt(tool, parameters, reason);

        let options = vec![
            ClarificationOption::with_description(
                OPTION_EXECUTE,
                "Execute",
                &format!("Run {} with these parameters", tool.name),
            ),
            ClarificationOption::with_description(
                OPTION_EDIT,
                "Edit",
                "Modify parameters before execution",
            ),
            ClarificationOption::with_description(
                OPTION_CANCEL,
                "Cancel",
                "Skip tool and continue as chat",
            ),
        ];

        ClarificationRequest::select(&format!("tool-confirm:{}", tool.id), &prompt, options)
            .with_source(&format!("dispatcher:{}", tool.source.label()))
    }

    /// Format the confirmation prompt
    fn format_prompt(
        &self,
        tool: &UnifiedTool,
        parameters: Option<&serde_json::Value>,
        reason: Option<&str>,
    ) -> String {
        let mut prompt = format!("**{}**\n{}", tool.name, tool.description);

        // Add reason if provided
        if let Some(r) = reason {
            prompt.push_str(&format!("\n\n_{}_", r));
        }

        // Add parameters if enabled and present
        if self.config.show_parameters {
            if let Some(params) = parameters {
                if !params.is_null() && params.as_object().map_or(false, |o| !o.is_empty()) {
                    prompt.push_str("\n\n**Parameters:**\n");
                    prompt.push_str(&format_parameters(params));
                }
            }
        }

        prompt.push_str("\n\nExecute this tool?");
        prompt
    }

    /// Handle the confirmation result
    ///
    /// # Arguments
    ///
    /// * `result` - The clarification result from the user
    ///
    /// # Returns
    ///
    /// The action to take based on user response
    pub fn handle_result(&self, result: ClarificationResult) -> ConfirmationAction {
        match result.result_type {
            ClarificationResultType::Selected => {
                match result.value.as_deref() {
                    Some(OPTION_EXECUTE) => ConfirmationAction::Execute,
                    Some(OPTION_CANCEL) => ConfirmationAction::Cancel,
                    Some(OPTION_EDIT) => ConfirmationAction::EditParameters,
                    _ => ConfirmationAction::Cancel, // Unknown option = cancel
                }
            }
            ClarificationResultType::TextInput => {
                // Text input not expected for tool confirmation
                ConfirmationAction::Cancel
            }
            ClarificationResultType::Cancelled => ConfirmationAction::Cancel,
            ClarificationResultType::Timeout => ConfirmationAction::Timeout,
        }
    }

    /// Get the confirmation timeout in milliseconds
    pub fn timeout_ms(&self) -> u64 {
        self.config.timeout_ms
    }

    /// Get the confirmation threshold
    pub fn threshold(&self) -> f32 {
        self.config.threshold
    }

    /// Check if confirmation is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

impl Default for ToolConfirmation {
    fn default() -> Self {
        Self::with_defaults()
    }
}

// =============================================================================
// Confirmation Decision
// =============================================================================

/// Decision result from confirmation flow
#[derive(Debug, Clone)]
pub struct ConfirmationDecision {
    /// The action decided
    pub action: ConfirmationAction,

    /// The original tool (if any)
    pub tool: Option<UnifiedTool>,

    /// The original parameters (if any)
    pub parameters: Option<serde_json::Value>,

    /// Routing layer that produced this result
    pub routing_layer: RoutingLayer,

    /// Additional context message
    pub message: Option<String>,
}

impl ConfirmationDecision {
    /// Create an execute decision
    pub fn execute(tool: UnifiedTool, parameters: serde_json::Value) -> Self {
        Self {
            action: ConfirmationAction::Execute,
            tool: Some(tool),
            parameters: Some(parameters),
            routing_layer: RoutingLayer::L3Inference,
            message: None,
        }
    }

    /// Create a cancel decision (fallback to chat)
    pub fn cancel() -> Self {
        Self {
            action: ConfirmationAction::Cancel,
            tool: None,
            parameters: None,
            routing_layer: RoutingLayer::Default,
            message: Some("User cancelled tool execution".to_string()),
        }
    }

    /// Create a timeout decision
    pub fn timeout() -> Self {
        Self {
            action: ConfirmationAction::Timeout,
            tool: None,
            parameters: None,
            routing_layer: RoutingLayer::Default,
            message: Some("Confirmation timed out".to_string()),
        }
    }

    /// Create a no-confirmation-needed decision (high confidence)
    pub fn no_confirmation_needed(tool: UnifiedTool, parameters: serde_json::Value) -> Self {
        Self {
            action: ConfirmationAction::Execute,
            tool: Some(tool),
            parameters: Some(parameters),
            routing_layer: RoutingLayer::L3Inference,
            message: Some("High confidence - no confirmation needed".to_string()),
        }
    }

    /// Check if execution should proceed
    pub fn should_execute(&self) -> bool {
        self.action.should_proceed()
    }

    /// Check if should fallback to chat
    pub fn should_fallback(&self) -> bool {
        self.action.should_fallback()
    }
}

// =============================================================================
// Constants
// =============================================================================

/// Option value for "Execute"
pub const OPTION_EXECUTE: &str = "execute";

/// Option value for "Cancel"
pub const OPTION_CANCEL: &str = "cancel";

/// Option value for "Edit"
pub const OPTION_EDIT: &str = "edit";

// =============================================================================
// Helper Functions
// =============================================================================

/// Format parameters for display
fn format_parameters(params: &serde_json::Value) -> String {
    if let Some(obj) = params.as_object() {
        obj.iter()
            .map(|(k, v)| {
                let value_str = match v {
                    serde_json::Value::String(s) => format!("\"{}\"", truncate(s, 50)),
                    serde_json::Value::Null => "null".to_string(),
                    _ => truncate(&v.to_string(), 50),
                };
                format!("• {}: {}", k, value_str)
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        params.to_string()
    }
}

/// Truncate string with ellipsis
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;
    use serde_json::json;

    fn create_test_tool() -> UnifiedTool {
        UnifiedTool::new(
            "native:search",
            "search",
            "Search the web for information",
            ToolSource::Native,
        )
    }

    #[test]
    fn test_confirmation_config_default() {
        let config = ConfirmationConfig::default();

        assert_eq!(config.threshold, 0.7);
        assert!(config.enabled);
        assert_eq!(config.timeout_ms, 30000);
        assert!(config.show_parameters);
    }

    #[test]
    fn test_confirmation_config_with_threshold() {
        let config = ConfirmationConfig::default().with_threshold(0.5);
        assert_eq!(config.threshold, 0.5);

        // Test clamping
        let config = ConfirmationConfig::default().with_threshold(1.5);
        assert_eq!(config.threshold, 1.0);

        let config = ConfirmationConfig::default().with_threshold(-0.5);
        assert_eq!(config.threshold, 0.0);
    }

    #[test]
    fn test_confirmation_config_disabled() {
        let config = ConfirmationConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_should_confirm() {
        let confirmation = ToolConfirmation::with_defaults();

        // Below threshold - should confirm
        assert!(confirmation.should_confirm(0.5));
        assert!(confirmation.should_confirm(0.69));

        // At or above threshold - should not confirm
        assert!(!confirmation.should_confirm(0.7));
        assert!(!confirmation.should_confirm(0.9));
        assert!(!confirmation.should_confirm(1.0));
    }

    #[test]
    fn test_should_confirm_disabled() {
        let config = ConfirmationConfig::disabled();
        let confirmation = ToolConfirmation::new(config);

        // Disabled - never confirm
        assert!(!confirmation.should_confirm(0.0));
        assert!(!confirmation.should_confirm(0.5));
    }

    #[test]
    fn test_build_request() {
        let confirmation = ToolConfirmation::with_defaults();
        let tool = create_test_tool();

        let request = confirmation.build_request(&tool, None, None);

        assert!(request.id.contains("tool-confirm:"));
        assert!(request.prompt.contains("search"));
        assert!(request.prompt.contains("Execute this tool?"));
        assert!(request.options.is_some());

        let options = request.options.unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].value, OPTION_EXECUTE);
        assert_eq!(options[1].value, OPTION_CANCEL);
    }

    #[test]
    fn test_build_request_with_parameters() {
        let confirmation = ToolConfirmation::with_defaults();
        let tool = create_test_tool();
        let params = json!({"query": "weather in Tokyo", "limit": 5});

        let request = confirmation.build_request(&tool, Some(&params), None);

        assert!(request.prompt.contains("Parameters:"));
        assert!(request.prompt.contains("query"));
        assert!(request.prompt.contains("weather in Tokyo"));
    }

    #[test]
    fn test_build_request_with_reason() {
        let confirmation = ToolConfirmation::with_defaults();
        let tool = create_test_tool();

        let request =
            confirmation.build_request(&tool, None, Some("User wants to search for information"));

        assert!(request.prompt.contains("User wants to search"));
    }

    #[test]
    fn test_build_request_with_edit() {
        let confirmation = ToolConfirmation::with_defaults();
        let tool = create_test_tool();

        let request = confirmation.build_request_with_edit(&tool, None, None);

        let options = request.options.unwrap();
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].value, OPTION_EXECUTE);
        assert_eq!(options[1].value, OPTION_EDIT);
        assert_eq!(options[2].value, OPTION_CANCEL);
    }

    #[test]
    fn test_handle_result_execute() {
        let confirmation = ToolConfirmation::with_defaults();
        let result = ClarificationResult::selected(0, OPTION_EXECUTE.to_string());

        let action = confirmation.handle_result(result);
        assert_eq!(action, ConfirmationAction::Execute);
        assert!(action.should_proceed());
    }

    #[test]
    fn test_handle_result_cancel() {
        let confirmation = ToolConfirmation::with_defaults();
        let result = ClarificationResult::selected(1, OPTION_CANCEL.to_string());

        let action = confirmation.handle_result(result);
        assert_eq!(action, ConfirmationAction::Cancel);
        assert!(action.should_fallback());
    }

    #[test]
    fn test_handle_result_edit() {
        let confirmation = ToolConfirmation::with_defaults();
        let result = ClarificationResult::selected(1, OPTION_EDIT.to_string());

        let action = confirmation.handle_result(result);
        assert_eq!(action, ConfirmationAction::EditParameters);
    }

    #[test]
    fn test_handle_result_cancelled() {
        let confirmation = ToolConfirmation::with_defaults();
        let result = ClarificationResult::cancelled();

        let action = confirmation.handle_result(result);
        assert_eq!(action, ConfirmationAction::Cancel);
    }

    #[test]
    fn test_handle_result_timeout() {
        let confirmation = ToolConfirmation::with_defaults();
        let result = ClarificationResult::timeout();

        let action = confirmation.handle_result(result);
        assert_eq!(action, ConfirmationAction::Timeout);
        assert!(action.is_error());
    }

    #[test]
    fn test_confirmation_decision_execute() {
        let tool = create_test_tool();
        let params = json!({"query": "test"});

        let decision = ConfirmationDecision::execute(tool, params);

        assert!(decision.should_execute());
        assert!(!decision.should_fallback());
        assert!(decision.tool.is_some());
        assert!(decision.parameters.is_some());
    }

    #[test]
    fn test_confirmation_decision_cancel() {
        let decision = ConfirmationDecision::cancel();

        assert!(!decision.should_execute());
        assert!(decision.should_fallback());
        assert!(decision.tool.is_none());
    }

    #[test]
    fn test_confirmation_decision_timeout() {
        let decision = ConfirmationDecision::timeout();

        assert!(!decision.should_execute());
        assert!(!decision.should_fallback());
        assert!(decision.action.is_error());
    }

    #[test]
    fn test_confirmation_action() {
        assert!(ConfirmationAction::Execute.should_proceed());
        assert!(!ConfirmationAction::Execute.should_fallback());
        assert!(!ConfirmationAction::Execute.is_error());

        assert!(!ConfirmationAction::Cancel.should_proceed());
        assert!(ConfirmationAction::Cancel.should_fallback());
        assert!(!ConfirmationAction::Cancel.is_error());

        assert!(!ConfirmationAction::Timeout.should_proceed());
        assert!(!ConfirmationAction::Timeout.should_fallback());
        assert!(ConfirmationAction::Timeout.is_error());
    }

    #[test]
    fn test_format_parameters() {
        let params = json!({
            "query": "test search",
            "limit": 10,
            "enabled": true
        });

        let formatted = format_parameters(&params);

        assert!(formatted.contains("query"));
        assert!(formatted.contains("test search"));
        assert!(formatted.contains("limit"));
        assert!(formatted.contains("10"));
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("this is a long text", 10), "this is...");
    }

    #[test]
    fn test_skip_native_tools() {
        let config = ConfirmationConfig {
            skip_native_tools: true,
            ..Default::default()
        };
        let confirmation = ToolConfirmation::new(config);

        let native_tool = UnifiedTool::new("native:test", "test", "Test", ToolSource::Native);
        let mcp_tool = UnifiedTool::new(
            "mcp:test",
            "test",
            "Test",
            ToolSource::Mcp {
                server: "test".to_string(),
            },
        );

        assert!(confirmation.should_skip_for_tool(&native_tool));
        assert!(!confirmation.should_skip_for_tool(&mcp_tool));
    }
}
