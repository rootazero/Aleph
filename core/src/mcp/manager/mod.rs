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
//!
//! # Features
//!
//! - **Server Lifecycle**: Add, remove, start, stop, restart servers
//! - **Health Monitoring**: Circuit breaker pattern with automatic restarts
//! - **Tool Aggregation**: Unified view of tools from all servers
//! - **Configuration Persistence**: Save/load server configurations
//! - **Event Broadcasting**: Notify subscribers of state changes

mod types;

pub use types::{
    HealthStatus, McpCommand, McpManagerConfig, McpManagerEvent, McpServerInfo,
    McpServerStatusDetail, McpTransportType, ServerHealth,
};
