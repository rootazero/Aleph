//! Capability Registrar — dynamic registration API for plugins
//!
//! Provides `register_plugin_row` (the `registry_row` mount effect); the
//! crate-internal `CapabilityApi` behind it is the only writer of
//! capabilities into `PluginRegistry`.

pub mod api;
pub mod mcp_registrar;

pub(crate) use api::register_plugin_row;
