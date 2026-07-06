//! Path validation and resolution utilities

use std::path::{Path, PathBuf};
use tracing::info;

use crate::builtin_tools::error::ToolError;

/// Denied paths for security.
///
/// Adding entries here is backwards-compatible (strictly tighter) — these are
/// well-known credential stores an agent should never read or overwrite.
/// Matched by [`check_and_resolve_path`] via symlink-canonicalizing prefix
/// comparison, so a directory entry (e.g. `~/.ssh`) covers everything beneath
/// it and a leaf file (e.g. `~/.netrc`) covers exactly that file.
///
/// The credential breadth here mirrors `OpenSquilla`'s `sensitive_paths.py`
/// (SSH/cloud/registry/secret stores) layered onto Aleph's stronger checker,
/// and — like hermes-agent's `get_read_block_error` — extends the deny set to
/// Aleph's *own* credential surface (the encrypted `secrets.vault` and the
/// `data/` auth/device-pairing databases), which an agent must never read or
/// clobber through its file tools.
pub fn get_denied_paths() -> Vec<String> {
    let mut denied_paths = vec![
        // SSH / PGP / AWS — the original Unix credential directories.
        "~/.ssh".to_string(),
        "~/.gnupg".to_string(),
        "~/.aws".to_string(),
        // Cloud-provider credential stores.
        "~/.config/gcloud".to_string(),
        "~/.kube".to_string(),
        "~/.azure".to_string(),
        // Container-registry + package-registry credentials.
        "~/.docker/config.json".to_string(),
        "~/.npmrc".to_string(),
        "~/.pypirc".to_string(),
        // Generic secret stores and credential leaf files.
        "~/.password-store".to_string(),
        "~/.netrc".to_string(),
        "~/.git-credentials".to_string(),
    ];

    // Add specific Aleph config files (not the entire directory)
    // We allow the output directory but deny sensitive config files
    if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
        info!(config_dir = %config_dir.display(), "FileOpsTool: config_dir for denied_paths");
        // Deny config files but NOT the output directory
        denied_paths.push(format!("{}/config.toml", config_dir.display()));
        denied_paths.push(format!("{}/memory.db", config_dir.display()));
        denied_paths.push(format!("{}/conversations.db", config_dir.display()));
        denied_paths.push(format!("{}/skills", config_dir.display()));
        denied_paths.push(format!("{}/plugins", config_dir.display()));
        denied_paths.push(format!("{}/mcp", config_dir.display()));
        // Aleph's own credential / auth state — the crown jewels. `secrets.vault`
        // is the encrypted credential store (`VaultStore::default_path()` =
        // `<config_dir>/secrets.vault`); `data/` holds the device-pairing,
        // session, security and devices databases plus the singleton
        // `aleph.lock`. Denying the directory covers every current and future
        // leaf beneath it via the canonicalizing prefix match. Without this the
        // agent's own `file_read`/`file_write` could exfiltrate or corrupt the
        // vault — a hole the OS `deny_globs` does not close because it only
        // applies inside the sandboxed workspace root, not arbitrary reads.
        denied_paths.push(format!("{}/secrets.vault", config_dir.display()));
        denied_paths.push(format!("{}/secrets.vault.lock", config_dir.display()));
        denied_paths.push(format!("{}/data", config_dir.display()));
        // Note: output directory is intentionally NOT denied
    }

    // Add Unix-specific paths
    #[cfg(unix)]
    {
        denied_paths.extend(["/etc/passwd".to_string(), "/etc/shadow".to_string()]);
    }

    // Add Windows-specific sensitive paths
    #[cfg(target_os = "windows")]
    {
        denied_paths.extend([
            "%APPDATA%\\Microsoft\\Credentials".to_string(),
            "%LOCALAPPDATA%\\Microsoft\\Credentials".to_string(),
            "C:\\Windows\\System32\\config".to_string(),
        ]);
    }

    denied_paths
}

