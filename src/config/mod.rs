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
mod dead_keys;
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

// Re-export UI hints. The `build_ui_hints` producer was severed in the
// 2026-08-16 audit (zero consumers; `config.schema` constructs the DTO
// directly via `ConfigUiHints::new()`). Only the load-bearing DTO is exposed.
pub use ui_hints::ConfigUiHints;

// Re-export types for backward compatibility
pub use types::*;

// Tests
#[cfg(test)]
mod tests;
