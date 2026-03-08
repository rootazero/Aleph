//! Intent detection module for AI-powered conversation flow.
//!
//! This module provides a unified intent classification pipeline that determines
//! whether user input should be aborted, routed to a direct tool, executed as
//! a task, or handled conversationally.
//!
//! # Unified Pipeline Architecture (v3)
//!
//! ```text
//! User Input
//!     ↓
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L0: Abort Detection (<1ms)                                   │
//! │     - Exact-match stop words (multilingual)                  │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (not aborted)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L1: Slash Command Detection (<1ms)                           │
//! │     - Built-in commands (/screenshot, /ocr, /search, etc.)   │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L2: Structural Detection (<5ms)                              │
//! │     - Paths, URLs, context signals                           │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L3: Keyword Matching (<20ms)                                 │
//! │     - KeywordIndex with weighted scoring                     │
//! │     - Supports CJK character tokenization                    │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L4: Default Fallback                                         │
//! │     - Execute or Converse depending on configuration         │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Module Structure
//!
//! - **detection/**: Intent detection (abort, structural, keyword, AI, unified classifier)
//! - **decision/**: Execution decision making, calibration, and routing
//! - **parameters/**: Parameter types, defaults, and context
//! - **types/**: Core type definitions (TaskCategory, IntentResult)
//! - **support/**: Caching, rollback, and prompt templates

// Submodules
pub mod decision;
pub mod detection;
pub mod parameters;
pub mod support;
pub mod types;

// Re-export from detection: unified pipeline (primary API)
pub use detection::{
    IntentContext, KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule, StructuralContext,
    UnifiedIntentClassifier, UnifiedIntentClassifierBuilder,
};

// Re-export from detection: inline directive parser
pub use detection::{Directive, DirectiveDefinition, DirectiveParser, ParsedInput};

// Re-export from detection: legacy types (still used internally by parameters module)
pub use detection::{ExecutableTask, ExecutionIntent, IntentClassifier};
// Re-export from detection: AI detector (used internally by old classifier)
pub use detection::{AiIntentDetector, AiIntentResult};

// Re-export from types: new pipeline result types
pub use types::{DetectionLayer, DirectToolSource, ExecuteMetadata, IntentResult};

// Re-export from types: shared
pub use types::TaskCategory;

// Re-export from decision: execution mode types (used by gateway/inbound_router)
pub use decision::{
    CustomInvocation, ExecutionMode, McpInvocation, SkillInvocation, ToolInvocation,
};

// Re-export from decision: calibrator (used by unified classifier)
pub use decision::{
    CalibratedSignal, CalibrationHistory, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};

// Re-export from parameters
pub use parameters::{
    AppContext, ConflictResolution, ConversationContext, DefaultsResolver, InputFeatures,
    MatchingContext, MatchingContextBuilder, OrganizeMethod, ParameterSource, PendingParam,
    PresetRegistry, ScenarioPreset, TaskParameters, TimeContext,
};

// Re-export from support
pub use support::{
    AgentModePrompt, CacheConfig, CacheMetrics, CachedIntent, GenerationModelInfo, IntentCache,
    RollbackCapable, RollbackConfig, RollbackEntry, RollbackManager, RollbackResult,
    ToolDescription,
};
