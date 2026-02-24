//! High CPU Alert Policy
//!
//! Alerts user when CPU usage exceeds 90%.

use crate::daemon::dispatcher::policy::{
    ActionType, NotificationPriority, Policy, ProposedAction, RiskLevel,
};
use crate::daemon::events::DerivedEvent;
use crate::daemon::worldmodel::state::EnhancedContext;

/// Policy that alerts on high CPU usage
pub struct HighCpuAlertPolicy;

impl Policy for HighCpuAlertPolicy {
    fn name(&self) -> &str {
        "High CPU Alert"
    }

    fn evaluate(&self, context: &EnhancedContext, _event: &DerivedEvent) -> Option<ProposedAction> {
        if context.system_constraint.cpu_usage > 90.0 {
            return Some(ProposedAction {
                action_type: ActionType::NotifyUser {
                    message: format!("CPU usage at {:.1}%", context.system_constraint.cpu_usage),
                    priority: NotificationPriority::High,
                },
                reason: "CPU usage exceeds 90%".into(),
                risk_level: RiskLevel::Low,
                metadata: [(
                    "cpu_usage".into(),
                    context.system_constraint.cpu_usage.into(),
                )]
                .into_iter()
                .collect(),
            });
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
    fn test_high_cpu_triggers_above_threshold() {
        let policy = HighCpuAlertPolicy;
        let context = EnhancedContext {
            system_constraint: SystemLoad {
                cpu_usage: 95.5,
                memory_pressure: MemoryPressure::Normal,
                battery_level: None,
            },
            ..EnhancedContext::default()
        };

        let event = DerivedEvent::IdleStateChanged {
            timestamp: Utc::now(),
            is_idle: false,
            idle_duration: None,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_some());

        let action = result.unwrap();
        assert!(matches!(
            action.action_type,
            ActionType::NotifyUser {
                priority: NotificationPriority::High,
                ..
            }
        ));
        assert_eq!(action.risk_level as u8, RiskLevel::Low as u8);
        assert!(action.metadata.contains_key("cpu_usage"));
        assert_eq!(action.reason, "CPU usage exceeds 90%");
    }

    #[test]
    fn test_high_cpu_does_not_trigger_below_threshold() {
        let policy = HighCpuAlertPolicy;
        let context = EnhancedContext {
            system_constraint: SystemLoad {
                cpu_usage: 75.0,
                memory_pressure: MemoryPressure::Normal,
                battery_level: None,
            },
            ..EnhancedContext::default()
        };

        let event = DerivedEvent::IdleStateChanged {
            timestamp: Utc::now(),
            is_idle: false,
            idle_duration: None,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_none());
    }

    #[test]
    fn test_high_cpu_does_not_trigger_at_exactly_90() {
        let policy = HighCpuAlertPolicy;
        let context = EnhancedContext {
            system_constraint: SystemLoad {
                cpu_usage: 90.0,
                memory_pressure: MemoryPressure::Normal,
                battery_level: None,
            },
            ..EnhancedContext::default()
        };

        let event = DerivedEvent::IdleStateChanged {
            timestamp: Utc::now(),
            is_idle: false,
            idle_duration: None,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_none());
    }
}
