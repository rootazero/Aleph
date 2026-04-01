//! PII (Personally Identifiable Information) filtering engine
//!
//! Gateway-level privacy protection that filters outbound messages
//! before they reach LLM API providers.
//!
//! Unlike `utils::pii::scrub_pii()` (which is optimized for log scrubbing
//! and accepts false positives), this engine is tuned for precision —
//! false positives degrade LLM comprehension.

pub mod allowlist;
pub mod engine;
pub mod rules;

pub use engine::{FilterResult, PiiEngine, PiiMatch, PiiSeverity};
pub use rules::PiiRule;
