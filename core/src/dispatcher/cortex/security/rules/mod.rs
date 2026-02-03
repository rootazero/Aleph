//! Built-in sanitizer rules
//!
//! This module will contain the concrete implementations of `SanitizerRule`:
//!
//! - `TagInjectionRule` - Detects and neutralizes XML/HTML-style tag injection
//! - `PiiMaskerRule` - Masks personally identifiable information
//! - `InstructionOverrideRule` - Detects attempts to override system instructions
