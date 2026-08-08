//! Configuration module for Aleph
//!
//! This module provides the configuration system for Aleph, including:
//! - `Config`: The main configuration struct with load/save/validate methods
//! - Type definitions in the `types` submodule
//!
//! Phase 1: Stub implementation with basic fields.
//! Phase 4: Added memory configuration support.
//! Phase 5: Added AI provider configuration support.
//! Phase 6: Added Keychain integration and file watching support.
//! Phase 8: Added config file loading from ~/.aleph/config.toml

// Submodules
pub mod agent_manager;
pub mod agent_resolver;
pub mod backup;
pub mod defaults_override;
pub mod guides;
pub mod live_apply;
mod load;
mod methods;
mod migration;
pub mod patcher;
pub mod presets_override;
pub mod reload_impact;
mod save;
pub mod schema;
mod structs;
pub mod types;
pub mod ui_hints;
mod validate;

// Re-export main types
pub use structs::{ChannelInstanceConfig, Config, PluginMarketplaceEntry};

// Re-export patcher types
pub use patcher::ConfigPatcher;

// Re-export reload-impact classifier (self-management SSOT) and the
// hot-apply that makes its `Live` verdict true.
pub use live_apply::classify_verified;
pub use reload_impact::ReloadImpact;

// Re-export schema generation functions
pub use schema::generate_config_schema_json;

// Re-export UI hints
pub use ui_hints::{build_ui_hints, ConfigUiHints};

// Re-export types for backward compatibility
pub use types::*;

// Tests
#[cfg(test)]
mod tests;
