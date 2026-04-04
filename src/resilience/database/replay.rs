//! Replay module for state recovery
//!
//! Provides replay functionality for recovering agent state after crashes.

use crate::resilience::database::StateDatabase;

/// ReplayService handles replaying events from the database
pub struct ReplayService {
    db: StateDatabase,
}

impl ReplayService {
    pub fn new(db: StateDatabase) -> Self {
        Self { db }
    }
}
