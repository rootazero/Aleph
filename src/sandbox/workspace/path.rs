//! Path helpers for the workspace sandbox.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::session::service::SessionId;

/// Fold `.` and `..` out of a path, lexically, anchoring a relative one on the
/// workspace root. **Resolution only — it decides nothing.**
///
/// This is the EXEC layer's resolver, and it is deliberately not the same
/// function as the file layer's
/// [`crate::builtin_tools::file_ops::path_utils::check_and_resolve_path`].
/// They answer different questions and the split is the contract:
///
/// | | exec layer (here) | file layer (`path_utils`) |
/// |---|---|---|
/// | `~` / `$HOME` / `$USER` | left literal | expanded |
/// | relative base | workspace root | task-local `FsScope`, else error |
/// | `..` | popped **lexically, before** any symlink resolution | canonicalized first, popped only on the non-existent tail |
/// | credential denylist / `/proc` | none | enforced |
/// | root containment | enforced by the CALLER, as a hard jail | none — absolute paths are used as-is |
///
/// Net: the exec layer is an allowlist-jail with no denylist; the file layer is
/// a denylist with no jail. Neither is a fallback for the other, and
/// "unifying" them in either direction silently removes one of those two
/// properties. If you are about to do that, the pin in `path_utils` names this
/// function; read both first.
///
/// The containment this function's output is subject to lives in
/// [`super::WorkspaceSandbox::execute`], and is two-phase — see
/// `revalidate_cwd_containment` there for why once is not enough.
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
