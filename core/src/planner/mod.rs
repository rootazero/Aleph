//! Unified Planner Module
//!
//! This module implements the unified planner architecture that replaces
//! the previous 6-layer intent/dispatcher system with a simpler 2-layer
//! planner-executor architecture.
//!
//! # Architecture
//!
//! The unified planner takes user input and produces an `ExecutionPlan`
//! that can be one of three types:
//!
//! - **Conversational**: Pure conversation, no tools needed
//! - **SingleAction**: A single tool call or simple task
//! - **TaskGraph**: Complex multi-step task with dependencies
//!
//! # Usage
//!
//! ```ignore
//! use aether_core::planner::{ExecutionPlan, PlannedTask, PlannerError};
//!
//! // Create a simple conversational plan
//! let plan = ExecutionPlan::conversational();
//!
//! // Create a single action plan
//! let plan = ExecutionPlan::single_action(
//!     "read_file".to_string(),
//!     serde_json::json!({"path": "/tmp/test.txt"}),
//! );
//!
//! // Create a complex task graph
//! let tasks = vec![
//!     PlannedTask::new(0, "Step 1", task_type1),
//!     PlannedTask::new(1, "Step 2", task_type2),
//! ];
//! let plan = ExecutionPlan::task_graph(tasks, vec![(1, 0)]);
//! ```

mod types;

pub use types::{ExecutionPlan, PlannedTask, PlannerError};
