//! Resilient task execution framework.
//!
//! Provides three-level defense for task execution:
//! 1. Retry with exponential backoff
//! 2. Graceful degradation with fallback
//! 3. Notification on failure
//!
//! ## Example
//!
//! ```rust,ignore
//! use aethecore::resilient::{ResilientTask, ResilienceConfig, TaskOutcome};
//!
//! struct PodcastTask { /* ... */ }
//!
//! impl ResilientTask for PodcastTask {
//!     type Output = String;
//!
//!     async fn execute(&self, ctx: &TaskContext) -> Result<Self::Output> {
//!         // Try TTS generation
//!         generate_podcast_audio().await
//!     }
//!
//!     async fn fallback(&self, ctx: &TaskContext) -> Result<Self::Output> {
//!         // Fall back to markdown summary
//!         generate_markdown_summary().await
//!     }
//! }
//! ```

pub mod executor;
pub mod task;
pub mod types;

pub use executor::{execute_resilient, ResilientExecutor};
pub use task::{FnTask, ResilientTask};
pub use types::{
    classify_error, DegradationReason, DegradationStrategy, ErrorClass, ResilienceConfig,
    TaskContext, TaskOutcome,
};
