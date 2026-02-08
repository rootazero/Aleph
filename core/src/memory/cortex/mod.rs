//! Cortex evolution system
//!
//! Implements the experience replay buffer and skill distillation pipeline
//! for evolving from "stateless executor" to "self-evolving agent".

pub mod distillation;
pub mod dreaming;
pub mod pattern_extractor;
pub mod types;

pub use distillation::{
    DistillationConfig, DistillationPriority, DistillationService, PrioritizedTask,
};
pub use dreaming::{CortexDreamingConfig, CortexDreamingService, DreamingMetrics};
pub use pattern_extractor::{ExtractedPattern, PatternExtractor, PatternExtractorConfig};
pub use types::{
    DistillationMode, DistillationTask, EnvironmentContext, EvolutionStatus, Experience,
    ExperienceBuilder, ParameterConfig, ParameterMapping, ReplayMatch,
};
