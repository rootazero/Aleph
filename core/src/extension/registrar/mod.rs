//! Capability Registrar — dynamic registration API for plugins
//!
//! Provides `CapabilityApi` for writing capabilities into `PluginRegistry`,
//! and `CapabilityRegistrar` trait for runtime plugins.

pub mod api;
pub mod mcp_registrar;
pub mod wasm_registrar;

pub use api::CapabilityApi;

use anyhow::Result;

/// Trait for runtime plugins that register capabilities dynamically.
pub trait CapabilityRegistrar: Send + Sync {
    fn register(&self, api: &mut CapabilityApi) -> Result<()>;
    fn unregister(&self, api: &mut CapabilityApi) -> Result<()>;
}
