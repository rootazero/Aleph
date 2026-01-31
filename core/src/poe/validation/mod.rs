//! Validation subsystem for POE architecture.
//!
//! This module provides three types of validators:
//! - `HardValidator`: Deterministic checks (file existence, regex matching, commands)
//! - `SemanticValidator`: LLM-based quality evaluation
//! - `CompositeValidator`: Two-phase validation pipeline combining hard and semantic checks

pub mod composite;
pub mod hard;
pub mod semantic;

pub use composite::CompositeValidator;
pub use hard::HardValidator;
pub use semantic::SemanticValidator;
