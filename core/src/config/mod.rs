//! Configuration module for Aleph
//!
//! This module provides the configuration system for Aleph, including:
//! - `Config`: The main configuration struct with load/save/validate methods
//! - `FullConfig`: Complete configuration for serialization
//! - Type definitions in the `types` submodule
//!
//! Phase 1: Stub implementation with basic fields.
//! Phase 4: Added memory configuration support.
//! Phase 5: Added AI provider configuration support.
//! Phase 6: Added Keychain integration and file watching support.
//! Phase 8: Added config file loading from ~/.aleph/config.toml

// Submodules
mod structs;
mod load;
mod save;
mod validate;
mod migration;
mod methods;
pub mod diff;
pub mod reload;

pub mod schema;
pub mod types;
pub mod ui_hints;
pub mod watcher;

// Re-export main types
pub use structs::{Config, FullConfig};

// Re-export schema generation functions
pub use schema::generate_config_schema_json;

// Re-export UI hints
pub use ui_hints::{build_ui_hints, ConfigUiHints};

// Re-export types for backward compatibility
pub use types::*;

#[allow(unused_imports)]
pub use watcher::ConfigWatcher;

// Tests
#[cfg(test)]
mod tests;
