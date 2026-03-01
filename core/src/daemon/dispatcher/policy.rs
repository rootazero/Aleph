//! Dispatcher Policy System
//!
//! Defines the Policy trait, action types, risk levels, and the PolicyEngine.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use crate::sync_primitives::Arc;
use crate::daemon::events::DerivedEvent;
use crate::daemon::worldmodel::state::EnhancedContext;
use crate::daemon::worldmodel::WorldModel;
use crate::daemon::error::Result;

/// Policy trait - evaluates context and events to propose actions
#[async_trait]
pub trait Policy: Send + Sync {
    /// Policy name for identification
    fn name(&self) -> &str;

    /// Evaluate context and event to potentially propose an action
    fn evaluate(
        &self,
        context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction>;
}

/// A proposed action from policy evaluation
#[derive(Debug, Clone)]
pub struct ProposedAction {
    pub action_type: ActionType,
    pub reason: String,
    pub risk_level: RiskLevel,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Types of actions the dispatcher can execute
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionType {
    MuteSystemAudio,
    UnmuteSystemAudio,
    EnableDoNotDisturb,
    DisableDoNotDisturb,
    NotifyUser {
        message: String,
        priority: NotificationPriority,
    },
    AdjustBrightness {
        level: u8,
    },
}

/// Risk level for actions
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum RiskLevel {
    Low = 0,
    Medium = 1,
    High = 2,
}

/// Notification priority levels
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum NotificationPriority {
    Low,
    Normal,
    High,
}

/// Policy engine that evaluates all registered policies
pub struct PolicyEngine {
    policies: Vec<Box<dyn Policy>>,
}

impl PolicyEngine {
    /// Create MVP version with 5 initial policies
    pub fn new_mvp() -> Self {
        use crate::daemon::dispatcher::policies::{
            HighCpuAlertPolicy, IdleCleanupPolicy, LowBatteryPolicy, FocusModePolicy, MeetingMutePolicy,
        };

        Self {
            policies: vec![
                Box::new(MeetingMutePolicy),
                Box::new(LowBatteryPolicy),
                Box::new(FocusModePolicy),
                Box::new(IdleCleanupPolicy),
                Box::new(HighCpuAlertPolicy),
            ],
        }
    }

    /// Create PolicyEngine with YAML policies
    pub fn new_with_yaml(
        yaml_path: Option<PathBuf>,
        worldmodel: Arc<WorldModel>,
    ) -> Result<Self> {
        use crate::daemon::dispatcher::policies::{
            HighCpuAlertPolicy, IdleCleanupPolicy, LowBatteryPolicy, FocusModePolicy, MeetingMutePolicy,
        };
        use crate::daemon::dispatcher::yaml_policy::load_yaml_policies;

        let mut policies: Vec<Box<dyn Policy>> = vec![
            // Hardcoded policies (backward compatible)
            Box::new(MeetingMutePolicy),
            Box::new(LowBatteryPolicy),
            Box::new(FocusModePolicy),
            Box::new(IdleCleanupPolicy),
            Box::new(HighCpuAlertPolicy),
        ];

        // Load YAML policies if path provided
        if let Some(path) = yaml_path {
            let yaml_policies = load_yaml_policies(path, worldmodel)?;
            policies.extend(yaml_policies);
        }

        Ok(Self { policies })
    }

    /// Get the number of registered policies
    pub fn policy_count(&self) -> usize {
        self.policies.len()
    }

    /// Evaluate all policies against the context and event
    pub fn evaluate_all(
        &self,
        context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Vec<ProposedAction> {
        self.policies
            .iter()
            .filter_map(|policy| policy.evaluate(context, event))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_type_serialization() {
        let action = ActionType::MuteSystemAudio;
        let json = serde_json::to_string(&action).unwrap();
        let deserialized: ActionType = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, ActionType::MuteSystemAudio));
    }

    #[test]
    fn test_risk_level() {
        assert_eq!(RiskLevel::Low as u8, 0);
        assert_eq!(RiskLevel::Medium as u8, 1);
        assert_eq!(RiskLevel::High as u8, 2);
    }

    #[test]
    fn test_policy_engine_mvp_creation() {
        let engine = PolicyEngine::new_mvp();
        // Should have 5 MVP policies registered
        assert_eq!(engine.policies.len(), 5);
    }

    #[tokio::test]
    async fn test_policy_engine_with_yaml() {
        use crate::daemon::worldmodel::WorldModelConfig;
        use crate::daemon::event_bus::DaemonEventBus;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let yaml = r#"
- name: "Custom Rule"
  enabled: true
  trigger:
    event: activity_changed
  action:
    type: notify
  risk: low
"#;
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(yaml.as_bytes()).unwrap();

        let event_bus = Arc::new(DaemonEventBus::new(100));
        let config = WorldModelConfig::default();
        let worldmodel = Arc::new(WorldModel::new(config, event_bus).await.unwrap());

        let engine = PolicyEngine::new_with_yaml(
            Some(temp_file.path().to_path_buf()),
            worldmodel,
        ).unwrap();

        // Should have 5 hardcoded + 1 YAML = 6 total
        assert_eq!(engine.policies.len(), 6);
    }
}
