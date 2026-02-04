//! Action Executor
//!
//! Executes proactive actions proposed by policies.
//!
//! MVP Implementation: All actions are LOGGED only, no actual system calls
//! for safety during testing.

use crate::daemon::{ProposedAction, Result};
use std::sync::Arc;

/// Action executor - executes proactive actions
pub struct ActionExecutor {
    // Future expansion: system API handles
}

impl ActionExecutor {
    /// Create a new action executor
    pub fn new() -> Arc<Self> {
        Arc::new(Self {})
    }

    /// Execute a proposed action
    ///
    /// MVP: Logs all actions instead of executing them for safety.
    /// Production: Would make actual system calls.
    pub async fn execute(&self, action: ProposedAction) -> Result<()> {
        use crate::daemon::dispatcher::policy::ActionType;

        log::info!("ActionExecutor: Executing action {:?}", action.action_type);
        log::debug!("  Reason: {}", action.reason);
        log::debug!("  Risk: {:?}", action.risk_level);

        match action.action_type {
            ActionType::MuteSystemAudio => {
                log::info!("Executed: Mute system audio");
                log::debug!("  (Real: osascript -e 'set volume output muted true')");
            }

            ActionType::UnmuteSystemAudio => {
                log::info!("Executed: Unmute system audio");
                log::debug!("  (Real: osascript -e 'set volume output muted false')");
            }

            ActionType::EnableDoNotDisturb => {
                log::info!("Executed: Enable Do Not Disturb");
                log::debug!("  (Real: shortcuts run 'Set Focus')");
            }

            ActionType::DisableDoNotDisturb => {
                log::info!("Executed: Disable Do Not Disturb");
                log::debug!("  (Real: shortcuts run 'Clear Focus')");
            }

            ActionType::NotifyUser { ref message, priority } => {
                log::info!("Executed: Send notification [{:?}]: {}", priority, message);
                log::debug!("  (Real: Gateway IPC notification)");
            }

            ActionType::AdjustBrightness { level } => {
                log::info!("Executed: Adjust brightness to {}%", level);
                log::debug!("  (Real: brightness {})", level as f64 / 100.0);
            }
        }

        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::dispatcher::policy::{ActionType, NotificationPriority, RiskLevel};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_executor_mute_audio() {
        let executor = ActionExecutor::new();
        let action = ProposedAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Test mute".to_string(),
            risk_level: RiskLevel::Low,
            metadata: HashMap::new(),
        };

        let result = executor.execute(action).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_unmute_audio() {
        let executor = ActionExecutor::new();
        let action = ProposedAction {
            action_type: ActionType::UnmuteSystemAudio,
            reason: "Test unmute".to_string(),
            risk_level: RiskLevel::Low,
            metadata: HashMap::new(),
        };

        let result = executor.execute(action).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_enable_dnd() {
        let executor = ActionExecutor::new();
        let action = ProposedAction {
            action_type: ActionType::EnableDoNotDisturb,
            reason: "Test DND".to_string(),
            risk_level: RiskLevel::Medium,
            metadata: HashMap::new(),
        };

        let result = executor.execute(action).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_disable_dnd() {
        let executor = ActionExecutor::new();
        let action = ProposedAction {
            action_type: ActionType::DisableDoNotDisturb,
            reason: "Test disable DND".to_string(),
            risk_level: RiskLevel::Low,
            metadata: HashMap::new(),
        };

        let result = executor.execute(action).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_notify_user() {
        let executor = ActionExecutor::new();
        let action = ProposedAction {
            action_type: ActionType::NotifyUser {
                message: "Test notification".to_string(),
                priority: NotificationPriority::Normal,
            },
            reason: "Test notify".to_string(),
            risk_level: RiskLevel::Low,
            metadata: HashMap::new(),
        };

        let result = executor.execute(action).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_adjust_brightness() {
        let executor = ActionExecutor::new();
        let action = ProposedAction {
            action_type: ActionType::AdjustBrightness { level: 50 },
            reason: "Test brightness".to_string(),
            risk_level: RiskLevel::Low,
            metadata: HashMap::new(),
        };

        let result = executor.execute(action).await;
        assert!(result.is_ok());
    }
}
