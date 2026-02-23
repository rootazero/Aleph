//! MCP Manager Module
//!
//! Provides the McpManager actor for orchestrating multiple MCP server connections.
//!
//! # Architecture
//!
//! The MCP Manager is implemented as an actor pattern:
//! - `McpManagerHandle` - Public API for interacting with the manager
//! - `McpManagerActor` - Internal actor that processes commands
//! - `McpCommand` - Command enum for actor communication
//! - `McpManagerEvent` - Events emitted by the manager
//! - `McpPersistentConfig` - Configuration persistence layer
//!
//! # Features
//!
//! - **Server Lifecycle**: Add, remove, start, stop, restart servers
//! - **Health Monitoring**: Circuit breaker pattern with automatic restarts
//! - **Tool Aggregation**: Unified view of tools from all servers
//! - **Configuration Persistence**: Save/load server configurations with env var expansion
//! - **Event Broadcasting**: Notify subscribers of state changes
//!
//! # Example
//!
//! ```ignore
//! use alephcore::mcp::manager::{McpManagerActor, McpManagerHandle, McpManagerConfig};
//!
//! // Create the actor (loads config from default path)
//! let (actor, handle) = McpManagerActor::new(None).await?;
//!
//! // Spawn the actor task
//! tokio::spawn(actor.run());
//!
//! // Add a server
//! let server_config = McpManagerConfig::stdio("my-server", "My Server", "npx")
//!     .with_args(vec!["@modelcontextprotocol/server-filesystem".to_string()])
//!     .with_runtime("node");
//!
//! handle.add_server(server_config).await?;
//!
//! // List all servers
//! let servers = handle.list_servers().await?;
//!
//! // Subscribe to events
//! let mut events = handle.subscribe();
//! while let Ok(event) = events.recv().await {
//!     println!("Event: {:?}", event);
//! }
//!
//! // Graceful shutdown
//! handle.shutdown().await?;
//! ```

mod actor;
mod config;
mod handle;
mod types;

pub use actor::{HealthCheckConfig, McpManagerActor};
pub use config::McpPersistentConfig;
pub use handle::McpManagerHandle;
pub use types::{
    HealthStatus, McpCommand, McpManagerConfig, McpManagerEvent, McpServerInfo,
    McpServerStatusDetail, McpTransportType, ServerHealth,
};
