//! Extension system errors

use crate::discovery::DiscoveryError;
use std::path::PathBuf;
use thiserror::Error;

/// Extension system errors
#[derive(Debug, Error)]
pub enum ExtensionError {
    #[error("Discovery error: {0}")]
    Discovery(#[from] DiscoveryError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("YAML parse error in {path}: {message}")]
    YamlParse { path: PathBuf, message: String },

    #[error("Invalid manifest in {path}: {message}")]
    InvalidManifest { path: PathBuf, message: String },

    #[error("Missing required field '{field}' in {path}")]
    MissingField { path: PathBuf, field: String },

    #[error("Invalid plugin name '{name}': {reason}")]
    InvalidPluginName { name: String, reason: String },

    #[error("Plugin not found: {0}")]
    PluginNotFound(String),

    #[error("Skill not found: {0}")]
    SkillNotFound(String),

    #[error("Command not found: {0}")]
    CommandNotFound(String),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Already registered: {0}")]
    AlreadyRegistered(String),

    #[error("Config parse error in {path}: {message}")]
    ConfigParse { path: PathBuf, message: String },

    #[error("Config merge error: {0}")]
    ConfigMerge(String),

    #[error("Hook execution error: {0}")]
    HookExecution(String),

    #[error("Runtime error: {0}")]
    Runtime(String),

    #[error("npm install failed for {package}: {message}")]
    NpmInstall { package: String, message: String },

    #[error("Plugin bridge error: {0}")]
    PluginBridge(String),

    #[error("Permission denied for skill: {0}")]
    PermissionDenied(String),

    #[error("Template error: {0}")]
    TemplateError(String),

    #[error("File reference error in {path}: {message}")]
    FileReference { path: PathBuf, message: String },
}

pub type ExtensionResult<T> = Result<T, ExtensionError>;

impl ExtensionError {
    /// Create a YAML parse error
    pub fn yaml_parse(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::YamlParse {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Create an invalid manifest error
    pub fn invalid_manifest(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::InvalidManifest {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Create a missing field error
    pub fn missing_field(path: impl Into<PathBuf>, field: impl Into<String>) -> Self {
        Self::MissingField {
            path: path.into(),
            field: field.into(),
        }
    }

    /// Create an invalid plugin name error
    pub fn invalid_plugin_name(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidPluginName {
            name: name.into(),
            reason: reason.into(),
        }
    }

    /// Create a config parse error
    pub fn config_parse(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::ConfigParse {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Create an npm install error
    pub fn npm_install(package: impl Into<String>, message: impl Into<String>) -> Self {
        Self::NpmInstall {
            package: package.into(),
            message: message.into(),
        }
    }

    /// Create a file reference error
    pub fn file_reference(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::FileReference {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Create a template error
    pub fn template_error(message: impl Into<String>) -> Self {
        Self::TemplateError(message.into())
    }
}
