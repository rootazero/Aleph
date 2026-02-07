//! Atomic engine module for Aleph
//!
//! This module implements the atomic engine architecture inspired by OpenClaw's Pi engine,
//! with enhancements for L1/L2/L3 routing and self-healing execution.

mod atomic_action;
mod atomic_engine;
mod atomic_executor;
mod patch;
mod reflex_bench;
mod reflex_layer;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod performance_benchmarks;

pub use atomic_action::{AtomicAction, LineRange, WriteMode};
pub use atomic_engine::{AtomicEngine, ExecutionResult, RoutingLayer, RoutingResult, RoutingStats};
pub use atomic_executor::AtomicExecutor;
pub use patch::{Patch, PatchApplier};
pub use reflex_layer::{KeywordRule, ReflexLayer};
