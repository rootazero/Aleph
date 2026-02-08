//! Atomic engine module for Aleph
//!
//! This module implements the atomic engine architecture inspired by OpenClaw's Pi engine,
//! with enhancements for L1/L2/L3 routing and self-healing execution.

mod atomic_action;
mod atomic_engine;
mod atomic_executor;
mod classifier;
mod conflict_detector;
mod feature_extractor;
mod learning_agent;
mod patch;
mod persistence;
mod reflex_bench;
mod reflex_layer;
mod rule_learner;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod performance_benchmarks;

pub use atomic_action::{AtomicAction, LineRange, WriteMode, SearchPattern, SearchScope, FileFilter};
pub use atomic_engine::{AtomicEngine, ExecutionResult, RoutingLayer, RoutingResult, RoutingStats};
pub use atomic_executor::AtomicExecutor;
pub use classifier::{NaiveBayesClassifier, ActionClass};
pub use conflict_detector::{ConflictDetector, Conflict, ConflictType, ConflictSeverity, ConflictReport, ConflictResolver, ResolutionStrategy};
pub use feature_extractor::{FeatureExtractor, FeatureVector, Intent, Entity};
pub use learning_agent::{LearningAgent, LearningEvent, AgentStats};
pub use patch::{Patch, PatchApplier};
pub use persistence::{Persistence, LearnedPattern, RuleMetadata, PersistenceStats};
pub use reflex_layer::{KeywordRule, ReflexLayer, ActionType, ParamExtractor};
pub use rule_learner::{RuleLearner, LearnerStats};
