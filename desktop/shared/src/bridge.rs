//! SwiftBridge — spawn the `aleph-bridge` Swift CLI binary and parse JSON output.
//!
//! `aleph-bridge` is a thin macOS-native Swift CLI that exposes system APIs
//! (PIM, notifications, accessibility, etc.) as JSON-over-stdout commands.
//! This module provides an ergonomic async wrapper around that binary.

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use tokio::process::Command;

use crate::error::{DesktopError, Result};

/// Wrapper for spawning the `aleph-bridge` Swift CLI and parsing its JSON output.
pub struct SwiftBridge {
    /// Path to the `aleph-bridge` binary.
    pub(crate) binary_path: PathBuf,
}

impl SwiftBridge {
    /// Create a `SwiftBridge` with an explicit path to the binary.
    pub fn new(binary_path: PathBuf) -> Self {
        Self { binary_path }
    }

    /// Invoke a bridge command and deserialize its JSON stdout into `T`.
    ///
    /// The bridge CLI is called as:
    /// ```text
    /// aleph-bridge <domain> <action> [--key value ...]
    /// ```
    ///
    /// Errors:
    /// - Spawn failure → [`DesktopError::BridgeFailed`]
    /// - Non-zero exit code → [`DesktopError::BridgeFailed`] with stderr
    /// - JSON parse failure → [`DesktopError::BridgeFailed`] with raw stdout
    pub async fn call<T: DeserializeOwned>(
        &self,
        domain: &str,
        action: &str,
        args: &[(&str, &str)],
    ) -> Result<T> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.arg(domain).arg(action);

        // Pass extra arguments as --key value pairs.
        for (key, value) in args {
            cmd.arg(format!("--{key}"));
            cmd.arg(value);
        }

        let output = cmd.output().await.map_err(|e| {
            DesktopError::BridgeFailed(format!(
                "failed to spawn {}: {e}",
                self.binary_path.display()
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::BridgeFailed(format!(
                "aleph-bridge exited with {}: {stderr}",
                output.status
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str::<T>(&stdout).map_err(|e| {
            DesktopError::BridgeFailed(format!(
                "failed to parse JSON from aleph-bridge: {e}; raw output: {stdout}"
            ))
        })
    }

    /// Return `true` if the bridge binary exists on the filesystem.
    ///
    /// Note: a bare name like `"aleph-bridge"` (no directory component) is
    /// treated as always potentially available via `PATH`; this method only
    /// does a filesystem check when the path has a directory component.
    pub fn is_available(&self) -> bool {
        if self.binary_path.components().count() > 1 {
            self.binary_path.exists()
        } else {
            // Bare name — assume it might be on PATH; caller can do a real probe.
            true
        }
    }
}

impl Default for SwiftBridge {
    /// Locate `aleph-bridge` using a four-step search:
    ///
    /// 1. Next to the current executable (side-by-side deployment).
    /// 2. The repo-local Swift build artifact used during development.
    /// 3. `~/.aleph/bin/aleph-bridge` (user-local install).
    /// 4. Bare `"aleph-bridge"` (rely on `PATH`).
    fn default() -> Self {
        // 1. Sibling to the current exe.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join("aleph-bridge");
                if candidate.exists() {
                    return Self::new(candidate);
                }

                let candidate = dir.join("AlephBridge");
                if candidate.exists() {
                    return Self::new(candidate);
                }
            }

            // 2. Repo-local dev build:
            //    <repo>/target/{debug,release}/aleph-server
            //      -> <repo>/desktop/macos/bridge/.build/release/AlephBridge
            if let Some(target_dir) = exe.parent().and_then(|p| p.parent()) {
                if let Some(repo_root) = target_dir.parent() {
                    let candidate = repo_root
                        .join("desktop")
                        .join("macos")
                        .join("bridge")
                        .join(".build")
                        .join("release")
                        .join("AlephBridge");
                    if candidate.exists() {
                        return Self::new(candidate);
                    }
                }
            }
        }

        // 3. ~/.aleph/bin/aleph-bridge
        if let Some(home) = dirs::home_dir() {
            let candidate = home.join(".aleph").join("bin").join("aleph-bridge");
            if candidate.exists() {
                return Self::new(candidate);
            }
        }

        // 4. Bare name — rely on PATH.
        Self::new(PathBuf::from("aleph-bridge"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_stores_path() {
        let bridge = SwiftBridge::new(PathBuf::from("/usr/local/bin/aleph-bridge"));
        assert_eq!(
            bridge.binary_path,
            PathBuf::from("/usr/local/bin/aleph-bridge")
        );
    }

    #[test]
    fn test_default_contains_aleph_bridge() {
        let bridge = SwiftBridge::default();
        let path_str = bridge.binary_path.to_string_lossy();
        assert!(path_str.contains("aleph-bridge"));
    }
}
