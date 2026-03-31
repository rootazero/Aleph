//! SharedArena — multi-agent collaboration domain.
//!
//! This module provides the core domain types for SharedArena,
//! a structured workspace where multiple agents collaborate on a shared goal.

#[allow(dead_code)]
pub mod aggregate;
#[allow(dead_code)]
pub mod events;
#[allow(dead_code)]
pub mod handle;
pub mod manager;
#[allow(dead_code)]
pub mod storage;
pub mod types;

pub use manager::*;
pub use types::*;

#[cfg(test)]
mod integration_tests;