/// Reject glob patterns that would escape the (already deny-checked) base
/// directory: absolute patterns replace the base via `Path::join`, and any
/// `..` component climbs out of it. Relative, non-climbing patterns are safe
/// because every match still lands under `canonical`.
///
/// Uses `has_root()` instead of `is_absolute()` so that root-anchored-but-
/// drive-relative patterns (e.g. `/etc/*` on Windows, which has a root but no
/// drive prefix) are also rejected — they still escape the base via `join`.
pub(crate) fn reject_unsafe_glob_pattern(pattern: &str) -> Result<(), ToolError> {
    let p = std::path::Path::new(pattern);
    if p.has_root() {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must be relative to the search directory: {pattern}"
        )));
    }
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must not contain `..`: {pattern}"
        )));
    }
    Ok(())
}

/// Check if path is allowed and resolve it
///
/// Path resolution rules:
/// 1. Environment variables ($HOME, $USER, etc.) - expanded first
/// 2. Absolute paths (starting with `/`) - used as-is, then rebased through
///    the active [`FsScope`](crate::tools::fs_scope::FsScope) remap when the
///    run is worktree-isolated (parent-repo paths land inside the worktree,
///    mirroring what `WorktreeSandbox` already does for command execution)
/// 3. Home paths (starting with `~`) - expanded to home directory
/// 4. Relative paths - resolved relative to:
///    a. the per-run `FsScope` task-local base — per-run truth, immune to a
///    concurrent run rewriting the shared `ToolContextHandle` mid-run
///    b. `output_dir_override` if provided (workspace-scoped output dir from `ToolContext`)
///    c. Error if neither is available — no global fallback
///
/// The deny check always runs on the FINAL path (post-rebase), so a remap can
/// never smuggle a denied location past the gate.
pub fn check_and_resolve_path(
    path: &Path,
    denied_paths: &[String],
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    info!(path = %path.display(), "check_path: input path");

    // First, expand environment variables in the path string
    let path_str = path.to_string_lossy();
    let expanded_str = if path_str.contains('$') {
        let mut result = path_str.to_string();

        // Expand $HOME
        if let Some(home) = dirs::home_dir() {
            result = result.replace("$HOME", &home.to_string_lossy());
        }

        // Expand $USER
        if let Ok(user) = std::env::var("USER") {
            result = result.replace("$USER", &user);
        }

        // Only expand $HOME and $USER for security — arbitrary env var expansion
        // could allow path injection via attacker-controlled environment variables.

        info!(original = %path_str, expanded = %result, "check_path: expanded environment variables");
        PathBuf::from(result)
    } else {
        path.to_path_buf()
    };

    // Expand ~ to home directory
    let expanded = if expanded_str.starts_with("~/") || expanded_str.as_os_str() == "~" {
        let home = dirs::home_dir()
            .ok_or_else(|| ToolError::InvalidArgs("Cannot determine home directory".to_string()))?;
        home.join(
            expanded_str
                .strip_prefix("~")
                .unwrap_or_else(|_| std::path::Path::new("")),
        )
    } else if expanded_str.is_relative() {
        // Relative paths are resolved to:
        // 1. Per-run FsScope base (task-local — worktree root for isolated
        //    agents, workspace artifact dir for normal runs)
        // 2. ToolContext output_dir override (workspace-scoped, set by ExecutionEngine)
        // 3. Error if neither is available — callers must provide a base directory
        let base_dir = if let Some(scope) = crate::tools::fs_scope::current() {
            info!(fs_scope = %scope.base.display(), "check_path: using per-run FsScope base");
            scope.base
        } else if let Some(override_dir) = output_dir_override {
            info!(output_dir = %override_dir.display(), "check_path: using ToolContext output_dir override");
            override_dir.to_path_buf()
        } else {
            return Err(ToolError::InvalidArgs(
                "Relative path requires an active run scope or an output directory override; \
                 provide an absolute path instead"
                    .to_string(),
            ));
        };
        base_dir.join(expanded_str)
    } else {
        expanded_str
    };

    info!(expanded = %expanded.display(), exists = expanded.exists(), "check_path: expanded path");

    // Canonicalize if exists; for non-existent files, manually normalize to resolve ".."
    // components. This prevents path traversal bypasses (e.g., "/allowed/../secret/file").
    let canonical = if expanded.exists() {
        expanded
            .canonicalize()
            .map_err(|e| ToolError::Execution(format!("Failed to resolve path: {e}")))?
    } else {
        // For non-existent paths, canonicalize the longest existing ancestor
        // then append remaining components. This prevents symlink-based traversal
        // that pure component normalization would miss.
        safe_normalize(&expanded).map_err(|e| {
            ToolError::Execution(format!("Failed to normalize non-existent path: {e}"))
        })?
    };

    info!(canonical = %canonical.display(), "check_path: canonical path");

    // Worktree-isolation remap: when the active FsScope declares a rebase,
    // canonical paths under the parent repo are redirected into the isolated
    // worktree BEFORE the deny check below — the gate therefore evaluates the
    // path that will actually be touched.
    let canonical = match crate::tools::fs_scope::current().and_then(|s| s.rebase_path(&canonical))
    {
        Some(rebased) => {
            info!(
                from = %canonical.display(),
                to = %rebased.display(),
                "check_path: FsScope rebase into isolated worktree"
            );
            // Re-normalize so the result stays canonical (the worktree side
            // may sit behind a symlinked tmpdir) — keeps `path_locks` keys
            // consistent across spellings of the same file.
            safe_normalize(&rebased).map_err(|e| {
                ToolError::Execution(format!("Failed to normalize rebased path: {e}"))
            })?
        }
        None => canonical,
    };

    // Check against denied paths
    let path_str = canonical.to_string_lossy();
    for denied in denied_paths {
        let denied_expanded = if denied.starts_with("~") {
            if let Some(home) = dirs::home_dir() {
                home.join(denied.strip_prefix("~/").unwrap_or(denied))
                    .to_string_lossy()
                    .to_string()
            } else {
                denied.clone()
            }
        } else {
            denied.clone()
        };

        // Use Path::starts_with for proper directory-prefix matching.
        // String starts_with would falsely match "/foo-bar" when "/foo" is denied.
        //
        // Canonicalize the denied path the SAME way as the input (resolving
        // symlinks) before comparing. Otherwise a symlinked ancestor defeats the
        // check: on macOS `/etc` -> `/private/etc`, so a denied literal
        // "/etc/passwd" never prefix-matches the canonical "/private/etc/passwd",
        // silently allowing access.
        let canonical_path = Path::new(&*path_str);
        let denied_norm = safe_normalize(Path::new(&denied_expanded))
            .unwrap_or_else(|_| PathBuf::from(&denied_expanded));
        if canonical_path.starts_with(&denied_norm) {
            info!(
                path_str = %path_str,
                denied = %denied,
                denied_expanded = %denied_expanded,
                "check_path: ACCESS DENIED - path matches denied pattern"
            );
            return Err(ToolError::InvalidArgs(format!(
                "Access denied: {} is in a protected location",
                path.display()
            )));
        }
    }

    info!(canonical = %canonical.display(), "check_path: path allowed");
    Ok(canonical)
}

