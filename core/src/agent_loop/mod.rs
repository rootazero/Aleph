//! Agent Loop - Core execution engine for Aether
//!
//! This module implements the Agent Loop architecture, a unified
//! observe-think-act-feedback cycle for executing user tasks.
//!
//! # Architecture
//!
//! ```text
//! User Request → IntentRouter (L0-L2) → Fast Path / Agent Loop
//!                                              ↓
//!                                    ┌─────────────────┐
//!                                    │  Guards Check   │
//!                                    │  Compress       │
//!                                    │  Think (LLM)    │
//!                                    │  Decide         │
//!                                    │  Execute        │
//!                                    │  Feedback       │
//!                                    │  ↑______↓       │
//!                                    └─────────────────┘
//! ```
//!
//! # Compaction Trigger Points
//!
//! The agent loop emits events at key points for SessionCompactor integration:
//!
//! 1. **Before each iteration**: Emit `LoopContinue` with current token count
//!    - SessionCompactor checks for overflow and triggers compaction if needed
//!
//! 2. **After tool execution**: Emit `ToolCallCompleted`
//!    - SessionCompactor triggers pruning check
//!
//! 3. **Session end**: Emit `LoopStop` with reason
//!    - SessionCompactor performs final cleanup/pruning
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::agent_loop::{AgentLoop, LoopConfig, NoOpLoopCallback};
//!
//! let config = LoopConfig::default();
//! let agent_loop = AgentLoop::new(thinker, executor, compressor, config);
//!
//! let result = agent_loop.run(
//!     "Search for Rust tutorials".to_string(),
//!     RequestContext::empty(),
//!     tools,
//!     NoOpLoopCallback,
//! ).await;
//! ```

// Submodules
pub mod answer;
pub mod callback;
pub mod config;
pub mod decision;
pub mod guards;
pub mod message_builder;
pub mod overflow;
pub mod question;
pub mod session_sync;
pub mod state;

mod compaction_trigger;
mod loop_result;
mod traits;
mod agent_loop;

// Re-export public types
pub use answer::UserAnswer;
pub use callback::{CollectingCallback, LoggingCallback, LoopCallback, LoopEvent, NoOpLoopCallback};
pub use config::{CompressionConfig, LoopConfig, ModelRoutingConfig, ThinkRetryConfig};
pub use decision::{Action, ActionResult, Decision, LlmAction, LlmResponse};
pub use question::{ChoiceOption, QuestionKind, TextValidation};
pub use guards::{GuardViolation, LoopGuard};
pub use message_builder::{Message, MessageBuilder, MessageBuilderConfig, ToolCall};
pub use overflow::{ModelLimit, OverflowConfig, OverflowDetector};
pub use session_sync::SessionSync;
pub use state::{LoopState, LoopStep, Observation, RequestContext, StepSummary, Thinking, ToolInfo};

// Re-export compaction trigger (useful for custom agent loop implementations)
pub use compaction_trigger::{CompactionTrigger, OptionalCompactionTrigger};

// Re-export loop result
pub use loop_result::LoopResult;

// Re-export traits
pub use traits::{ActionExecutor, CompressedHistory, CompressorTrait, ThinkerTrait};

// Re-export main agent loop
pub use agent_loop::AgentLoop;
