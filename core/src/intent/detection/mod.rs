//! Detection layer for intent classification.
//!
//! This module provides the 3-level classification system:
//! - L1: Regex matching (<5ms)
//! - L2: Keyword matching (<20ms)
//! - L3: AI classification (1-3s)

pub mod ai_detector;
pub mod classifier;
pub mod keyword;

pub use ai_detector::{AiIntentDetector, AiIntentResult};
pub use classifier::{ExecutableTask, ExecutionIntent, IntentClassifier};
pub use keyword::{KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule};
