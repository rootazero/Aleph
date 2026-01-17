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
//! # Components
//!
//! - **IntentClassifier**: Main entry point with `classify()` method
//! - **KeywordIndex**: Weighted keyword matching with CJK support
//! - **AiIntentDetector**: AI-powered fallback detection
//! - **TaskCategory**: Enumeration of executable task types
//!
//! # Usage
//!
//! ```ignore
//! use aethecore::intent::IntentClassifier;
//! use aethecore::config::KeywordPolicy;
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

pub mod agent_prompt;
pub mod ai_detector;
pub mod cache;
pub mod calibrator;
pub mod classifier;
pub mod context;
pub mod defaults;
pub mod ffi;
pub mod keyword;
pub mod parameters;
pub mod presets;
pub mod task_category;

pub use agent_prompt::{AgentModePrompt, ToolDescription};
pub use ai_detector::{AiIntentDetector, AiIntentResult};
pub use classifier::{ExecutableTask, ExecutionIntent, IntentClassifier};
pub use defaults::DefaultsResolver;
pub use keyword::{KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule};
pub use ffi::{
    AmbiguousTaskFFI, ConflictResolutionFFI, ExecutableTaskFFI, ExecutionIntentTypeFFI,
    OrganizeMethodFFI, ParameterSourceFFI, TaskCategoryFFI, TaskParametersFFI,
};
pub use parameters::{ConflictResolution, OrganizeMethod, ParameterSource, TaskParameters};
pub use presets::{PresetRegistry, ScenarioPreset};
pub use task_category::TaskCategory;
pub use cache::{CacheConfig, CacheMetrics, CachedIntent, IntentCache};
pub use calibrator::{
    CalibrationHistory, CalibratedSignal, CalibratorConfig, ConfidenceCalibrator, IntentSignal,
    RoutingLayer,
};
pub use context::{
    AppContext, ConversationContext, InputFeatures, MatchingContext, MatchingContextBuilder,
    PendingParam, TimeContext,
};
