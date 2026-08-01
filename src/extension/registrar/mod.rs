//! Capability Registrar — dynamic registration API for plugins
//!
//! Provides `CapabilityApi` for writing capabilities into `PluginRegistry`.

pub mod api;
pub mod mcp_registrar;

pub use api::CapabilityApi;
