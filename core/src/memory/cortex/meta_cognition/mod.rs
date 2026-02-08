//! Meta-cognition layer for self-reflection and behavioral learning
//!
//! This module implements Aleph's ability to observe, critique, and improve
//! its own thinking process through:
//! - Reactive reflection (pain learning from failures)
//! - Proactive reflection (excellence learning from optimization)
//! - Dynamic behavioral anchor injection

pub mod anchor_store;
pub mod conflict_detector;
pub mod critic;
pub mod reactive;
pub mod schema;
pub mod types;

pub use anchor_store::AnchorStore;
pub use conflict_detector::{ConflictDetector, ConflictReport, ConflictType};
pub use critic::{ChainAnalysis, CriticAgent, CriticReport, CriticScanConfig};
pub use reactive::{
    FailureSignal, FailureSnapshot, LLMConfig, ReactiveReflector, ReflectionResult, RootCause,
};
pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
