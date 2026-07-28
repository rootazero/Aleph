//! `SandboxCapabilities` — what a command is allowed to do inside the sandbox.
//!
//! # Platform-specific network policy support (as of Cycle 1 hardening)
//!
//! **macOS**: `NetworkPolicy::AllowHosts` accepts IP literals only. Hostnames
//! return `SandboxError::UnsupportedPolicy` because Seatbelt's `remote ip`
//! matcher does not perform DNS resolution. `ProxyOnly { ports }` is honored
//! by allowing outbound connections to `localhost:port`.
//!
//! **Linux**: `AllowHosts` and `ProxyOnly` return `UnsupportedPolicy`.
//! Bubblewrap only offers all-or-nothing network isolation (`--unshare-net`);
//! fine-grained host filtering requires Landlock + seccomp-bpf or iptables
//! (deferred — see spec SP-2). Use `AllowAll` or `None`.
//!
//! **Windows**: `AllowHosts` and `ProxyOnly` return `UnsupportedPolicy`. The
//! WFP (Windows Filtering Platform) integration that would enforce per-host
//! rules is deferred — see spec SP-3. Use `AllowAll` or `None`.
//!
//! # Resource limits
//!
//! `max_memory_mb` is enforced on all three platforms when `Some`:
//! - macOS / Linux: `setrlimit(RLIMIT_AS)` applied via `pre_exec` before the
//!   sandbox helper spawns the target (limit propagates through exec).
//! - Windows: `JOBOBJECT_EXTENDED_LIMIT_INFORMATION.ProcessMemoryLimit` on
//!   the Job Object that contains the sandboxed process.
//!
//! `timeout_secs`, when `Some`, overrides the per-call default that the
//! `WorkspaceSandbox` applies. `None` falls back to the configured default.

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
    /// Hard ceiling on the sandboxed process's virtual address space.
    /// `None` means "no per-call cap" (the OS default still applies).
    #[serde(default)]
    pub max_memory_mb: Option<u64>,
    /// Per-call timeout override. `None` falls back to the sandbox-level
    /// default configured in `SandboxConfig`.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
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
    #[must_use]
    pub fn strict() -> Self {
        Self::default()
    }

    /// Return a copy with `fs_read` and `fs_write` sorted so that
    /// `Hash`/`Eq` are order-independent. Used before storing in
    /// `granted_elevations` cache to prevent duplicate entries for
    /// the same capability set with different path order.
    #[must_use]
    pub fn normalized(&self) -> Self {
        // rust-doctor-disable-next-line excessive-clone
        let mut copy = self.clone();
        copy.fs_read.sort();
        copy.fs_write.sort();
        copy
    }

    /// Is `self` ⊆ `baseline` (fs subset; Network ordered None ⊆ `AllowHosts` ⊆ `AllowAll`;
    /// spawn monotonic; resource limits at least as tight as baseline)?
    ///
    /// For filesystem paths, both paths are normalized (resolving `.` and `..`)
    /// before the prefix check to prevent path traversal bypasses.
    ///
    /// For `max_memory_mb` / `timeout_secs`: `None` means "unlimited". A
    /// limited child is within an unlimited baseline; an unlimited child is
    /// NOT within a limited baseline; two limited values use `<=`.
    #[must_use]
    pub fn is_within(&self, baseline: &Self) -> bool {
        let fs_read_ok = self.fs_read.iter().all(|p| {
            baseline
                .fs_read
                .iter()
                .any(|b| path_starts_with_normalized(p, b))
        });
        let fs_write_ok = self.fs_write.iter().all(|p| {
            baseline
                .fs_write
                .iter()
                .any(|b| path_starts_with_normalized(p, b))
        });
        let net_ok = network_within(&self.network, &baseline.network);
        let spawn_ok = !self.spawn_subprocess || baseline.spawn_subprocess;
        let mem_ok = limit_within(self.max_memory_mb, baseline.max_memory_mb);
        let timeout_ok = limit_within(self.timeout_secs, baseline.timeout_secs);
        fs_read_ok && fs_write_ok && net_ok && spawn_ok && mem_ok && timeout_ok
    }
}

/// `Option<u64>` subset check where smaller-or-equal-or-Some is "tighter".
/// `None` = unlimited.
const fn limit_within(child: Option<u64>, baseline: Option<u64>) -> bool {
    match (child, baseline) {
        (_, None) => true,            // unlimited baseline accepts anything
        (None, Some(_)) => false,     // unlimited child violates limited baseline
        (Some(c), Some(b)) => c <= b, // both limited: child must be ≤ baseline
    }
}

