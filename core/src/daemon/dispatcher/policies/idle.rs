//! Idle Cleanup Policy
//!
//! Suggests cleanup when system is idle for extended periods.

use crate::daemon::dispatcher::policy::{
    ActionType, NotificationPriority, Policy, ProposedAction, RiskLevel,
};
use crate::daemon::events::DerivedEvent;
use crate::daemon::worldmodel::state::{ActivityType, EnhancedContext};
use chrono::Duration;

/// Policy that suggests cleanup during idle periods
pub struct IdleCleanupPolicy;

impl Policy for IdleCleanupPolicy {
    fn name(&self) -> &str {
        "Idle Cleanup"
    }

    fn evaluate(&self, context: &EnhancedContext, event: &DerivedEvent) -> Option<ProposedAction> {
        if let DerivedEvent::ActivityChanged { new_activity, .. } = event {
            if matches!(new_activity, ActivityType::Idle)
                && context.activity_duration > Duration::minutes(30)
            {
                return Some(ProposedAction {
                    action_type: ActionType::NotifyUser {
                        message: "Clean up temporary files?".into(),
                        priority: NotificationPriority::Low,
                    },
                    reason: "System idle for 30+ minutes".into(),
                    risk_level: RiskLevel::Medium,
                    metadata: std::collections::HashMap::new(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_idle_cleanup_triggers_after_30_minutes() {
        let policy = IdleCleanupPolicy;
        let context = EnhancedContext {
            activity_duration: Duration::minutes(35),
            ..EnhancedContext::default()
        };

        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Reading,
            new_activity: ActivityType::Idle,
            confidence: 0.9,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_some());

        let action = result.unwrap();
        assert!(matches!(
            action.action_type,
            ActionType::NotifyUser {
                priority: NotificationPriority::Low,
                ..
            }
        ));
        assert_eq!(action.risk_level as u8, RiskLevel::Medium as u8);
        assert_eq!(action.reason, "System idle for 30+ minutes");
    }

    #[test]
    fn test_idle_cleanup_does_not_trigger_before_30_minutes() {
        let policy = IdleCleanupPolicy;
        let context = EnhancedContext {
            activity_duration: Duration::minutes(15),
            ..EnhancedContext::default()
        };

        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Reading,
            new_activity: ActivityType::Idle,
            confidence: 0.9,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_none());
    }

    #[test]
    fn test_idle_cleanup_does_not_trigger_for_non_idle() {
        let policy = IdleCleanupPolicy;
        let context = EnhancedContext {
            activity_duration: Duration::minutes(35),
            ..EnhancedContext::default()
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
}
