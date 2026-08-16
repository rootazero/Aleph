//! `SandboxPolicy` — internal unified policy representation derived from
//! user-facing `SandboxCapabilities`. Used by platform-specific sandbox drivers.

use std::path::PathBuf;

use super::capabilities::{NetworkPolicy as CapNetworkPolicy, SandboxCapabilities};

/// Internal unified policy struct consumed by platform-specific sandbox drivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    pub filesystem: FsPolicy,
    pub network: NetworkPolicy,
    pub process: ProcessPolicy,
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
    /// Both read and write paths specified — preserved as separate lists so
    /// platform drivers can apply the right primitive to each (e.g. bwrap
    /// `--ro-bind` vs `--bind`, Windows `read=` vs `write=`).
    ReadWritePaths {
        read: Vec<PathBuf>,
        write: Vec<PathBuf>,
    },
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
}

/// Process execution policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessPolicy {
    /// Whether subprocess spawning (fork/exec) is permitted.
    pub allow_fork: bool,
    /// Optional memory limit in megabytes.
    pub max_memory_mb: Option<u64>,
}

impl Default for SandboxPolicy {
    /// Strict defaults: workspace-only FS, no network, no fork.
    fn default() -> Self {
        Self {
            filesystem: FsPolicy::WorkspaceOnly,
            network: NetworkPolicy::None,
            process: ProcessPolicy {
                allow_fork: false,
                max_memory_mb: None,
            },
        }
    }
}

impl From<&SandboxCapabilities> for SandboxPolicy {
    fn from(caps: &SandboxCapabilities) -> Self {
        let filesystem = match (caps.fs_read.is_empty(), caps.fs_write.is_empty()) {
            (true, true) => FsPolicy::WorkspaceOnly,
            (false, true) => {
                // rust-doctor-disable-next-line excessive-clone
                FsPolicy::ReadPaths(caps.fs_read.clone())
            }
            (true, false) => {
                // rust-doctor-disable-next-line excessive-clone
                FsPolicy::WritePaths(caps.fs_write.clone())
            }
            (false, false) => FsPolicy::ReadWritePaths {
                // rust-doctor-disable-next-line excessive-clone
                read: caps.fs_read.clone(),
                // rust-doctor-disable-next-line excessive-clone
                write: caps.fs_write.clone(),
            },
        };

        let network = match &caps.network {
            CapNetworkPolicy::None => NetworkPolicy::None,
            CapNetworkPolicy::AllowAll => NetworkPolicy::AllowAll,
            // rust-doctor-disable-next-line excessive-clone
            CapNetworkPolicy::AllowHosts { hosts } => NetworkPolicy::AllowHosts(hosts.clone()),
        };

        let process = ProcessPolicy {
            allow_fork: caps.spawn_subprocess,
            max_memory_mb: caps.max_memory_mb,
        };

        Self {
            filesystem,
            network,
            process,
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
        assert_eq!(policy.process.max_memory_mb, None);
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
        // When both are present, keep them as separate read and write lists
        // so platform drivers can pick the right primitive (ro-bind vs bind).
        assert_eq!(
            policy.filesystem,
            FsPolicy::ReadWritePaths {
                read: vec!["/etc".into()],
                write: vec!["/tmp".into()],
            }
        );
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
            max_memory_mb: None,
            timeout_secs: None,
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(
            policy.filesystem,
            FsPolicy::ReadWritePaths {
                read: vec!["/etc".into()],
                write: vec!["/tmp".into()],
            }
        );
        assert_eq!(
            policy.network,
            NetworkPolicy::AllowHosts(vec!["github.com".into()])
        );
        assert!(policy.process.allow_fork);
    }

    #[test]
    fn from_caps_threads_memory_limit() {
        let caps = SandboxCapabilities {
            max_memory_mb: Some(256),
            ..Default::default()
        };
        let policy = SandboxPolicy::from(&caps);
        assert_eq!(policy.process.max_memory_mb, Some(256));
    }
}
