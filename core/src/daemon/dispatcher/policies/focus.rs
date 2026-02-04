//! Focus Mode Policy
//!
//! Enables Do Not Disturb when programming with high CPU usage.

use crate::daemon::dispatcher::policy::{ActionType, Policy, ProposedAction, RiskLevel};
use crate::daemon::events::DerivedEvent;
use crate::daemon::worldmodel::state::{ActivityType, EnhancedContext};

/// Policy that enables focus mode during intensive programming
pub struct FocusModePolicy;

impl Policy for FocusModePolicy {
    fn name(&self) -> &str {
        "Focus Mode for High CPU Tasks"
    }

    fn evaluate(
        &self,
        context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction> {
        if let DerivedEvent::ActivityChanged {
            new_activity: ActivityType::Programming { .. },
            ..
        } = event {
            if context.system_constraint.cpu_usage > 70.0 {
                return Some(ProposedAction {
                    action_type: ActionType::EnableDoNotDisturb,
                    reason: "High CPU usage detected during programming".into(),
                    risk_level: RiskLevel::Medium,
                    metadata: [("cpu_usage".into(), context.system_constraint.cpu_usage.into())]
                        .into_iter()
                        .collect(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::worldmodel::state::{MemoryPressure, SystemLoad};
    use chrono::Utc;

    #[test]
    fn test_focus_mode_triggers_on_programming_with_high_cpu() {
        let policy = FocusModePolicy;
        let mut context = EnhancedContext::default();
        context.system_constraint = SystemLoad {
            cpu_usage: 85.0,
            memory_pressure: MemoryPressure::Normal,
            battery_level: None,
        };

        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Idle,
            new_activity: ActivityType::Programming {
                language: Some("rust".to_string()),
                project: Some("/path/to/project".to_string()),
            },
            confidence: 0.9,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_some());

        let action = result.unwrap();
        assert!(matches!(
            action.action_type,
            ActionType::EnableDoNotDisturb
        ));
        assert_eq!(action.risk_level as u8, RiskLevel::Medium as u8);
        assert!(action.metadata.contains_key("cpu_usage"));
    }

    #[test]
    fn test_focus_mode_does_not_trigger_with_low_cpu() {
        let policy = FocusModePolicy;
        let mut context = EnhancedContext::default();
        context.system_constraint = SystemLoad {
            cpu_usage: 30.0,
            memory_pressure: MemoryPressure::Normal,
            battery_level: None,
        };

        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Idle,
            new_activity: ActivityType::Programming {
                language: Some("rust".to_string()),
                project: None,
            },
            confidence: 0.9,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_none());
    }

    #[test]
    fn test_focus_mode_does_not_trigger_for_non_programming() {
        let policy = FocusModePolicy;
        let mut context = EnhancedContext::default();
        context.system_constraint = SystemLoad {
            cpu_usage: 85.0,
            memory_pressure: MemoryPressure::Normal,
            battery_level: None,
        };

        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Idle,
            new_activity: ActivityType::Reading,
            confidence: 0.9,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_none());
    }
}
