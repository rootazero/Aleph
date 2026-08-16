//! Configuration type definitions for Aleph
//!
//! This module contains all struct definitions used in the configuration system.
//! Types are organized by domain:
//!
//! - `general`: Core settings (`GeneralConfig`, `BehaviorConfig`)
//! - `provider`: AI provider settings (`ProviderConfig`)
//! - `routing`: Routing rules (`RoutingRuleConfig`)
//! - `memory`: Memory/RAG settings (`MemoryConfig`)
//! - `search`: Search capability settings (`SearchConfigInternal`)
//! - `tools`: Native and MCP tools (`ToolsConfig`, `UnifiedToolsConfig`)
//! - `orchestrator`: Three-Layer Control orchestrator settings (`OrchestratorConfig`, `OrchestratorGuards`)

pub mod acp;
pub mod agent;
pub mod agents_def;
pub mod desktop;
pub mod dispatcher;
pub mod execution;
pub mod fetch;
pub mod general;
pub mod generation;
pub mod group_chat;
pub mod memory;
pub mod moa;
pub mod orchestrator;
pub mod phase6_wiring;
pub mod policies;
pub mod privacy;
pub mod profile;
pub mod projects;
pub mod prompt;
pub mod provider;
pub mod resume;
pub mod route;
pub mod routing;
pub mod search;
pub mod secrets;
pub mod security;
pub mod serde_helpers;
pub mod stop_hooks;
pub mod tools;
pub mod voice_local;

// Re-export all types for backward compatibility
// Users can still use `use crate::config::XXX` instead of `use crate::config::types::XXX`
pub use acp::*;
pub use agent::*;
pub use agents_def::*;
pub use dispatcher::*;
pub use execution::*;
pub use fetch::*;
pub use general::*;
pub use generation::*;
pub use group_chat::*;
pub use memory::*;
pub use moa::*;
pub use orchestrator::*;
pub use phase6_wiring::*;
pub use policies::*;
pub use privacy::*;
pub use profile::*;
pub use projects::*;
pub use prompt::*;
pub use provider::*;
pub use resume::*;
pub use route::*;
pub use routing::*;
pub use search::*;
pub use secrets::*;
pub use security::*;
pub use stop_hooks::*;
pub use tools::*;
pub use voice_local::*;
