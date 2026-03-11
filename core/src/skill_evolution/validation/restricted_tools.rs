//! Restricted toolset — software-defined sandbox boundaries.
//!
//! Validates tool calls against a whitelist and constrains all
//! file path operations to a root directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// A violation detected by the restricted toolset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxViolation {
    pub tool_name: String,
    pub reason: String,
}

/// Enforces sandbox constraints on tool calls.
pub struct RestrictedToolset {
    allowed_tools: HashSet<String>,
    root_dir: PathBuf,
    allow_network: bool,
}

impl RestrictedToolset {
    /// Create a new restricted toolset.
    pub fn new(
        allowed_tools: HashSet<String>,
        root_dir: PathBuf,
        allow_network: bool,
    ) -> Self {
        Self {
            allowed_tools,
            root_dir,
            allow_network,
        }
    }

    /// Validate a tool call. Returns Ok(()) if allowed, Err with violation if not.
    pub fn validate_call(
        &self,
        tool_name: &str,
        file_path: Option<&Path>,
    ) -> Result<(), SandboxViolation> {
        // Check tool whitelist
        if !self.allowed_tools.contains(tool_name) {
            return Err(SandboxViolation {
                tool_name: tool_name.to_string(),
                reason: format!("tool '{}' not in whitelist", tool_name),
            });
        }

        // Check network access
        if !self.allow_network && is_network_tool(tool_name) {
            return Err(SandboxViolation {
                tool_name: tool_name.to_string(),
                reason: "network access not allowed in sandbox".to_string(),
            });
        }

        // Check path bounds
        if let Some(path) = file_path {
            self.validate_path(tool_name, path)?;
        }

        Ok(())
    }

    /// Validate that a path resolves within root_dir.
    ///
    /// Two-phase check: (1) logical normalization rejects obvious traversals
    /// without touching the filesystem, then (2) canonicalization catches
    /// symlink-based escapes for paths that exist on disk.
    fn validate_path(&self, tool_name: &str, path: &Path) -> Result<(), SandboxViolation> {
        // Phase 1: Logical normalization (fast-path, no filesystem access)
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root_dir.join(path)
        };

        let normalized = normalize_path(&resolved);
        let root_normalized = normalize_path(&self.root_dir);

        if !normalized.starts_with(&root_normalized) {
            return Err(SandboxViolation {
                tool_name: tool_name.to_string(),
                reason: format!(
                    "path '{}' escapes sandbox root '{}'",
                    path.display(),
                    self.root_dir.display()
                ),
            });
        }

        // Phase 2: Canonicalize to catch symlink escapes (only if path exists)
        if let Ok(canonical) = std::fs::canonicalize(&resolved) {
            let canonical_root = std::fs::canonicalize(&self.root_dir)
                .unwrap_or_else(|_| root_normalized.clone());
            if !canonical.starts_with(&canonical_root) {
                return Err(SandboxViolation {
                    tool_name: tool_name.to_string(),
                    reason: format!(
                        "path '{}' resolves to '{}' which escapes sandbox root",
                        path.display(),
                        canonical.display()
                    ),
                });
            }
        }

        Ok(())
    }
}

/// Simple path normalization (resolve `.` and `..` without touching filesystem).
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

/// Check if a tool name implies network access.
fn is_network_tool(name: &str) -> bool {
    let network_tools = ["http_request", "fetch_url", "web_search", "api_call"];
    network_tools.iter().any(|t| name.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_toolset(tools: &[&str], root: &str) -> RestrictedToolset {
        RestrictedToolset::new(
            tools.iter().map(|s| s.to_string()).collect(),
            PathBuf::from(root),
            false,
        )
    }

    #[test]
    fn allowed_tool_passes() {
        let ts = make_toolset(&["read_file", "write_file"], "/sandbox");
        assert!(ts.validate_call("read_file", None).is_ok());
    }

    #[test]
    fn disallowed_tool_fails() {
        let ts = make_toolset(&["read_file"], "/sandbox");
        let err = ts.validate_call("delete_all", None).unwrap_err();
        assert!(err.reason.contains("not in whitelist"));
    }

    #[test]
    fn path_within_root_passes() {
        let ts = make_toolset(&["write_file"], "/sandbox");
        assert!(ts.validate_call("write_file", Some(Path::new("subdir/file.txt"))).is_ok());
    }

    #[test]
    fn path_escape_fails() {
        let ts = make_toolset(&["write_file"], "/sandbox");
        let err = ts
            .validate_call("write_file", Some(Path::new("../../../etc/passwd")))
            .unwrap_err();
        assert!(err.reason.contains("escapes sandbox root"));
    }

    #[test]
    fn absolute_path_outside_root_fails() {
        let ts = make_toolset(&["write_file"], "/sandbox");
        let err = ts
            .validate_call("write_file", Some(Path::new("/home/user/secret.txt")))
            .unwrap_err();
        assert!(err.reason.contains("escapes sandbox root"));
    }

    #[test]
    fn network_tool_blocked_when_disabled() {
        let ts = make_toolset(&["http_request"], "/sandbox");
        let err = ts.validate_call("http_request", None).unwrap_err();
        assert!(err.reason.contains("network access not allowed"));
    }

    #[test]
    fn network_tool_allowed_when_enabled() {
        let ts = RestrictedToolset::new(
            ["http_request"].iter().map(|s| s.to_string()).collect(),
            PathBuf::from("/sandbox"),
            true, // allow network
        );
        assert!(ts.validate_call("http_request", None).is_ok());
    }

    #[test]
    fn normalize_resolves_dotdot() {
        let p = normalize_path(Path::new("/a/b/../c"));
        assert_eq!(p, PathBuf::from("/a/c"));
    }
}
