//! Intent detection module for AI-powered conversation flow.
//!
//! This module provides a three-layer classification system for determining
//! whether user input should trigger Agent mode (executable tasks) or
//! remain conversational.
//!
//! # Three-Layer Architecture
//!
//! ```text
//! User Input
//!     ↓
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L1: Regex Matching (<5ms)                                   │
//! │     - Fast pattern matching for explicit commands           │
//! │     - Confidence: 1.0                                       │
//! │     - Example: "整理文件夹里的文件" → FileOrganize           │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L2: Keyword Matching (<20ms)                                │
//! │     - KeywordIndex with weighted scoring                    │
//! │     - Supports CJK character tokenization                   │
//! │     - Configurable via KeywordPolicy in config.toml         │
//! │     - Confidence: 0.5-0.95 (based on score)                 │
//! │     - Fallback: Static keyword sets                         │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match)
//! ┌─────────────────────────────────────────────────────────────┐
//! │ L3: AI Classification (optional, 1-3s)                      │
//! │     - AiIntentDetector for complex/ambiguous cases          │
//! │     - Language-agnostic detection                           │
//! │     - Extracts parameters (path, location, etc.)            │
//! │     - Confidence: based on AI response                      │
//! └─────────────────────────────────────────────────────────────┘
//!     ↓ (no match or AI disabled)
//! ExecutionIntent::Conversational
//! ```
//!
//! # Module Structure
//!
//! - **detection/**: Intent detection (L1-L3 classification)
//! - **decision/**: Execution decision making and routing
//! - **parameters/**: Parameter types, defaults, and context
//! - **types/**: Core type definitions (TaskCategory, FFI)
//! - **support/**: Caching, rollback, and legacy prompts
//!
//! # Usage
//!
//! ```ignore
//! use alephcore::intent::IntentClassifier;
//! use alephcore::config::KeywordPolicy;
//!
//! // Basic usage (L1 + L2 only)
//! let classifier = IntentClassifier::new();
//! let intent = classifier.classify("帮我整理文件").await;
//!
//! // With keyword policy from config
//! let policy = KeywordPolicy::with_builtin_rules();
//! let classifier = IntentClassifier::with_keyword_policy(&policy);
//!
//! // With AI L3 enabled
//! let classifier = classifier.with_ai_provider(provider);
//! ```
//!
//! # Exclusion Patterns
//!
//! Inputs containing analysis/understanding verbs are excluded from Agent mode:
//! - Chinese: 分析, 理解, 解释, 总结, 摘要...
//! - English: analyze, understand, explain, summarize...
//!
//! This ensures requests like "分析这个文件" (analyze this file) are
//! handled conversationally rather than triggering file operations.

// Submodules
pub mod decision;
pub mod detection;
pub mod parameters;
pub mod support;
pub mod types;

// Re-export from detection
pub use detection::{
    AiIntentDetector, AiIntentResult, ExecutableTask, ExecutionIntent, IntentClassifier,
    KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule,
};

// Re-export from decision
pub use decision::{
    AggregatedIntent, AggregatorConfig, CalibratedSignal, CalibrationHistory, CalibratorConfig,
    ConfidenceCalibrator, ContextSignals, CustomInvocation, DeciderConfig, DecisionMetadata,
    DecisionResult, DirectMode, DirectRouteInfo, ExecutionIntentDecider, ExecutionMode,
    IntentAction, IntentAggregator, IntentLayer, IntentRouter, IntentSignal, McpInvocation,
    MissingParameter, RouteResult, RoutingLayer, SkillInvocation, SlashCommand, ThinkingContext,
    ToolInvocation,
};
// Backward compatibility
#[allow(deprecated)]
pub use decision::DecisionLayer;

// Re-export from parameters
pub use parameters::{
    AppContext, ConflictResolution, ConversationContext, DefaultsResolver, InputFeatures,
    MatchingContext, MatchingContextBuilder, OrganizeMethod, ParameterSource, PendingParam,
    PresetRegistry, ScenarioPreset, TaskParameters, TimeContext,
};

// Re-export from types
pub use types::TaskCategory;

// Re-export from support
pub use support::{
    AgentModePrompt, CacheConfig, CacheMetrics, CachedIntent, GenerationModelInfo, IntentCache,
    RollbackCapable, RollbackConfig, RollbackEntry, RollbackManager, RollbackResult,
    ToolDescription,
};
