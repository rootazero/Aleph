//! SharedArena — multi-agent collaboration domain.
//!
//! This module provides the core domain types for SharedArena,
//! a structured workspace where multiple agents collaborate on a shared goal.

pub mod aggregate;
pub mod handle;
pub mod manager;
pub mod types;

pub use manager::*;
pub use types::*;

#[cfg(test)]
mod integration_tests;
