//! Atomic engine module for Aleph
//!
//! This module implements the atomic engine architecture inspired by OpenClaw's Pi engine,
//! with enhancements for L1/L2/L3 routing and self-healing execution.

pub mod atomic;
mod atomic_action;
mod atomic_engine;
mod atomic_executor;
mod classifier;
mod conflict_detector;
mod feature_extractor;
mod learning_agent;
mod patch;
mod performance_optimizer;
mod persistence;
mod reflex_bench;
mod reflex_layer;
mod rule_learner;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod performance_benchmarks;

pub use atomic_action::{AtomicAction, SearchPattern, SearchScope, FileFilter};
pub(crate) use atomic_action::{LineRange, WriteMode};
pub use atomic_engine::{ExecutionResult, RoutingLayer};
pub(crate) use atomic_engine::{AtomicEngine, RoutingResult, RoutingStats};
pub use atomic_executor::AtomicExecutor;
pub(crate) use classifier::{NaiveBayesClassifier, ActionClass};
pub use conflict_detector::{ConflictDetector, Conflict, ConflictType, ConflictReport, ConflictResolver, ResolutionStrategy};
pub(crate) use conflict_detector::ConflictSeverity;
pub use feature_extractor::{Intent, Entity};
pub(crate) use feature_extractor::{FeatureExtractor, FeatureVector};
pub(crate) use learning_agent::{LearningAgent, LearningEvent, AgentStats};
pub use patch::Patch;
pub(crate) use patch::PatchApplier;
pub use performance_optimizer::CacheStats;
pub(crate) use performance_optimizer::{PerformanceOptimizer, CachedResult, IndexStats};
pub use persistence::Persistence;
pub(crate) use persistence::{LearnedPattern, RuleMetadata, PersistenceStats};
pub use reflex_layer::ActionType;
pub(crate) use reflex_layer::{KeywordRule, ReflexLayer, ParamExtractor};
pub(crate) use rule_learner::{RuleLearner, LearnerStats};
