//! Low Battery Policy
//!
//! Notifies user when battery level drops below 20%.

use crate::daemon::dispatcher::policy::{
    ActionType, NotificationPriority, Policy, ProposedAction, RiskLevel,
};
use crate::daemon::events::DerivedEvent;
use crate::daemon::worldmodel::state::EnhancedContext;

/// Policy that alerts user when battery is low
pub struct LowBatteryPolicy;

impl Policy for LowBatteryPolicy {
    fn name(&self) -> &str {
        "Low Battery Alert"
    }

    fn evaluate(&self, context: &EnhancedContext, _event: &DerivedEvent) -> Option<ProposedAction> {
        if let Some(battery_level) = context.system_constraint.battery_level {
            if battery_level < 20 {
                return Some(ProposedAction {
                    action_type: ActionType::NotifyUser {
                        message: format!("Battery level low: {}%", battery_level),
                        priority: NotificationPriority::High,
                    },
                    reason: "Battery level below 20%".into(),
                    risk_level: RiskLevel::Low,
                    metadata: [("battery_level".into(), battery_level.into())]
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
    fn test_low_battery_triggers_below_threshold() {
        let policy = LowBatteryPolicy;
        let context = EnhancedContext {
            system_constraint: SystemLoad {
                cpu_usage: 0.0,
                memory_pressure: MemoryPressure::Normal,
                battery_level: Some(15),
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
        assert!(action.metadata.contains_key("battery_level"));
    }

    #[test]
    fn test_low_battery_does_not_trigger_above_threshold() {
        let policy = LowBatteryPolicy;
        let context = EnhancedContext {
            system_constraint: SystemLoad {
                cpu_usage: 0.0,
                memory_pressure: MemoryPressure::Normal,
                battery_level: Some(50),
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
    fn test_low_battery_does_not_trigger_without_battery_info() {
        let policy = LowBatteryPolicy;
        let context = EnhancedContext::default(); // battery_level is None

        let event = DerivedEvent::IdleStateChanged {
            timestamp: Utc::now(),
            is_idle: false,
            idle_duration: None,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_none());
    }
}
