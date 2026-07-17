//! Plugin Registry Module
//!
//! This module provides the registration infrastructure for the Aleph plugin system.
//!
//! ## Components
//!
//! - [`PluginRegistry`] - Central storage for all plugin registrations
//! - Registration types for plugin API registration
//!
//! ## Registration Types
//!
//! - [`ToolRegistration`] - Expose callable tools to agents
//! - [`HookRegistration`] - Intercept system events
//! - [`ServiceRegistration`] - Background services
//! - [`SkillRegistration`] - Prompt-based skills
//! - [`AgentRegistration`] - Agent definitions
//!
//! ## Diagnostics
//!
//! - [`PluginDiagnostic`] - Health reporting for plugins
//! - [`DiagnosticLevel`] - Severity levels (warn, error)

mod plugin_registry;
mod types;

pub use plugin_registry::*;
pub use types::*;
