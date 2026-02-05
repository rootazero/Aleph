//! Collaboration Module
//!
//! Provides Session-as-a-Service capabilities for multi-agent collaboration.
//!
//! # Components
//!
//! - `handle`: SessionHandle abstraction for subagent control
//! - `swapping`: Agent Swapping for memory optimization
//! - `coordinator`: SessionCoordinator for lifecycle management

mod coordinator;
mod handle;
mod swapping;

pub use coordinator::{CoordinatorConfig, SessionCounts, SessionCoordinator};
pub use handle::{SessionHandle, TaskResult};
pub use swapping::{SwapConfig, SwapManager, SwapResult, SwapStats, SwappedContext};
