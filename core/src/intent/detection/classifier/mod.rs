//! Intent classifier module.
//!
//! Provides the unified intent classification pipeline (v3).

mod unified;

pub use unified::{
    IntentConfig, IntentContext, UnifiedIntentClassifier, UnifiedIntentClassifierBuilder,
};
