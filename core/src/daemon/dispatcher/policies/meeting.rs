//! Meeting Mute Policy
//!
//! Automatically mutes system audio when user enters a meeting.

use crate::daemon::dispatcher::policy::{ActionType, Policy, ProposedAction, RiskLevel};
use crate::daemon::events::DerivedEvent;
use crate::daemon::worldmodel::state::{ActivityType, EnhancedContext};

/// Policy that mutes audio when entering a meeting
pub struct MeetingMutePolicy;

impl Policy for MeetingMutePolicy {
    fn name(&self) -> &str {
        "Auto-Mute in Meeting"
    }

    fn evaluate(
        &self,
        _context: &EnhancedContext,
        event: &DerivedEvent,
    ) -> Option<ProposedAction> {
        if let DerivedEvent::ActivityChanged {
            new_activity: ActivityType::Meeting { .. },
            ..
        } = event {
            return Some(ProposedAction {
                action_type: ActionType::MuteSystemAudio,
                reason: "User entered a meeting".into(),
                risk_level: RiskLevel::Low,
                metadata: std::collections::HashMap::new(),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_meeting_mute_triggers_on_meeting_start() {
        let policy = MeetingMutePolicy;
        let context = EnhancedContext::default();
        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Idle,
            new_activity: ActivityType::Meeting { participants: 5 },
            confidence: 0.9,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_some());

        let action = result.unwrap();
        assert!(matches!(action.action_type, ActionType::MuteSystemAudio));
        assert_eq!(action.risk_level as u8, RiskLevel::Low as u8);
        assert_eq!(action.reason, "User entered a meeting");
    }

    #[test]
    fn test_meeting_mute_does_not_trigger_for_other_activities() {
        let policy = MeetingMutePolicy;
        let context = EnhancedContext::default();
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
    fn test_meeting_mute_does_not_trigger_for_non_activity_events() {
        let policy = MeetingMutePolicy;
        let context = EnhancedContext::default();
        let event = DerivedEvent::IdleStateChanged {
            timestamp: Utc::now(),
            is_idle: true,
            idle_duration: None,
        };

        let result = policy.evaluate(&context, &event);
        assert!(result.is_none());
    }
}
