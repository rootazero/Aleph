//! Desktop bridge — Swift helper subprocess + JSON-RPC client modules.
//!
//! This module replaces the old `bridge.rs` single file with a focused
//! submodule layout:
//! - `codec`  : line-delimited JSON encode/decode.
//! - `inflight` : in-flight RPC id -> oneshot reply channel table.
//! - `client` (T0.5) : long-lived RPC client driving the helper subprocess.
//! - `supervisor` (T0.6) : restart-on-crash supervisor + disabled mode.
//!
//! The legacy spawn-per-call `SwiftBridge` below is kept for one release so
//! `desktop/macos/src/pim.rs` continues to compile; it is replaced by the
//! new long-lived client in Task 0.5.

pub mod codec;
pub mod inflight;

#[allow(dead_code)] // populated in T0.5
pub mod client;

#[allow(dead_code)] // populated in T0.6
pub mod supervisor;

// ---------------------------------------------------------------------------
// Legacy SwiftBridge (spawn-per-call) — removed in T0.5 once the long-lived
// client in `bridge::client` is wired up. DO NOT add new callers.
// ---------------------------------------------------------------------------

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use tokio::process::Command;

use crate::error::{DesktopError, Result};

/// Wrapper for spawning the `aleph-bridge` Swift CLI and parsing its JSON output.
///
/// **Deprecated**: replaced by `bridge::client::SwiftBridgeClient` in T0.5.
pub struct SwiftBridge {
    pub(crate) binary_path: PathBuf,
}

impl SwiftBridge {
    pub fn new(binary_path: PathBuf) -> Self {
        Self { binary_path }
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        domain: &str,
        action: &str,
        args: &[(&str, &str)],
    ) -> Result<T> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.arg(domain).arg(action);

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

    pub fn is_available(&self) -> bool {
        if self.binary_path.components().count() > 1 {
            self.binary_path.exists()
        } else {
            true
        }
    }
}

impl Default for SwiftBridge {
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

            // 2. Repo-local dev build.
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
