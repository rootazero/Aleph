//! UniFFI core bindings - Re-exports from ffi module
//!
//! This module provides backward compatibility by re-exporting types from the
//! new modular `ffi` module structure.
//!
//! # Architecture
//!
//! The actual implementations are split across focused submodules:
//! - `ffi::mod` - Core AetherCore struct and initialization
//! - `ffi::processing` - AI processing methods (process, cancel, etc.)
//! - `ffi::tools` - Tool management (list_tools, add_mcp_tool, etc.)
//! - `ffi::memory` - Memory operations (search_memory, clear_memory, etc.)
//! - `ffi::config` - Configuration management (reload_config, update_provider, etc.)
//! - `ffi::skills` - Skills management (list_skills, install_skill, etc.)
//! - `ffi::mcp` - MCP server management (list_mcp_servers, add_mcp_server, etc.)
//! - `ffi::cowork` - Cowork task orchestration and model router
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::uniffi_core::{AetherCore, init_core};
//!
//! let handler = Box::new(MyHandler::new());
//! let core = init_core("~/.config/aether/config.toml", handler)?;
//!
//! core.process("Hello, world!".to_string(), None)?;
//! ```

// Re-export all public types from ffi module
pub use crate::ffi::{
    // Core types
    AetherCore,
    AetherEventHandler,
    AetherFfiError,
    // Processing types
    ProcessOptions,
    // Tool types
    ToolInfoFFI,
    // Memory types
    MemoryItem,
    // Initialization
    init_core,
};
