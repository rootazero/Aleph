//! Configuration module for Aether
//!
//! This module provides the configuration system for Aether, including:
//! - `Config`: The main configuration struct with load/save/validate methods
//! - `FullConfig`: FFI-compatible version for UniFFI
//! - Type definitions in the `types` submodule
//!
//! Phase 1: Stub implementation with basic fields.
//! Phase 4: Added memory configuration support.
//! Phase 5: Added AI provider configuration support.
//! Phase 6: Added Keychain integration and file watching support.
//! Phase 8: Added config file loading from ~/.aether/config.toml

// Submodules
mod structs;
mod load;
mod save;
mod validate;
mod migration;
mod methods;

pub mod types;
pub mod watcher;

// Re-export main types
pub use structs::{Config, FullConfig};

// Re-export types for backward compatibility
pub use types::*;

#[allow(unused_imports)]
pub use watcher::ConfigWatcher;

// Tests
#[cfg(test)]
mod tests;
