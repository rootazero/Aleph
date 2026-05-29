//! Atomic engine module for Aleph
//!
//! This module implements the atomic engine architecture inspired by OpenClaw's Pi engine,
//! with enhancements for L1/L2/L3 routing and self-healing execution.

pub mod atomic;
mod atomic_action;
mod atomic_executor;
#[allow(dead_code)]
mod classifier;
#[allow(dead_code)]
mod feature_extractor;
mod patch;
mod reflex_bench;
mod reflex_layer;

// NOTE: Disabled — AtomicEngine types have been removed. Tests need rewrite.
#[cfg(all(test, feature = "disabled-tests"))]
mod integration_tests;

// NOTE: Disabled — AtomicEngine types have been removed. Benchmarks need rewrite.
#[cfg(all(test, feature = "disabled-tests"))]
mod performance_benchmarks;

pub use atomic_action::{AtomicAction, FileFilter, SearchPattern, SearchScope};
pub(crate) use atomic_action::{LineRange, WriteMode};
pub use atomic_executor::AtomicExecutor;
pub use feature_extractor::{Entity, Intent};
pub use patch::Patch;
pub(crate) use patch::PatchApplier;
pub use reflex_layer::ActionType;
#[cfg(test)]
pub(crate) use reflex_layer::ReflexLayer;
