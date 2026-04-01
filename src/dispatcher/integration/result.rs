//! Dispatcher result types

use crate::dispatcher::{RoutingLayer, UnifiedTool};

use super::DispatcherAction;

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
