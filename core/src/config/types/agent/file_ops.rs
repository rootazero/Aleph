//! File operations executor configuration
//!
//! Contains FileOpsConfigToml for configuring file system operations.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dispatcher::{
    DEFAULT_FILE_OPS_ENABLED, DEFAULT_MAX_FILE_SIZE, DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE,
    DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE,
};

// =============================================================================
// FileOpsConfigToml
// =============================================================================

/// File operations executor configuration
///
/// Configures permissions and behavior for file system operations.
/// Uses path-based access control with allowed/denied lists.
///
/// # Example TOML
/// ```toml
/// [cowork.file_ops]
/// enabled = true
/// allowed_paths = ["~/Downloads", "~/Documents"]
/// denied_paths = ["~/.ssh", "~/.gnupg"]
/// max_file_size = "100MB"
/// require_confirmation_for_write = true
/// require_confirmation_for_delete = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileOpsConfigToml {
    /// Enable file operations executor
    #[serde(default = "default_file_ops_enabled")]
    pub enabled: bool,

    /// Paths that are allowed for file operations (glob patterns)
    /// Empty list = all paths allowed (except denied)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_paths: Vec<String>,

    /// Paths that are denied for file operations (glob patterns)
    /// Takes precedence over allowed_paths
    /// Default denied paths (~/.ssh, ~/.gnupg, etc.) are always applied
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_paths: Vec<String>,

    /// Maximum file size in bytes for read operations
    /// 0 = unlimited
    /// Accepts human-readable values: "100MB", "1GB", etc.
    #[serde(
        default = "default_max_file_size",
        deserialize_with = "deserialize_file_size"
    )]
    pub max_file_size: u64,

    /// Require confirmation before write operations
    #[serde(default = "default_require_confirmation_for_write")]
    pub require_confirmation_for_write: bool,

    /// Require confirmation before delete operations
    #[serde(default = "default_require_confirmation_for_delete")]
    pub require_confirmation_for_delete: bool,
}

impl Default for FileOpsConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_file_ops_enabled(),
            allowed_paths: Vec::new(),
            denied_paths: Vec::new(),
            max_file_size: default_max_file_size(),
            require_confirmation_for_write: default_require_confirmation_for_write(),
            require_confirmation_for_delete: default_require_confirmation_for_delete(),
        }
    }
}

impl FileOpsConfigToml {
    /// Validate the file ops configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate max_file_size (warn if very large)
        if self.max_file_size > 10 * 1024 * 1024 * 1024 {
            tracing::warn!(
                max_file_size = self.max_file_size,
                "agent.file_ops.max_file_size is very large (>10GB)"
            );
        }

        // Validate path patterns are valid glob patterns
        for path in &self.allowed_paths {
            if glob::Pattern::new(path).is_err() {
                return Err(format!(
                    "agent.file_ops.allowed_paths contains invalid glob pattern: '{}'",
                    path
                ));
            }
        }

        for path in &self.denied_paths {
            if glob::Pattern::new(path).is_err() {
                return Err(format!(
                    "agent.file_ops.denied_paths contains invalid glob pattern: '{}'",
                    path
                ));
            }
        }

        Ok(())
    }

    /// Create a FileOpsExecutor from this configuration
    pub fn create_executor(&self) -> crate::dispatcher::executor::FileOpsExecutor {
        crate::dispatcher::executor::FileOpsExecutor::new(
            self.allowed_paths.clone(),
            self.denied_paths.clone(),
            self.max_file_size,
            self.require_confirmation_for_write,
            self.require_confirmation_for_delete,
        )
    }
}

// =============================================================================
// Default Functions
// =============================================================================

pub fn default_file_ops_enabled() -> bool {
    DEFAULT_FILE_OPS_ENABLED
}

pub fn default_max_file_size() -> u64 {
    DEFAULT_MAX_FILE_SIZE
}

pub fn default_require_confirmation_for_write() -> bool {
    DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE
}

pub fn default_require_confirmation_for_delete() -> bool {
    DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE
}

// =============================================================================
// File Size Parsing
// =============================================================================

/// Deserialize file size from human-readable string or number
///
/// Supports formats like "100MB", "1GB", "500KB", or plain numbers (bytes)
pub(crate) fn deserialize_file_size<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FileSizeValue {
        Number(u64),
        String(String),
    }

    match FileSizeValue::deserialize(deserializer)? {
        FileSizeValue::Number(n) => Ok(n),
        FileSizeValue::String(s) => parse_file_size(&s).map_err(D::Error::custom),
    }
}

/// Parse human-readable file size string
pub fn parse_file_size(s: &str) -> Result<u64, String> {
    let s = s.trim().to_uppercase();

    // Try to parse as plain number first
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }

    // Parse with suffix
    let (num_part, suffix) = if s.ends_with("GB") {
        (&s[..s.len() - 2], 1024 * 1024 * 1024)
    } else if s.ends_with("MB") {
        (&s[..s.len() - 2], 1024 * 1024)
    } else if s.ends_with("KB") {
        (&s[..s.len() - 2], 1024)
    } else if s.ends_with('B') {
        (&s[..s.len() - 1], 1)
    } else {
        return Err(format!(
            "Invalid file size format: '{}'. Use formats like '100MB', '1GB', etc.",
            s
        ));
    };

    let num: u64 = num_part
        .trim()
        .parse()
        .map_err(|_| format!("Invalid number in file size: '{}'", num_part))?;

    Ok(num * suffix)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_ops_default_config() {
        let config = FileOpsConfigToml::default();
        assert!(config.enabled);
        assert!(config.allowed_paths.is_empty());
        assert!(config.denied_paths.is_empty());
        assert_eq!(config.max_file_size, 100 * 1024 * 1024); // 100MB
        assert!(config.require_confirmation_for_write);
        assert!(config.require_confirmation_for_delete);
    }

    #[test]
    fn test_file_ops_validation() {
        let mut config = FileOpsConfigToml::default();

        // Valid config should pass
        assert!(config.validate().is_ok());

        // Valid glob patterns
        config.allowed_paths = vec!["~/Documents/**".to_string()];
        assert!(config.validate().is_ok());

        // Invalid glob pattern
        config.allowed_paths = vec!["[invalid".to_string()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_parse_file_size() {
        assert_eq!(parse_file_size("100").unwrap(), 100);
        assert_eq!(parse_file_size("1KB").unwrap(), 1024);
        assert_eq!(parse_file_size("1MB").unwrap(), 1024 * 1024);
        assert_eq!(parse_file_size("1GB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_file_size("100MB").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_file_size("  50 MB  ").unwrap(), 50 * 1024 * 1024);

        // Case insensitive
        assert_eq!(parse_file_size("1mb").unwrap(), 1024 * 1024);
        assert_eq!(parse_file_size("1Mb").unwrap(), 1024 * 1024);

        // Invalid formats
        assert!(parse_file_size("invalid").is_err());
        assert!(parse_file_size("100TB").is_err()); // TB not supported
    }
}
