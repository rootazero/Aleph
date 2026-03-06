//! SharedArena — multi-agent collaboration domain.
//!
//! This module provides the core domain types for SharedArena,
//! a structured workspace where multiple agents collaborate on a shared goal.

pub mod arena;
pub mod events;
pub mod handle;
pub mod types;

pub use arena::*;
pub use events::*;
pub use handle::*;
pub use types::*;
