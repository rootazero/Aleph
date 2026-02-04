//! EventCollection - Fluent API for filtering/aggregating events

use crate::daemon::events::DerivedEvent;
use chrono::Duration;
use rhai::FnPtr;

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

    /// Filter events using predicate
    pub fn filter(&self, _predicate: FnPtr) -> EventCollection {
        // TODO: Implement Rhai callback
        self.clone()
    }

    /// Sum duration of all events
    pub fn sum_duration(&self) -> Duration {
        // TODO: Implement
        Duration::zero()
    }

    /// Check if any event matches predicate
    pub fn any(&self, _predicate: FnPtr) -> bool {
        // TODO: Implement
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_collection_count() {
        let coll = EventCollection::empty();
        assert_eq!(coll.count(), 0);
    }

    #[test]
    fn test_event_collection_sum_duration() {
        let coll = EventCollection::empty();
        let dur = coll.sum_duration();
        assert_eq!(dur.num_seconds(), 0);
    }
}
