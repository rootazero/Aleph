//! Dispatcher action types

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
