//! Detection layer for intent classification.
//!
//! This module provides the 3-level classification system:
//! - L1: Regex matching (<5ms)
//! - L2: Keyword matching (<20ms)
//! - L3: AI classification (1-3s)

mod abort;
mod ai_binary;
pub mod ai_detector;
mod classifier;
pub mod keyword;
mod structural;

pub use abort::AbortDetector;
pub use ai_binary::{AiBinaryClassifier, AiBinaryConfig};
pub use ai_detector::{AiIntentDetector, AiIntentResult};
pub use classifier::{
    ExecutableTask, ExecutionIntent, IntentClassifier, IntentConfig, IntentContext,
    UnifiedIntentClassifier, UnifiedIntentClassifierBuilder, intent_type_to_category,
};
pub use keyword::{KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule};
pub use structural::{StructuralContext, StructuralDetector};
