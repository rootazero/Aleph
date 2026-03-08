//! Intent detection layers.
//!
//! Provides abort detection, structural detection, AI binary classification,
//! keyword matching, inline directive extraction, and the unified classifier pipeline.

mod abort;
mod ai_binary;
mod classifier;
pub mod directive;
pub mod keyword;
mod structural;

pub use abort::AbortDetector;
pub use ai_binary::{AiBinaryClassifier, AiBinaryConfig};
pub use classifier::{
    IntentConfig, IntentContext, UnifiedIntentClassifier, UnifiedIntentClassifierBuilder,
};
pub use directive::{Directive, DirectiveDefinition, DirectiveParser, ParsedInput};
pub use keyword::{KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule};
pub use structural::{StructuralContext, StructuralDetector};
