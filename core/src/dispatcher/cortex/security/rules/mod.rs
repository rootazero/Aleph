//! Built-in sanitizer rules
//!
//! This module contains the concrete implementations of `SanitizerRule`:
//!
//! - `TagInjectionRule` - Detects and neutralizes XML/HTML-style tag injection
//! - `PiiMaskerRule` - Masks personally identifiable information
//! - `InstructionOverrideRule` - Detects attempts to override system instructions

pub mod instruction_override;
pub mod pii_masker;
pub mod tag_injection;

pub use instruction_override::InstructionOverrideRule;
pub use pii_masker::PiiMaskerRule;
pub use tag_injection::TagInjectionRule;
