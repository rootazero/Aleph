//! Plugin Registry Module
//!
//! This module provides the registration infrastructure for the Aleph plugin system.
//!
//! ## Components
//!
//! - [`ComponentRegistry`] - Manages loaded extension components (skills, commands, agents, plugins)
//! - [`PluginRegistry`] - Central storage for all plugin registrations
//! - Registration types - 9 types for plugin API registration
//!
//! ## Registration Categories
//!
//! - **P0 Core**: Essential types for basic plugin functionality
//!   - [`ToolRegistration`] - Expose callable tools to agents
//!   - [`HookRegistration`] - Intercept system events
//!
//! - **P1 Important**: Key integration points
//!   - [`ChannelRegistration`] - Messaging platform integrations
//!   - [`ProviderRegistration`] - AI model providers
//!   - [`GatewayMethodRegistration`] - RPC method extensions
//!
//! - **P2 Useful**: Additional extension points
//!   - [`HttpRouteRegistration`] - REST API endpoints
//!   - [`HttpHandlerRegistration`] - HTTP middleware
//!   - [`CliRegistration`] - CLI command extensions
//!   - [`ServiceRegistration`] - Background services
//!
//! - **P3 Optional**: Nice-to-have features
//!   - [`CommandRegistration`] - In-chat slash commands
//!
//! ## Diagnostics
//!
//! - [`PluginDiagnostic`] - Health reporting for plugins
//! - [`DiagnosticLevel`] - Severity levels (warn, error)

mod component_registry;
mod plugin_registry;
mod types;

pub use component_registry::*;
pub use plugin_registry::*;
pub use types::*;
