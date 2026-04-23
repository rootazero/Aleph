//! SandboxPolicy — internal unified policy representation derived from
//! user-facing `SandboxCapabilities`. Used by platform-specific sandbox drivers.

use std::path::PathBuf;

use super::capabilities::{NetworkPolicy as CapNetworkPolicy, SandboxCapabilities};

/// Internal unified policy struct consumed by platform-specific sandbox drivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub filesystem: FsPolicy,
    pub network: NetworkPolicy,
    pub process: ProcessPolicy,
    pub environment: EnvPolicy,
}

/// Filesystem access policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsPolicy {
    /// Only the workspace directory (default strict mode).
    WorkspaceOnly,
    /// Read access to specific paths.
    ReadPaths(Vec<PathBuf>),
    /// Write access to specific paths.
    WritePaths(Vec<PathBuf>),
    /// Full read access with optional exclusions.
    FullRead { exclude: Vec<PathBuf> },
    /// Full write access with optional exclusions.
    FullWrite { exclude: Vec<PathBuf> },
}

/// Network access policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPolicy {
    /// No network access.
    None,
    /// Allow connections to specific hosts.
    AllowHosts(Vec<String>),
    /// Allow all network access.
    AllowAll,
    /// Allow only proxy connections on specific ports.
    ProxyOnly { ports: Vec<u16> },
}

/// Process execution policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessPolicy {
    /// Whether subprocess spawning (fork/exec) is permitted.
    pub allow_fork: bool,
    /// Hard timeout in seconds.
    pub timeout_secs: u64,
    /// Optional memory limit in megabytes.
    pub max_memory_mb: Option<u64>,
}

/// Environment variable policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvPolicy {
    /// Inherit the parent environment.
    Inherit,
    /// Restricted — only safe variables (PATH, HOME, etc.).
    Restricted,
    /// Minimal — only what the sandbox itself needs.
    Minimal,
}

impl Default for SandboxPolicy {
    /// Strict defaults: workspace-only FS, no network, no fork, 60s timeout.
    fn default() -> Self {
        Self {
            filesystem: FsPolicy::WorkspaceOnly,
            network: NetworkPolicy::None,
            process: ProcessPolicy {
                allow_fork: false,
                timeout_secs: 60,
                max_memory_mb: None,
            },
            environment: EnvPolicy::Minimal,
        }
    }
}

impl From<&SandboxCapabilities> for SandboxPolicy {
    fn from(caps: &SandboxCapabilities) -> Self {
        let filesystem = if caps.fs_write.is_empty() && caps.fs_read.is_empty() {
            FsPolicy::WorkspaceOnly
        } else if !caps.fs_write.is_empty() && caps.fs_read.is_empty() {
            FsPolicy::WritePaths(caps.fs_write.clone())
        } else if !caps.fs_read.is_empty() && caps.fs_write.is_empty() {
            FsPolicy::ReadPaths(caps.fs_read.clone())
        } else {
            // Both read and write paths specified — treat as write (superset).
            FsPolicy::WritePaths(caps.fs_write.clone())
        };

        let network = match &caps.network {
            CapNetworkPolicy::None => NetworkPolicy::None,
            CapNetworkPolicy::AllowAll => NetworkPolicy::AllowAll,
            CapNetworkPolicy::AllowHosts { hosts } => NetworkPolicy::AllowHosts(hosts.clone()),
        };

        let process = ProcessPolicy {
            allow_fork: caps.spawn_subprocess,
            timeout_secs: 60,
            max_memory_mb: None,
        };

        Self {
            filesystem,
            network,
            process,
            environment: EnvPolicy::Minimal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_strict() {
        let policy = SandboxPolicy::default();
        assert_eq!(policy.filesystem, FsPolicy::WorkspaceOnly);
        assert_eq!(policy.network, NetworkPolicy::None);
        assert!(!policy.process.allow_fork);
        assert_eq!(policy.process.timeout_secs, 60);
        assert_eq!(policy.process.max_memory_mb, None);
        assert_eq!(policy.environment, EnvPolicy::Minimal);
    }

    #[test]
    fn from_caps_empty() {
        let caps = SandboxCapabilities::default();
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(policy.filesystem, FsPolicy::WorkspaceOnly);
        assert_eq!(policy.network, NetworkPolicy::None);
        assert!(!policy.process.allow_fork);
    }

    #[test]
    fn from_caps_read_only() {
        let caps = SandboxCapabilities {
            fs_read: vec!["/tmp".into(), "/var/log".into()],
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(
            policy.filesystem,
            FsPolicy::ReadPaths(vec!["/tmp".into(), "/var/log".into()])
        );
        assert_eq!(policy.network, NetworkPolicy::None);
        assert!(!policy.process.allow_fork);
    }

    #[test]
    fn from_caps_write_only() {
        let caps = SandboxCapabilities {
            fs_write: vec!["/tmp/output".into()],
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(
            policy.filesystem,
            FsPolicy::WritePaths(vec!["/tmp/output".into()])
        );
    }

    #[test]
    fn from_caps_both_read_and_write() {
        let caps = SandboxCapabilities {
            fs_read: vec!["/etc".into()],
            fs_write: vec!["/tmp".into()],
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        // When both are present, write takes precedence (superset).
        assert_eq!(policy.filesystem, FsPolicy::WritePaths(vec!["/tmp".into()]));
    }

    #[test]
    fn from_caps_network_none() {
        let caps = SandboxCapabilities {
            network: CapNetworkPolicy::None,
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(policy.network, NetworkPolicy::None);
    }

    #[test]
    fn from_caps_network_allow_all() {
        let caps = SandboxCapabilities {
            network: CapNetworkPolicy::AllowAll,
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(policy.network, NetworkPolicy::AllowAll);
    }

    #[test]
    fn from_caps_network_allow_hosts() {
        let caps = SandboxCapabilities {
            network: CapNetworkPolicy::AllowHosts {
                hosts: vec!["example.com".into(), "api.example.com".into()],
            },
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(
            policy.network,
            NetworkPolicy::AllowHosts(vec!["example.com".into(), "api.example.com".into()])
        );
    }

    #[test]
    fn from_caps_spawn_subprocess() {
        let caps = SandboxCapabilities {
            spawn_subprocess: true,
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        assert!(policy.process.allow_fork);
        assert_eq!(policy.process.timeout_secs, 60);
        assert_eq!(policy.process.max_memory_mb, None);
    }

    #[test]
    fn from_caps_full_combo() {
        let caps = SandboxCapabilities {
            fs_read: vec!["/etc".into()],
            fs_write: vec!["/tmp".into()],
            network: CapNetworkPolicy::AllowHosts {
                hosts: vec!["github.com".into()],
            },
            spawn_subprocess: true,
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(policy.filesystem, FsPolicy::WritePaths(vec!["/tmp".into()]));
        assert_eq!(
            policy.network,
            NetworkPolicy::AllowHosts(vec!["github.com".into()])
        );
        assert!(policy.process.allow_fork);
        assert_eq!(policy.environment, EnvPolicy::Minimal);
    }
}
