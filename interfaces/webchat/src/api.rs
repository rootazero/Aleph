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

// Cluster API
pub mod cluster;

// Split submodules
pub mod acp;
pub mod artifacts;
pub mod browser;
pub mod clarification;
pub mod config;
pub mod discord;
pub mod embedding;
pub mod exec_approval;
pub mod extensions;
pub mod fetch;
pub mod fs;
pub mod generation_providers;
pub mod logs;
pub mod mcp;
pub mod memory;
pub mod memory_config;
pub mod moa;
pub mod projects;
pub mod providers;
pub mod routing;
pub mod runtimes;
pub mod search;
pub mod security;
pub mod sessions;
pub mod settings;
pub mod system;
pub mod team_chat;
pub mod teams;
pub mod trace;
pub mod workspace;

// Re-export all public types for backward compatibility (crate::api::Foo)
pub use acp::*;
pub use browser::*;
pub use clarification::*;
pub use config::*;
pub use discord::*;
pub use embedding::*;
pub use exec_approval::*;
pub use extensions::*;
pub use fetch::*;
pub use generation_providers::*;
pub use graph::*;
pub use logs::*;
pub use mcp::*;
pub use memory::*;
pub use memory_config::*;
pub use projects::*;
pub use providers::*;
pub use routing::*;
pub use runtimes::*;
pub use search::*;
pub use security::*;
pub use settings::*;
pub use system::*;
pub use trace::*;
pub use workspace::*;
