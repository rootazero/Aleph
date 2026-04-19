//! SandboxCapabilities — what a command is allowed to do inside the sandbox.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SandboxCapabilities {
    #[serde(default)]
    pub fs_read: Vec<PathBuf>,
    #[serde(default)]
    pub fs_write: Vec<PathBuf>,
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default)]
    pub spawn_subprocess: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NetworkPolicy {
    #[default]
    None,
    AllowAll,
    AllowHosts {
        hosts: Vec<String>,
    },
}

impl SandboxCapabilities {
    /// Baseline: read/write within workspace cwd, no network, no subprocess spawn.
    pub fn strict() -> Self {
        Self::default()
    }

    /// Is `self` ⊆ `baseline` (fs subset; Network ordered None ⊆ AllowHosts ⊆ AllowAll;
    /// spawn monotonic)?
    pub fn is_within(&self, baseline: &Self) -> bool {
        let fs_read_ok = self
            .fs_read
            .iter()
            .all(|p| baseline.fs_read.iter().any(|b| p.starts_with(b)));
        let fs_write_ok = self
            .fs_write
            .iter()
            .all(|p| baseline.fs_write.iter().any(|b| p.starts_with(b)));
        let net_ok = network_within(&self.network, &baseline.network);
        let spawn_ok = !self.spawn_subprocess || baseline.spawn_subprocess;
        fs_read_ok && fs_write_ok && net_ok && spawn_ok
    }
}

fn network_within(child: &NetworkPolicy, baseline: &NetworkPolicy) -> bool {
    use NetworkPolicy::*;
    match (child, baseline) {
        (None, _) => true,
        (_, AllowAll) => true,
        (AllowAll, _) => false,
        (AllowHosts { hosts: c }, AllowHosts { hosts: b }) => c.iter().all(|h| b.contains(h)),
        (AllowHosts { .. }, None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_is_default() {
        let s = SandboxCapabilities::strict();
        assert!(s.fs_read.is_empty());
        assert!(s.fs_write.is_empty());
        assert_eq!(s.network, NetworkPolicy::None);
        assert!(!s.spawn_subprocess);
    }

    #[test]
    fn empty_is_within_anything() {
        let baseline = SandboxCapabilities {
            fs_read: vec!["/tmp".into()],
            ..Default::default()
        };
        assert!(SandboxCapabilities::default().is_within(&baseline));
    }

    #[test]
    fn network_allowall_not_within_none() {
        let child = SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let baseline = SandboxCapabilities::strict();
        assert!(!child.is_within(&baseline));
    }

    #[test]
    fn network_none_within_allowall() {
        let child = SandboxCapabilities::strict();
        let baseline = SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        assert!(child.is_within(&baseline));
    }

    #[test]
    fn spawn_subprocess_monotonic() {
        let child_spawns = SandboxCapabilities {
            spawn_subprocess: true,
            ..Default::default()
        };
        let baseline_no = SandboxCapabilities::strict();
        let baseline_yes = SandboxCapabilities {
            spawn_subprocess: true,
            ..Default::default()
        };
        assert!(!child_spawns.is_within(&baseline_no));
        assert!(child_spawns.is_within(&baseline_yes));
    }

    #[test]
    fn fs_write_subset_by_prefix() {
        let child = SandboxCapabilities {
            fs_write: vec!["/tmp/foo/bar".into()],
            ..Default::default()
        };
        let baseline = SandboxCapabilities {
            fs_write: vec!["/tmp/foo".into()],
            ..Default::default()
        };
        assert!(child.is_within(&baseline));
    }
}
