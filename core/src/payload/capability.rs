//! Capability re-export for backward compatibility
//!
//! The `Capability` enum has been moved to `crate::core::capability` to break
//! the circular dependency between payload/ and capability/ modules.
//!
//! This module re-exports `Capability` to maintain backward compatibility
//! for code that imports from `crate::payload::Capability`.

pub use crate::core::Capability;
