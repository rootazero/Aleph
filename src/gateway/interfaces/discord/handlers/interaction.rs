//! Interaction Handler
//!
//! Handles button clicks and select menu interactions.

use crate::gateway::interfaces::discord::handlers::approval::ApprovalQueue;
use serde::Deserialize;
use std::sync::Arc;

/// Message component interaction
#[derive(Debug, Clone, Deserialize)]
pub struct MessageComponent {
    pub custom_id: String,
    pub component_type: u8,
}

/// Interaction handler result
pub type InteractionResult = Result<(), InteractionError>;

/// Interaction errors
#[derive(Debug, thiserror::Error)]
pub enum InteractionError {
    #[error("invalid interaction: {0}")]
    InvalidInteraction(String),

    #[error("handler error: {0}")]
    HandlerError(String),
}

/// Interaction handler for buttons and select menus
#[derive(Clone)]
pub struct InteractionHandler {
    approval_queue: Option<Arc<ApprovalQueue>>,
}

impl InteractionHandler {
    pub fn new() -> Self {
        Self {
            approval_queue: None,
        }
    }

    pub fn with_approval_queue(mut self, queue: Arc<ApprovalQueue>) -> Self {
        self.approval_queue = Some(queue);
        self
    }

    pub async fn handle(&self, _interaction: Interaction) -> InteractionResult {
        Ok(())
    }

    #[allow(dead_code)]
    async fn handle_component(&self, component: MessageComponent) -> InteractionResult {
        match component.component_type {
            2 => self.handle_button(&component.custom_id).await?,
            3..=5 => self.handle_select_menu(&component.custom_id).await?,
            _ => {}
        }
        Ok(())
    }

    #[allow(dead_code)]
    async fn handle_button(&self, custom_id: &str) -> InteractionResult {
        if let Some(approval_id) = custom_id.strip_prefix("exec_approve:") {
            if let Some(queue) = &self.approval_queue {
                queue
                    .approve(approval_id)
                    .await
                    .map_err(|e| InteractionError::HandlerError(e.to_string()))?;
            }
        } else if let Some(approval_id) = custom_id.strip_prefix("exec_deny:") {
            if let Some(queue) = &self.approval_queue {
                queue
                    .deny(approval_id)
                    .await
                    .map_err(|e| InteractionError::HandlerError(e.to_string()))?;
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    async fn handle_select_menu(&self, _custom_id: &str) -> InteractionResult {
        Ok(())
    }
}

impl Default for InteractionHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Placeholder for Interaction enum ( serenity provides this)
#[derive(Debug, Clone)]
pub enum Interaction {}