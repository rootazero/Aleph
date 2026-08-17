//! Extension system errors

use crate::discovery::DiscoveryError;
use std::path::PathBuf;
use thiserror::Error;

/// Extension system errors.
///
/// Five variants previously lived here and were severed in the 2026-08-17
/// audit (sw-ext2-01): `YamlParse`, `MissingField`, `ConfigParse`, `NpmInstall`,
/// `TemplateError`. Their constructor helpers (`yaml_parse`, `missing_field`,
/// `config_parse`, `npm_install`, `template_error`) had zero callers in
/// `src/`, `src/bin/`, `interfaces/`, `shared/`.
#[derive(Debug, Error)]
pub enum ExtensionError {
    #[error("Discovery error: {0}")]
    Discovery(#[from] DiscoveryError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Invalid manifest in {path}: {message}")]
    InvalidManifest { path: PathBuf, message: String },

    #[error("Invalid plugin name '{name}': {reason}")]
    InvalidPluginName { name: String, reason: String },

    #[error("Plugin not found: {0}")]
    PluginNotFound(String),

    #[error("Skill not found: {0}")]
    SkillNotFound(String),

    #[error("Command not found: {0}")]
    CommandNotFound(String),

    #[error("Service not found: {0}")]
    ServiceNotFound(String),

    #[error("Hook execution error: {0}")]
    HookExecution(String),

    #[error("Runtime error: {0}")]
    Runtime(String),

    #[error("Plugin bridge error: {0}")]
    PluginBridge(String),

    #[error("File reference error in {path}: {message}")]
    FileReference { path: PathBuf, message: String },
}

pub type ExtensionResult<T> = Result<T, ExtensionError>;

impl ExtensionError {
    /// Create an invalid manifest error
    pub fn invalid_manifest(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::InvalidManifest {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Create an invalid plugin name error
    pub fn invalid_plugin_name(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidPluginName {
            name: name.into(),
            reason: reason.into(),
        }
    }

    /// Create a file reference error
    pub fn file_reference(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::FileReference {
            path: path.into(),
            message: message.into(),
        }
    }
}