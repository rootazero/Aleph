//! EventCollection - Fluent API for filtering/aggregating events

use crate::daemon::events::DerivedEvent;
use crate::daemon::dispatcher::scripting::api::event::EventApi;
use chrono::Duration;
use rhai::{Engine, FnPtr, AST};

#[derive(Clone)]
pub struct EventCollection {
    events: Vec<DerivedEvent>,
}

impl EventCollection {
    pub fn new(events: Vec<DerivedEvent>) -> Self {
        Self { events }
    }

    pub fn empty() -> Self {
        Self { events: Vec::new() }
    }

    /// Count events in collection
    pub fn count(&self) -> i64 {
        self.events.len() as i64
    }

    /// Filter events using Rhai predicate
    pub fn filter(&self, engine: &Engine, ast: &AST, predicate: FnPtr) -> Result<EventCollection, Box<rhai::EvalAltResult>> {
        let mut filtered = Vec::new();

        for event in &self.events {
            let event_api = EventApi::new(event.clone());
            let result: bool = predicate.call(engine, ast, (event_api,))?;
            if result {
                filtered.push(event.clone());
            }
        }

        Ok(EventCollection::new(filtered))
    }

    /// Sum duration of all events
    pub fn sum_duration(&self) -> Duration {
        // For MVP, just return zero
        // TODO: Phase 5.2 - extract duration from events
        Duration::zero()
    }

    /// Check if any event matches predicate
    pub fn any(&self, engine: &Engine, ast: &AST, predicate: FnPtr) -> Result<bool, Box<rhai::EvalAltResult>> {
        for event in &self.events {
            let event_api = EventApi::new(event.clone());
            let result: bool = predicate.call(engine, ast, (event_api,))?;
            if result {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::events::DerivedEvent;
    use crate::daemon::worldmodel::state::ActivityType;
    use chrono::Utc;

    #[test]
    fn test_event_collection_count() {
        let events = vec![
            DerivedEvent::ActivityChanged {
                timestamp: Utc::now(),
                old_activity: ActivityType::Idle,
                new_activity: ActivityType::Programming {
                    language: Some("rust".to_string()),
                    project: None,
                },
                confidence: 0.9,
            },
        ];

        let coll = EventCollection::new(events);
        assert_eq!(coll.count(), 1);
    }

    #[test]
    fn test_event_collection_filter_coding() {
        let events = vec![
            DerivedEvent::ActivityChanged {
                timestamp: Utc::now(),
                old_activity: ActivityType::Idle,
                new_activity: ActivityType::Programming {
                    language: Some("rust".to_string()),
                    project: None,
                },
                confidence: 0.9,
            },
            DerivedEvent::ActivityChanged {
                timestamp: Utc::now(),
                old_activity: ActivityType::Programming {
                    language: None,
                    project: None,
                },
                new_activity: ActivityType::Idle,
                confidence: 1.0,
            },
        ];

        let coll = EventCollection::new(events);

        // TODO: Test filter with Rhai predicate
        // For now, just test count
        assert_eq!(coll.count(), 2);
    }
}
