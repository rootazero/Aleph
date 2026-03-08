//! Intent detection module for AI-powered conversation flow.
//!
//! This module provides a unified intent classification pipeline that determines
//! whether user input should be aborted, routed to a direct tool, executed as
//! a task, or handled conversationally.
//!
//! # Pipeline Architecture
//!
//! ```text
//! User Input -> DirectiveParser -> Abort -> L0 Commands -> L1 Structural -> L2 Keywords -> L3 AI -> L4 Default
//! ```

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

// Re-export from detection: directive parser
pub use detection::{Directive, DirectiveDefinition, DirectiveParser, ParsedInput};

// Re-export from types: pipeline result types
pub use types::{DetectionLayer, DirectToolSource, ExecuteMetadata, IntentResult};

// Re-export from types: shared
pub use types::TaskCategory;

// Re-export from decision: calibrator
pub use decision::{
    CalibratedSignal, CalibrationHistory, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};

// Re-export from parameters
pub use parameters::{
    AppContext, ConflictResolution, ConversationContext, DefaultsResolver, InputFeatures,
    MatchingContext, MatchingContextBuilder, OrganizeMethod, ParameterSource, PendingParam,
    TaskParameters, TimeContext,
};

// Re-export from support
pub use support::{
    AgentModePrompt, CacheConfig, CacheMetrics, CachedIntent, GenerationModelInfo, IntentCache,
    RollbackCapable, RollbackConfig, RollbackEntry, RollbackManager, RollbackResult,
    ToolDescription,
};
