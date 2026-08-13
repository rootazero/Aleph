//! PII (Personally Identifiable Information) filtering engine
//!
//! Gateway-level privacy protection that filters outbound messages
//! before they reach LLM API providers.
//!
//! Unlike `aleph_logging::scrub_pii()` (which is optimized for log scrubbing
//! and accepts false positives), this engine is tuned for precision —
//! false positives degrade LLM comprehension.

pub mod allowlist;
pub mod engine;
pub mod rules;

/// The configuration vocabulary this module's public API is written in.
///
/// These are not decoration: `PiiEngine::new` / `init` / `reload` all take a
/// `PrivacyConfig`, and `PiiAction` is the type of every decision the engine
/// reports, so nothing outside this module can call it without naming them.
/// They were removed once as "dead re-exports (zero callers anywhere in src/ or
/// tests/)" — a claim `cargo check` and `cargo test --lib` both agree with,
/// because neither compiles `tests/`, and `tests/security_integration.rs` had
/// been importing all three from here since it was written. The whole
/// `--all-targets` build has not compiled since.
///
/// Re-exporting rather than moving keeps `crate::config` the one definition;
/// this is a second name for it, not a second copy.
pub use crate::config::{PiiAction, PlatformPiiPolicy, PrivacyConfig};
pub use engine::{FilterResult, PiiEngine, PiiMatch, PiiSeverity};
pub use rules::PiiRule;
