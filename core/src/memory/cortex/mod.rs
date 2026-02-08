//! Cortex evolution system
//!
//! Implements the experience replay buffer and skill distillation pipeline
//! for evolving from "stateless executor" to "self-evolving agent".

pub mod distillation;
pub mod types;

pub use distillation::{DistillationConfig, DistillationPriority, DistillationService, PrioritizedTask};
pub use types::{
    DistillationMode, DistillationTask, EnvironmentContext, EvolutionStatus, Experience,
    ExperienceBuilder, ParameterConfig, ParameterMapping, ReplayMatch,
};
