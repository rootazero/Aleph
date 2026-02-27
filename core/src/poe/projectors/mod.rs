//! POE event projectors.
//!
//! Projectors consume events from PoeEventBus and project them into
//! various storage backends:
//! - CrystallizationProjector → LanceDB poe_experiences table
//! - TrustProjector → SQLite poe_trust_scores table (Task 9)
//! - MemoryProjector → Memory facts (Task 12)

pub mod crystallization;
