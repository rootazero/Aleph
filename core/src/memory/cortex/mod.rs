//! Cortex evolution system
//!
//! Implements the experience replay buffer and skill distillation pipeline
//! for evolving from "stateless executor" to "self-evolving agent".

pub mod clustering;
pub mod distillation;
pub mod dreaming;
pub mod integration;
pub mod pattern_extractor;
pub mod types;

pub use clustering::{Cluster, ClusteringConfig, ClusteringService};
pub use distillation::{
    DistillationConfig, DistillationPriority, DistillationService, PrioritizedTask,
};
pub use dreaming::{CortexDreamingConfig, CortexDreamingService, DreamingMetrics};
pub use integration::{CortexConfig, CortexIntegration};
pub use pattern_extractor::{ExtractedPattern, PatternExtractor, PatternExtractorConfig};
pub use types::{
    DistillationMode, DistillationTask, EnvironmentContext, EvolutionStatus, Experience,
    ExperienceBuilder, ParameterConfig, ParameterMapping, ReplayMatch,
};
