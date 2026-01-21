//! Plugin system error types
//!
//! Defines all error types used throughout the plugin system.

use std::path::PathBuf;
use thiserror::Error;

/// Plugin system errors
#[derive(Debug, Error)]
pub enum PluginError {
    /// Plugin directory not found
    #[error("Plugin directory not found: {0}")]
    DirectoryNotFound(PathBuf),

    /// Invalid plugin structure (missing .claude-plugin/plugin.json)
    #[error("Invalid plugin structure at {path}: {reason}")]
    InvalidStructure { path: PathBuf, reason: String },

    /// Failed to parse plugin manifest
    #[error("Failed to parse plugin.json at {path}: {source}")]
    ManifestParseError {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// Missing required field in manifest
    #[error("Missing required field '{field}' in plugin manifest at {path}")]
    MissingRequiredField { path: PathBuf, field: String },

    /// Failed to parse SKILL.md file
    #[error("Failed to parse SKILL.md at {path}: {reason}")]
    SkillParseError { path: PathBuf, reason: String },

    /// Failed to parse hooks.json
    #[error("Failed to parse hooks.json at {path}: {source}")]
    HooksParseError {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// Failed to parse agent definition
    #[error("Failed to parse agent at {path}: {reason}")]
    AgentParseError { path: PathBuf, reason: String },

    /// Failed to parse MCP configuration
    #[error("Failed to parse .mcp.json at {path}: {source}")]
    McpConfigParseError {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// Plugin not found
    #[error("Plugin not found: {0}")]
    PluginNotFound(String),

    /// Skill not found
    #[error("Skill not found: {plugin}:{skill}")]
    SkillNotFound { plugin: String, skill: String },

    /// Plugin already loaded
    #[error("Plugin already loaded: {0}")]
    AlreadyLoaded(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Hook execution error
    #[error("Hook execution failed: {0}")]
    HookExecutionError(String),

    /// Runtime not available
    #[error("Runtime not available: {runtime}. Ensure Aether has initialized runtimes.")]
    RuntimeNotAvailable { runtime: String },

    /// State persistence error
    #[error("Failed to persist plugin state: {0}")]
    StatePersistenceError(String),

    /// Integration error
    #[error("Failed to integrate plugin: {0}")]
    IntegrationError(String),
}

/// Result type for plugin operations
pub type PluginResult<T> = std::result::Result<T, PluginError>;
