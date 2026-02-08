//! Meta-cognition layer for self-reflection and behavioral learning
//!
//! This module implements Aleph's ability to observe, critique, and improve
//! its own thinking process through:
//! - Reactive reflection (pain learning from failures)
//! - Proactive reflection (excellence learning from optimization)
//! - Dynamic behavioral anchor injection

pub mod schema;
pub mod types;

pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
