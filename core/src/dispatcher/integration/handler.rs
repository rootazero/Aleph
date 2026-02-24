//! Dispatcher integration handler

use crate::dispatcher::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationConfig, ConfirmationState,
    PendingConfirmationInfo, RoutingLayer, ToolConfirmation, UnifiedTool, UserConfirmationDecision,
};
use crate::error::Result;
use crate::event_handler::InternalEventHandler;
use std::sync::Arc;
use tracing::{info, warn};

use super::{DispatcherConfig, DispatcherResult};

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
        let reason_str = reason
            .clone()
            .unwrap_or_else(|| "Low confidence match".to_string());

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
        let state = self
            .async_confirmation
            .resume_with_decision(confirmation_id, decision);

        match state {
            ConfirmationState::Confirmed {
                tool,
                parameters,
                routing_layer,
                confidence,
            } => {
                info!(tool = %tool.name, "User confirmed tool execution (async)");
                Ok(
                    DispatcherResult::execute(tool, parameters, routing_layer, confidence)
                        .with_confirmation()
                        .with_reason("User confirmed execution"),
                )
            }
            ConfirmationState::Cancelled { reason } => {
                info!(reason = %reason, "User cancelled tool execution (async)");
                Ok(DispatcherResult::cancelled().with_reason(reason))
            }
            ConfirmationState::TimedOut {
                confirmation_id: id,
            } => {
                warn!(id = %id, "Confirmation timed out (async)");
                Ok(DispatcherResult::error("Confirmation timed out"))
            }
            ConfirmationState::NotRequired => {
                // This shouldn't happen if called correctly
                warn!("resume_with_decision called but confirmation not found or not required");
                Ok(DispatcherResult::general_chat().with_reason("Confirmation not found"))
            }
            ConfirmationState::Pending(_) => {
                // This shouldn't happen after resume_with_decision
                warn!("Confirmation still pending after resume_with_decision");
                Ok(DispatcherResult::error("Confirmation processing error"))
            }
        }
    }

    /// Get a pending confirmation by ID
    pub fn get_pending_confirmation(
        &self,
        confirmation_id: &str,
    ) -> Option<PendingConfirmationInfo> {
        self.async_confirmation
            .get_pending(confirmation_id)
            .map(|p| p.to_ffi())
    }

    /// Check if a confirmation is still pending
    pub fn is_confirmation_pending(&self, confirmation_id: &str) -> bool {
        self.async_confirmation
            .get_pending(confirmation_id)
            .is_some()
    }

    /// Get the count of pending confirmations
    pub fn pending_confirmation_count(&self) -> usize {
        self.async_confirmation.pending_count()
    }

    /// Cleanup expired confirmations and notify via callback
    ///
    /// Returns the number of expired confirmations that were cleaned up
    pub fn cleanup_expired_confirmations(&self, event_handler: &dyn InternalEventHandler) -> usize {
        // Cleanup expired and get their IDs
        let expired_ids = self.async_confirmation.cleanup_expired();
        let count = expired_ids.len();

        // Notify Swift for each expired confirmation
        for id in expired_ids {
            event_handler.on_confirmation_expired(&id);
        }

        count
    }
}

impl Default for DispatcherIntegration {
    fn default() -> Self {
        Self::new(DispatcherConfig::default())
    }
}
