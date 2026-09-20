//! Capability Registrar — dynamic registration API for plugins
//!
//! Provides `CapabilityApi` for writing capabilities into `PluginRegistry`.

pub mod api;
pub mod mcp_registrar;

pub(crate) use api::register_plugin_row;
pub use api::CapabilityApi;
