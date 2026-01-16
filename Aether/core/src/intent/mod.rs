//! Intent detection module for AI-powered conversation flow.
//!
//! This module provides:
//! - **AiIntentDetector**: AI-powered detection for capability invocation
//! - **IntentClassifier**: Task classification for Agent execution mode
//!
//! # Architecture
//!
//! The module uses a single AI-first approach:
//! - **AiIntentDetector**: AI-powered detection for language-agnostic classification
//!
//! AI analyzes user input and decides whether to:
//! 1. Respond directly (general conversation)
//! 2. Request capability invocation (search, video, etc.)
//! 3. Ask for clarification (missing required parameters)
//!
//! # Example
//!
//! ```ignore
//! let detector = AiIntentDetector::new(provider);
//! let result = detector.detect("¿Cómo está el clima en Madrid?").await?;
//! // result.intent == "search"
//! // result.params["location"] == "Madrid"
//! ```

pub mod ai_detector;
pub mod classifier;
pub mod parameters;
pub mod task_category;

pub use ai_detector::{AiIntentDetector, AiIntentResult};
pub use classifier::{ExecutableTask, ExecutionIntent, IntentClassifier};
pub use parameters::{ConflictResolution, OrganizeMethod, ParameterSource, TaskParameters};
pub use task_category::TaskCategory;
