//! EventApi - Wrapper for DerivedEvent exposed to Rhai

use crate::daemon::events::DerivedEvent;
use crate::daemon::worldmodel::state::ActivityType;
use chrono::Duration;

#[derive(Clone)]
pub struct EventApi {
    inner: DerivedEvent,
}

impl EventApi {
    pub fn new(event: DerivedEvent) -> Self {
        Self { inner: event }
    }

    /// Get activity as string (e.g., "Programming", "Meeting")
    pub fn activity(&self) -> String {
        match &self.inner {
            DerivedEvent::ActivityChanged { new_activity, .. } => {
                Self::activity_to_string(new_activity)
            }
            _ => "Unknown".to_string(),
        }
    }

    fn activity_to_string(activity: &ActivityType) -> String {
        match activity {
            ActivityType::Idle => "Idle".to_string(),
            ActivityType::Programming { .. } => "Programming".to_string(),
            ActivityType::Meeting { .. } => "Meeting".to_string(),
            ActivityType::Reading => "Reading".to_string(),
            ActivityType::Unknown => "Unknown".to_string(),
        }
    }

    /// Get event duration
    pub fn duration(&self) -> Duration {
        // TODO: Extract duration from event
        Duration::zero()
    }

    /// Check if event is coding activity
    pub fn is_coding(&self) -> bool {
        matches!(&self.inner, DerivedEvent::ActivityChanged {
            new_activity: ActivityType::Programming { .. },
            ..
        })
    }

    /// Check if event is idle
    pub fn is_idle(&self) -> bool {
        matches!(&self.inner, DerivedEvent::ActivityChanged {
            new_activity: ActivityType::Idle,
            ..
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_event_api_activity() {
        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Idle,
            new_activity: ActivityType::Programming {
                language: Some("rust".to_string()),
                project: None,
            },
            confidence: 0.9,
        };

        let api = EventApi::new(event);
        assert_eq!(api.activity(), "Programming");
        assert!(api.is_coding());
        assert!(!api.is_idle());
    }

    #[test]
    fn test_event_api_is_idle() {
        let event = DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Programming {
                language: None,
                project: None,
            },
            new_activity: ActivityType::Idle,
            confidence: 1.0,
        };

        let api = EventApi::new(event);
        assert_eq!(api.activity(), "Idle");
        assert!(api.is_idle());
        assert!(!api.is_coding());
    }
}