/// Normalize a non-existent path by canonicalizing the longest existing ancestor,
/// then appending the remaining components. This prevents symlink-based path traversal
/// that pure component-level normalization would miss.
///
/// Returns an error if the longest existing ancestor cannot be canonicalized
/// (e.g., due to permission issues), ensuring we never return an uncanonicalized
/// path that could bypass security checks.
fn safe_normalize(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut remaining = Vec::new();
    while !existing.exists() {
        if let Some(file_name) = existing.file_name() {
            remaining.push(file_name.to_owned());
            existing.pop();
        } else {
            break;
        }
    }
    let mut result = existing.canonicalize().map_err(|e| {
        format!(
            "Failed to canonicalize ancestor '{}': {}",
            existing.display(),
            e
        )
    })?;
    for component in remaining.into_iter().rev() {
        if component == ".." {
            result.pop();
        } else if component != "." {
            result.push(component);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// The denylist must include Aleph's own encrypted vault and the `data/`
    /// auth directory. Asserted by path *suffix* so the test stays hermetic and
    /// independent of where `get_config_dir()` resolves in the test environment
    /// (no `ALEPH_HOME`/`$HOME` mutation, hence no cross-test env leak).
    #[test]
    fn denied_paths_cover_aleph_credential_stores() {
        let denied = get_denied_paths();
        assert!(
            denied.iter().any(|p| p.ends_with("/secrets.vault")),
            "secrets.vault missing from denylist: {denied:?}"
        );
        assert!(
            denied.iter().any(|p| p.ends_with("/data")),
            "data/ auth dir missing from denylist: {denied:?}"
        );
    }

    /// End-to-end enforcement: the vault leaf file is rejected, a file *inside*
    /// the denied `data/` directory is rejected via the canonicalizing prefix
    /// match, and an unrelated sibling under the same root is still allowed.
    #[test]
    fn check_path_blocks_vault_and_data_allows_sibling() {
        let root = tempdir().unwrap();
        let vault = root.path().join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let data = root.path().join("data");
        fs::create_dir(&data).unwrap();
        let pairing = data.join("pairing.db");
        fs::write(&pairing, b"db").unwrap();
        let allowed = root.path().join("output.txt");
        fs::write(&allowed, b"ok").unwrap();

        let denied = vec![
            vault.to_string_lossy().to_string(),
            data.to_string_lossy().to_string(),
        ];

        // Vault leaf file is denied.
        assert!(
            check_and_resolve_path(&vault, &denied, None).is_err(),
            "vault read should be denied"
        );
        // A file inside the denied data/ dir is denied (directory-prefix match).
        assert!(
            check_and_resolve_path(&pairing, &denied, None).is_err(),
            "data/pairing.db read should be denied"
        );
        // An unrelated sibling under the same root is allowed.
        assert!(
            check_and_resolve_path(&allowed, &denied, None).is_ok(),
            "unrelated sibling should be allowed"
        );
    }

    /// Relative paths anchor at the per-run `FsScope` base when one is
    /// published — and the scope wins over the (potentially stale, shared)
    /// `output_dir_override`.
    #[tokio::test]
    async fn fs_scope_base_anchors_relative_paths() {
        let scope_root = tempdir().unwrap();
        let other_root = tempdir().unwrap();
        let scope = crate::tools::fs_scope::FsScope::workspace(scope_root.path().to_path_buf());
        let resolved = crate::tools::fs_scope::with_fs_scope(Some(scope), async {
            check_and_resolve_path(Path::new("sub/file.txt"), &[], Some(other_root.path()))
        })
        .await
        .expect("relative path must resolve inside the scope base");
        let canonical_scope = scope_root.path().canonicalize().unwrap();
        assert_eq!(resolved, canonical_scope.join("sub/file.txt"));
    }

    /// Worktree isolation: an absolute path under the parent repo is rebased
    /// into the worktree checkout before any filesystem access.
    #[tokio::test]
    async fn fs_scope_rebase_redirects_parent_repo_paths() {
        let repo = tempdir().unwrap();
        let wt = tempdir().unwrap();
        fs::create_dir_all(repo.path().join("src")).unwrap();
        fs::write(repo.path().join("src/a.rs"), b"fn main() {}").unwrap();
        let repo_c = repo.path().canonicalize().unwrap();
        let wt_c = wt.path().canonicalize().unwrap();

        let scope = crate::tools::fs_scope::FsScope::worktree(wt_c.clone(), repo_c.clone());
        let input = repo_c.join("src/a.rs");
        let resolved = crate::tools::fs_scope::with_fs_scope(Some(scope), async move {
            check_and_resolve_path(&input, &[], None)
        })
        .await
        .expect("rebase must succeed");
        assert_eq!(resolved, wt_c.join("src/a.rs"));
    }

    /// The deny gate evaluates the FINAL (post-rebase) path — a rebase can
    /// never launder a denied target.
    #[tokio::test]
    async fn fs_scope_rebase_cannot_bypass_deny() {
        let repo = tempdir().unwrap();
        let wt = tempdir().unwrap();
        fs::write(repo.path().join("secret.txt"), b"s").unwrap();
        let repo_c = repo.path().canonicalize().unwrap();
        let wt_c = wt.path().canonicalize().unwrap();

        // Deny the REBASED location only.
        let denied = vec![wt_c.join("secret.txt").to_string_lossy().to_string()];
        let scope = crate::tools::fs_scope::FsScope::worktree(wt_c, repo_c.clone());
        let input = repo_c.join("secret.txt");
        let result = crate::tools::fs_scope::with_fs_scope(Some(scope), async move {
            check_and_resolve_path(&input, &denied, None)
        })
        .await;
        assert!(
            result.is_err(),
            "deny must apply to the post-rebase target, got {result:?}"
        );
    }
}
