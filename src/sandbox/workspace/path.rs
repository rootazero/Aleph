//! Path helpers for the workspace sandbox.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::session::service::SessionId;

/// Normalize a path, resolving `..` and `.` segments, then check if it stays
/// within the workspace root.
pub(crate) fn normalize_path(path: &Path, workspace_root: &Path) -> PathBuf {
    use std::path::Component;
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in base.components() {
        match component {
            Component::Normal(c) => normalized.push(c),
            Component::RootDir => normalized.push("/"),
            Component::CurDir => {} // skip .
            Component::ParentDir => {
                // pop the last component for ..
                normalized.pop();
            }
            Component::Prefix(p) => {
                // Preserve Windows drive/UNC prefix so the path remains
                // valid for canonicalize and stays on the intended volume.
                normalized.push(p.as_os_str());
            }
        }
    }
    normalized
}

/// Compute the per-session workspace directory the same way `WorkspaceSandbox`
/// does, without instantiating one. Lets out-of-band consumers (cluster node
/// file commands) jail to the exact dir the node's bash sandbox uses.
#[must_use]
pub fn session_workspace_dir(workspace_root: &Path, sid: &SessionId) -> PathBuf {
    workspace_root.join(session_key_to_filename(sid))
}

/// Deterministic filesystem-safe directory name derived from a `SessionId`.
///
/// Uses SHA-256 of the JSON-serialised key, truncated to 16 bytes (32 hex
/// chars). Keeps the path short and avoids slashes / special chars that the
/// various `SessionKey` variants may carry.
pub(crate) fn session_key_to_filename(sid: &SessionId) -> String {
    let json = serde_json::to_string(sid).unwrap_or_else(|_| format!("{sid:?}"));
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}
