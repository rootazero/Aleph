//! TOML manifest parser for Aleph plugins (V2)
//!
//! This module parses the `aleph.plugin.toml` manifest format, which provides
//! a more ergonomic and feature-rich way to define Aleph plugins.
//!
//! # Example TOML Manifest
//!
//! ```toml
//! [plugin]
//! id = "my-plugin"
//! name = "My Plugin"
//! version = "1.0.0"
//! description = "A sample plugin"
//! kind = "wasm"
//! entry = "plugin.wasm"
//!
//! [permissions]
//! network = true
//! filesystem = "read"
//! env = false
//!
//! [prompt]
//! file = "SYSTEM.md"
//! scope = "system"
//!
//! [[tools]]
//! name = "my-tool"
//! description = "Does something useful"
//! handler = "handle_my_tool"
//!
//! [[hooks]]
//! event = "PreToolUse"
//! handler = "on_pre_tool_use"
//! ```

mod conversion;
mod parsing;
mod types;
mod wasm_capabilities;

#[cfg(test)]
mod tests;

/// TOML manifest filename
pub const ALEPH_PLUGIN_TOML: &str = "aleph.plugin.toml";

// Re-export all public types
pub use types::{
    AlephPluginToml, CapabilitiesSection, ChannelSection, CommandSection, FilesystemPermission,
    HookSection, HttpRouteSection, PermissionsSection, PluginAuthorToml, PluginSection,
    PromptSection, ProviderSection, ServiceSection, ToolSection,
};

// Re-export conversion functions
pub use conversion::convert_permissions;

// Re-export parsing functions
pub use parsing::{
    parse_aleph_plugin_toml, parse_aleph_plugin_toml_content, parse_aleph_plugin_toml_sync,
};

