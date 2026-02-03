//! Cortex 2.0 - Robust dispatcher internals
//!
//! This module provides hardened infrastructure for the dispatcher layer,
//! implementing the Strangler Fig pattern to incrementally replace legacy
//! components with improved implementations.
//!
//! # Components
//!
//! - `parser`: Streaming JSON extraction with repair capabilities
//! - `security`: Input sanitization pipeline (PII masking, injection detection)
//! - `decision`: Unified configuration management
//! - `budget`: Token counting and RAG fallback strategies
//!
//! # Design Principles
//!
//! 1. **Graceful Degradation**: Every operation has a fallback path
//! 2. **Structured Errors**: Rich error types with recovery hints
//! 3. **Security First**: All inputs sanitized before processing
//! 4. **Observable**: Comprehensive metrics and logging
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::cortex::{CortexError, RecoveryHint, SecuritySeverity};
//!
//! fn handle_error(err: CortexError) {
//!     match err.recovery_hint() {
//!         RecoveryHint::RetryWithRepair => { /* attempt JSON repair */ }
//!         RecoveryHint::FallbackToChat => { /* switch to conversational mode */ }
//!         RecoveryHint::Abort => { /* report error to user */ }
//!         _ => {}
//!     }
//! }
//! ```

pub mod decision;
pub mod error;
pub mod parser;
pub mod security;

pub use decision::{
    merge_config, ConfirmationConfig, ConfirmationOverride, DecisionAction, DecisionConfig,
    ExecutionConfig, RoutingConfig, RoutingOverride, SessionOverride,
};
pub use error::{CortexError, RecoveryHint, SecuritySeverity};
pub use parser::{JsonFragment, JsonStreamDetector};
pub use security::{
    Locale, PipelineResult, SanitizeAction, SanitizeContext, SanitizeResult, SanitizerRule,
    SecurityConfig, SecurityPipeline, TrustLevel,
};
