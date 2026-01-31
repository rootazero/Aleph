//! POE (Principle-Operation-Evaluation) Architecture
//!
//! A goal-oriented agent execution framework that:
//! 1. **Principle**: Defines success criteria before execution (SuccessManifest)
//! 2. **Operation**: Executes with heuristic guidance (Worker abstraction)
//! 3. **Evaluation**: Validates results with mixed hard/semantic checks

pub mod budget;
pub mod manager;
pub mod types;
pub mod validation;
pub mod worker;

// Re-exports will be added after implementing each module
