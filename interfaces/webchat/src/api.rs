// API layer for Gateway RPC methods
// Provides type-safe interfaces for interacting with the Gateway

// Existing submodules
pub mod agents;
pub mod chat;
pub mod cron;
pub mod heartbeat;
pub mod tool_permissions;

// Graph visualization
pub mod graph;

// Split submodules
pub mod acp;
pub mod agent_config;
pub mod agent_run;
pub mod browser;
pub mod config;
pub mod discord;
pub mod embedding;
pub mod generation_providers;
pub mod logs;
pub mod mcp;
pub mod memory;
pub mod memory_config;
pub mod providers;
pub mod routing;
pub mod search;
pub mod security;
pub mod settings;
pub mod system;
pub mod teams;
pub mod trace;
pub mod workspace;

// Re-export all public types for backward compatibility (crate::api::Foo)
pub use graph::*;
pub use acp::*;
pub use agent_config::*;
pub use agent_run::*;
pub use browser::*;
pub use config::*;
pub use discord::*;
pub use embedding::*;
pub use generation_providers::*;
pub use logs::*;
pub use mcp::*;
pub use memory::*;
pub use memory_config::*;
pub use providers::*;
pub use routing::*;
pub use search::*;
pub use security::*;
pub use settings::*;
pub use system::*;
pub use trace::*;
pub use workspace::*;
