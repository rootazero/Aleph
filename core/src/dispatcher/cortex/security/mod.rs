//! SecurityPipe - Input sanitization pipeline
//!
//! This module provides a security sanitization pipeline for processing
//! user inputs before they reach the LLM. It supports:
//!
//! - Tag injection detection and neutralization
//! - PII masking (phone numbers, emails, ID numbers)
//! - Instruction override detection
//! - Confidence penalties for suspicious patterns
//!
//! # Architecture
//!
//! The pipeline applies rules in priority order. Each rule can:
//! - Pass input unchanged
//! - Mask sensitive content
//! - Escape special characters
//! - Apply confidence penalties
//! - Block input entirely
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::dispatcher::cortex::security::{
//!     SecurityPipeline, SecurityConfig, SanitizeContext,
//! };
//!
//! let mut pipeline = SecurityPipeline::new(SecurityConfig::default_enabled());
//! // Add rules...
//!
//! let ctx = SanitizeContext::default();
//! let result = pipeline.process("user input here", &ctx);
//!
//! if result.blocked {
//!     println!("Input blocked: {:?}", result.block_reason);
//! } else {
//!     println!("Sanitized: {}", result.text);
//! }
//! ```

pub mod rules;
pub mod sanitizer;

pub use sanitizer::{
    Locale, PipelineResult, SanitizeAction, SanitizeContext, SanitizeResult, SanitizerRule,
    SecurityConfig, SecurityPipeline, TrustLevel,
};
