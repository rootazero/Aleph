//! Cowork type definitions
//!
//! This module defines the core data structures for task orchestration:
//! - Task: Individual unit of work
//! - TaskGraph: DAG of tasks with dependencies
//! - TaskStatus: Execution state tracking
//! - TaskResult: Execution output

mod task;
mod graph;
mod result;

pub use task::*;
pub use graph::*;
pub use result::*;
