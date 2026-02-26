//! Agent Loop - Core execution engine for Aleph
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
//! use alephcore::agent_loop::{AgentLoop, LoopConfig, NoOpLoopCallback};
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
pub mod bootstrap;
pub mod builder;
pub mod callback;
pub mod config;
pub mod context_provider;
pub mod decision;
pub mod events;
pub mod guards;
pub mod message_builder;
pub mod meta_cognition_integration;
pub mod overflow;
pub mod question;
pub mod reply_normalizer;
pub mod session_sync;
pub mod state;
pub mod thinking;

mod compaction_trigger;
mod cortex_telemetry;
mod loop_result;
mod traits;
#[allow(clippy::module_inception)]
mod agent_loop;

#[cfg(feature = "cli")]
pub mod callback_cli;

// Re-export public types
pub use answer::UserAnswer;
pub use builder::AgentLoopBuilder;
pub use callback::{CollectingCallback, LoggingCallback, LoopCallback, LoopEvent, NoOpLoopCallback};
pub use config::{CompressionConfig, LoopConfig, ModelRoutingConfig, ThinkRetryConfig};
pub use context_provider::ContextProvider;
pub use decision::{Action, ActionResult, Decision, LlmAction, LlmResponse};
pub use events::{AgentLoopEvent, InsightSeverity};
pub use question::{ChoiceOption, QuestionKind, TextValidation};
pub use guards::{GuardViolation, LoopGuard};
pub use message_builder::{Message, MessageBuilder, MessageBuilderConfig, ToolCall};
pub use meta_cognition_integration::{MetaCognitionConfig, MetaCognitionIntegration};
pub use overflow::{ModelLimit, OverflowConfig, OverflowDetector};
pub use session_sync::SessionSync;
pub use state::{LoopState, LoopStep, Observation, RequestContext, StepSummary, Thinking, ToolInfo};
pub use thinking::{ConfidenceLevel, ReasoningStep, ReasoningStepType, StructuredThinking, ThinkingParser};

// Re-export compaction trigger (useful for custom agent loop implementations)
pub use compaction_trigger::{CompactionTrigger, OptionalCompactionTrigger};

// Re-export cortex telemetry
pub use cortex_telemetry::{CortexTelemetry, ExecutionTelemetry, OptionalCortexTelemetry};

// Re-export loop result
pub use loop_result::LoopResult;

// Re-export traits
pub use traits::{ActionExecutor, CompressedHistory, CompressorTrait, ThinkerTrait};

// Re-export main agent loop
pub use agent_loop::{AgentLoop, RunContext};

// Re-export CLI callback (when cli feature is enabled)
#[cfg(feature = "cli")]
pub use callback_cli::CliLoopCallback;
