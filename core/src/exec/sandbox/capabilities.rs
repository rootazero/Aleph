use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Fine-grained permission model for sandboxed execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capabilities {
    /// Filesystem access permissions
    pub filesystem: Vec<FileSystemCapability>,
    /// Network access permissions
    pub network: NetworkCapability,
    /// Process execution constraints
    pub process: ProcessCapability,
    /// Environment variable access
    pub environment: EnvironmentCapability,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            filesystem: vec![FileSystemCapability::TempWorkspace],
            network: NetworkCapability::Deny,
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300, // 5 minutes
                max_memory_mb: 512,
                max_cpu_percent: 80,
            },
            environment: EnvironmentCapability::Restricted,
        }
    }
}

/// Filesystem access capability
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileSystemCapability {
    /// Read-only access to specific path
    ReadOnly(PathBuf),
    /// Read-write access to specific path
    ReadWrite(PathBuf),
    /// Access to temporary workspace (auto-created, auto-cleaned)
    TempWorkspace,
}

/// Network access capability
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkCapability {
    /// No network access
    Deny,
    /// Allow access to specific domains
    AllowDomains(Vec<String>),
    /// Allow all network access
    AllowAll,
}

/// Process execution constraints
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessCapability {
    /// Prevent fork/exec of child processes
    pub no_fork: bool,
    /// Maximum execution time in seconds
    pub max_execution_time: u64,
    /// Maximum memory usage in MB
    pub max_memory_mb: u64,
    /// Maximum CPU usage percentage
    pub max_cpu_percent: u8,
}

/// Environment variable access
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EnvironmentCapability {
    /// No environment variables
    None,
    /// Only safe environment variables (PATH, HOME, USER, etc.)
    Restricted,
    /// Full environment access
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities_default() {
        let caps = Capabilities::default();
        assert_eq!(caps.filesystem.len(), 1);
        assert!(matches!(
            caps.filesystem[0],
            FileSystemCapability::TempWorkspace
        ));
        assert!(matches!(caps.network, NetworkCapability::Deny));
        assert!(caps.process.no_fork);
        assert_eq!(caps.process.max_execution_time, 300);
    }

    #[test]
    fn test_capabilities_serialization() {
        let caps = Capabilities::default();
        let json = serde_json::to_string(&caps).unwrap();
        let deserialized: Capabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps.process.no_fork, deserialized.process.no_fork);
    }
}