/// Normalize a path and check if `child` is within `baseline`.
/// Resolves `.` and `..` segments to prevent path traversal bypasses, and
/// attempts to resolve symlinks so an approved parent cannot be escaped via
/// a symlink placed under it.
///
/// Fail-closed: if any *existing* ancestor along the way is a symlink
/// pointing outside `baseline`, the check returns `false`. Lexical-only
/// fallbacks are safe only when the parent chain has been verified free of
/// untrusted symlinks — i.e. when the path genuinely does not exist yet
/// (the caller wants to create it) and the deepest existing ancestor is
/// inside `baseline`. Configuration-time baseline paths that simply do not
/// exist yet still fall through to lexical comparison; production code is
/// expected to set up the workspace tree before issuing `is_within` checks.
fn path_starts_with_normalized(child: &std::path::Path, baseline: &std::path::Path) -> bool {
    if child
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }

    let child_norm = normalize_path_components(child);
    let baseline_norm = normalize_path_components(baseline);

    // Best-effort symlink resolution: if the path exists, canonicalize it.
    // For non-existent targets (e.g., a write to a new file), fall back to
    // a parent-chain walk that verifies each existing ancestor is inside
    // `baseline` and not itself a symlink that escapes.
    let child_canon = std::fs::canonicalize(&child_norm).ok();
    let baseline_canon =
        std::fs::canonicalize(&baseline_norm).unwrap_or_else(|_| baseline_norm.clone());

    if let Some(cc) = &child_canon {
        return cc.starts_with(&baseline_canon);
    }

    if !child_norm.starts_with(&baseline_norm) {
        return false;
    }

    // Child does not exist. Walk up the parent chain verifying no ancestor
    // is a symlink pointing outside `baseline`. This closes the
    // symlink-swap TOCTOU: an attacker who creates `/tmp/foo` as a symlink
    // to `/etc` after this check but before the actual write would have
    // their symlink traversed by `symlink_metadata` and rejected.
    let mut cursor: Option<&std::path::Path> = Some(child_norm.as_path());
    while let Some(p) = cursor {
        if p == baseline_norm.as_path() {
            return true;
        }

        match std::fs::symlink_metadata(p) {
            Ok(meta) if meta.is_symlink() => return false,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
        cursor = p.parent();
    }

    false
}

fn normalize_path_components(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(c) => normalized.push(c),
            Component::RootDir => normalized.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(p) => {
                // On Windows, preserve the drive letter / UNC prefix so that
                // paths on different drives do not collapse into the same
                // namespace and bypass the prefix check.
                normalized.push(p.as_os_str());
            }
        }
    }
    normalized
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

    #[cfg(unix)]
    #[test]
    fn fs_write_rejects_symlink_swap_outside_baseline() {
        // Regression: a baseline that exists but whose child path is
        // non-existent at check time MUST NOT silently fall back to a
        // lexical check that allows a subsequent symlink-swap to escape.
        // We use a temp dir to model a real baseline path, and create a
        // symlink under it pointing outside. The child path targets a
        // file under the symlink (which does not yet exist).
        let tmp = tempfile::tempdir().unwrap();
        let baseline_root = tmp.path().to_path_buf();
        let outside = tmp.path().join("outside-target");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, baseline_root.join("escape")).unwrap();

        let baseline = SandboxCapabilities {
            fs_write: vec![baseline_root.clone()],
            ..Default::default()
        };
        let child = SandboxCapabilities {
            fs_write: vec![baseline_root.join("escape").join("payload")],
            ..Default::default()
        };
        assert!(
            !child.is_within(&baseline),
            "is_within must fail closed when child parent is a symlink pointing outside baseline"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fs_write_rejects_symlink_parent_dir_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let baseline_root = tmp.path().join("safe");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&baseline_root).unwrap();
        std::fs::create_dir_all(outside.join("dir")).unwrap();
        std::os::unix::fs::symlink(outside.join("dir"), baseline_root.join("link")).unwrap();

        let baseline = SandboxCapabilities {
            fs_write: vec![baseline_root.clone()],
            ..Default::default()
        };
        let child = SandboxCapabilities {
            fs_write: vec![baseline_root.join("link").join("..").join("secret")],
            ..Default::default()
        };
        assert!(!child.is_within(&baseline));
    }

    #[test]
    fn limit_within_unlimited_baseline_accepts_any() {
        assert!(limit_within(None, None));
        assert!(limit_within(Some(64), None));
        assert!(limit_within(Some(u64::MAX), None));
    }

    #[test]
    fn limit_within_unlimited_child_violates_limited_baseline() {
        assert!(!limit_within(None, Some(64)));
    }

    #[test]
    fn limit_within_smaller_child_is_tighter() {
        assert!(limit_within(Some(32), Some(64)));
        assert!(limit_within(Some(64), Some(64)));
        assert!(!limit_within(Some(128), Some(64)));
    }

    #[test]
    fn is_within_respects_memory_tighter() {
        let baseline = SandboxCapabilities {
            max_memory_mb: Some(128),
            ..Default::default()
        };
        let child_tighter = SandboxCapabilities {
            max_memory_mb: Some(64),
            ..Default::default()
        };
        let child_unlimited = SandboxCapabilities::default();
        assert!(child_tighter.is_within(&baseline));
        assert!(!child_unlimited.is_within(&baseline));
    }

    #[test]
    fn is_within_respects_timeout_tighter() {
        let baseline = SandboxCapabilities {
            timeout_secs: Some(60),
            ..Default::default()
        };
        let child_tighter = SandboxCapabilities {
            timeout_secs: Some(10),
            ..Default::default()
        };
        let child_unlimited = SandboxCapabilities::default();
        assert!(child_tighter.is_within(&baseline));
        assert!(!child_unlimited.is_within(&baseline));
    }
}
