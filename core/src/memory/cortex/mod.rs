//! Cortex evolution system (DEPRECATED)
//!
//! **This module is deprecated.** Its capabilities have been absorbed by the POE module:
//! - Meta-cognition (anchors, critic, reflector) → `crate::poe::meta_cognition`
//! - Crystallization (distillation, clustering, dreaming) → `crate::poe::crystallization`
//!
//! This module is retained for backward compatibility during the transition period.
//! New code should use `crate::poe::meta_cognition` and `crate::poe::crystallization` instead.
//!
//! Original description:
//! Implements the experience replay buffer and skill distillation pipeline
//! for evolving from "stateless executor" to "self-evolving agent".

pub mod clustering;
pub mod distillation;
pub mod dreaming;
pub mod integration;
pub mod meta_cognition;
pub mod pattern_extractor;
pub mod types;

pub use clustering::{Cluster, ClusteringConfig, ClusteringService};
pub use distillation::{
    DistillationConfig, DistillationPriority, DistillationService, PrioritizedTask,
};
pub use dreaming::{CortexDreamingConfig, CortexDreamingService, DreamingMetrics};
pub use integration::{CortexConfig, CortexIntegration};
pub use meta_cognition::{AnchorScope, AnchorSource, BehavioralAnchor};
pub use pattern_extractor::{ExtractedPattern, PatternExtractor, PatternExtractorConfig};
pub use types::{
    DistillationMode, DistillationTask, EnvironmentContext, EvolutionStatus, Experience,
    ExperienceBuilder, ParameterConfig, ParameterMapping, ReplayMatch,
};
