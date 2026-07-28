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

    // Add Unix-specific paths. Beyond the classic credential files, deny the
    // privilege-escalation / persistence surfaces an agent's file tools must
    // never read or clobber — writing any of these is a host-takeover vector
    // (sudoers, cron, PAM, the dynamic-linker preload hook), and reading the
    // SSH host-key dir or root's home leaks credentials. Mirrors hermes-agent's
    // `_SENSITIVE_PATH_PREFIXES`; each is a directory or leaf covered by the
    // canonicalizing prefix match below.
    #[cfg(unix)]
    {
        denied_paths.extend([
            "/etc/passwd".to_string(),
            "/etc/shadow".to_string(),
            "/etc/sudoers".to_string(),
            "/etc/sudoers.d".to_string(),
            "/etc/ssh".to_string(),
            "/etc/pam.d".to_string(),
            "/etc/crontab".to_string(),
            "/etc/cron.d".to_string(),
            "/etc/ld.so.preload".to_string(),
            "/root/.ssh".to_string(),
        ]);
    }

    // Add Windows-specific sensitive paths. The `%APPDATA%` / `%LOCALAPPDATA%`
    // tokens are expanded at match time by [`path_is_denied`] — without that
    // two of these three rules never fire (a canonical path never literally
    // contains `%APPDATA%`).
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

/// Expand a denylist entry's leading `~` (home) and Windows environment tokens
/// (`%APPDATA%` / `%LOCALAPPDATA%` / `%USERPROFILE%`) to concrete paths so the
/// prefix comparison below sees the same shape a canonical path has. Unix
/// entries carry no `%…%` tokens, so the Windows expansion is a no-op there.
fn expand_denied_entry(denied: &str) -> String {
    // `mut` is only exercised on Windows (the env-token expansion below); on
    // other targets the binding is written once.
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut out = if denied.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            home.join(denied.strip_prefix("~/").unwrap_or(denied))
                .to_string_lossy()
                .to_string()
        } else {
            denied.to_string()
        }
    } else {
        denied.to_string()
    };
    #[cfg(target_os = "windows")]
    {
        for (token, var) in [
            ("%APPDATA%", "APPDATA"),
            ("%LOCALAPPDATA%", "LOCALAPPDATA"),
            ("%USERPROFILE%", "USERPROFILE"),
        ] {
            if out.contains(token) {
                if let Ok(val) = std::env::var(var) {
                    out = out.replace(token, &val);
                }
            }
        }
    }
    out
}

/// Whether an already-canonical path falls under any denylist entry.
///
/// The single source of truth for the deny check, shared by
/// [`check_and_resolve_path`] and by the per-entry re-checks that enumeration /
/// relocation operations (`stats`, `organize`, recursive `copy`) run on paths
/// they discover *after* the initial gate — a symlink or glob match can point
/// at a denied target the top-level path never named. Each entry is expanded
/// ([`expand_denied_entry`]) and normalized the SAME way as the input (resolving
/// symlinks in existing ancestors) before the component-wise prefix compare, so
/// a symlinked ancestor (`/etc` → `/private/etc` on macOS) cannot defeat it.
pub fn path_is_denied(canonical: &Path, denied_paths: &[String]) -> bool {
    for denied in denied_paths {
        let denied_expanded = expand_denied_entry(denied);
        let denied_norm = safe_normalize(Path::new(&denied_expanded))
            .unwrap_or_else(|_| PathBuf::from(&denied_expanded));
        if canonical.starts_with(&denied_norm) {
            return true;
        }
    }
    false
}

/// Whether `canonical` is a Linux `/proc/<pid>/…` pseudo-file that leaks another
/// process's secrets (environment, memory, mappings). These are not covered by
/// the credential denylist and are not regular files an agent has any business
/// reading — `/proc/<pid>/environ` alone exposes every exported secret of a
/// running process. Defense-in-depth mirroring hermes-agent's
/// `_is_blocked_device_path`; a no-op on non-Linux where `/proc` is absent.
pub fn is_blocked_proc_path(canonical: &Path) -> bool {
    use std::path::Component;
    let mut comps = canonical.components();
    // Must be rooted at `/proc/<something>/…`.
    if comps.next() != Some(Component::RootDir) {
        return false;
    }
    if comps.next() != Some(Component::Normal(std::ffi::OsStr::new("proc"))) {
        return false;
    }
    // `<pid>` (or `self` / `thread-self`) — any single component.
    if comps.next().is_none() {
        return false;
    }
    // Block the secret-bearing leaves anywhere below the pid dir.
    const BLOCKED_LEAVES: &[&str] = &[
        "environ",
        "cmdline",
        "mem",
        "maps",
        "smaps",
        "smaps_rollup",
        "numa_maps",
        "auxv",
        "pagemap",
        "stack",
        "syscall",
    ];
    canonical
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|leaf| BLOCKED_LEAVES.contains(&leaf))
}

/// Reject glob patterns that would escape the (already deny-checked) base
/// directory: absolute patterns replace the base via `Path::join`, and any
/// `..` component climbs out of it. Relative, non-climbing patterns are safe
/// because every match still lands under `canonical`.
///
/// Uses `has_root()` instead of `is_absolute()` so that root-anchored-but-
/// drive-relative patterns (e.g. `/etc/*` on Windows, which has a root but no
/// drive prefix) are also rejected — they still escape the base via `join`.
///
/// Additionally rejects any pattern containing a drive or UNC prefix
/// (`Component::Prefix`) — e.g. `C:foo` on Windows. Such patterns are not
/// root-anchored (`has_root()` returns false) yet `Path::join(base, "C:foo")`
/// discards the base entirely and resolves relative to drive C's current
/// directory, bypassing the deny-checked base. On Unix `Component::Prefix`
/// never occurs, so this check is a safe no-op there.
pub(crate) fn reject_unsafe_glob_pattern(pattern: &str) -> Result<(), ToolError> {
    let p = std::path::Path::new(pattern);
    if p.has_root() {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must be relative to the search directory: {pattern}"
        )));
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::Prefix(_)))
    {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must not contain a drive/UNC prefix: {pattern}"
        )));
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ToolError::InvalidArgs(format!(
            "Glob pattern must not contain `..`: {pattern}"
        )));
    }
    Ok(())
}

/// Expand `$HOME`/`$USER`, a leading `~`, and a relative base into a concrete
/// path **without canonicalizing** — so a final-component symlink is preserved
/// (canonicalization would resolve it to its target). Shared by
/// [`check_and_resolve_path`] and [`resolve_for_removal`].
fn expand_input_path(
    path: &Path,
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
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
        PathBuf::from(result)
    } else {
        path.to_path_buf()
    };

    // Expand ~ to home directory
    if expanded_str.starts_with("~/") || expanded_str.as_os_str() == "~" {
        let home = dirs::home_dir()
            .ok_or_else(|| ToolError::InvalidArgs("Cannot determine home directory".to_string()))?;
        Ok(home.join(
            expanded_str
                .strip_prefix("~")
                .unwrap_or_else(|_| std::path::Path::new("")),
        ))
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
        Ok(base_dir.join(expanded_str))
    } else {
        Ok(expanded_str)
    }
}

/// Resolve a path for a **removal or rename** whose final component must NOT be
/// followed when it is a symlink.
///
/// `check_and_resolve_path` canonicalizes a final-component symlink to its
/// *target*; a `delete`/`move` acting on that target would destroy the tree the
/// link points at and leave the link dangling (or move the target out from
/// under it). Filesystem `remove_file` / `rename` never follow a final symlink,
/// so operating on the link path is both correct and what the user meant.
///
/// The full deny check still runs against the resolved target (via
/// [`check_and_resolve_path`]), and the link's own location is deny-checked too,
/// so neither the link nor its target can name a protected location. Returns the
/// path to operate on: the un-followed link when the final component is a
/// symlink, otherwise the canonical target (identical to
/// `check_and_resolve_path`).
pub fn resolve_for_removal(
    path: &Path,
    denied_paths: &[String],
    output_dir_override: Option<&Path>,
) -> Result<PathBuf, ToolError> {
    // Deny-check the resolved target first (conservative: a link whose target is
    // protected cannot be used as a handle to it).
    let canonical_target = check_and_resolve_path(path, denied_paths, output_dir_override)?;

    let expanded = expand_input_path(path, output_dir_override)?;
    let is_symlink = std::fs::symlink_metadata(&expanded)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if !is_symlink {
        return Ok(canonical_target);
    }

    // The final component is a symlink: operate on the LINK, not its target.
    // Canonicalize only the PARENT (resolving any intermediate symlinks + the
    // FsScope rebase) and re-attach the un-followed final component.
    let Some(file_name) = expanded.file_name() else {
        return Ok(canonical_target);
    };
    let parent = expanded.parent().unwrap_or_else(|| Path::new("/"));
    let canon_parent = safe_normalize(parent)
        .map_err(|e| ToolError::Execution(format!("Failed to resolve parent: {e}")))?;
    let canon_parent =
        match crate::tools::fs_scope::current().and_then(|s| s.rebase_path(&canon_parent)) {
            Some(rebased) => safe_normalize(&rebased).map_err(|e| {
                ToolError::Execution(format!("Failed to normalize rebased parent: {e}"))
            })?,
            None => canon_parent,
        };
    let link_path = canon_parent.join(file_name);
    if path_is_denied(&link_path, denied_paths) {
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} is in a protected location",
            path.display()
        )));
    }
    Ok(link_path)
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

    // Env-var / `~` / relative-base expansion (NO canonicalization — a final
    // symlink is preserved). Shared with `resolve_for_removal` so the two
    // resolvers cannot drift on how a spelled path becomes a filesystem path.
    let expanded = expand_input_path(path, output_dir_override)?;

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

    // Check against denied paths. Uses Path-component prefix matching (not
    // string starts_with, which would falsely match "/foo-bar" against "/foo")
    // via the shared `path_is_denied` helper, which canonicalizes each denied
    // entry the same way as the input so a symlinked ancestor (macOS
    // `/etc` -> `/private/etc`) cannot defeat it.
    if path_is_denied(&canonical, denied_paths) {
        info!(
            canonical = %canonical.display(),
            "check_path: ACCESS DENIED - path matches denied pattern"
        );
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} is in a protected location",
            path.display()
        )));
    }

    // Defense-in-depth: block `/proc/<pid>/{environ,maps,mem,…}` secret-bearing
    // pseudo-files regardless of the credential denylist.
    if is_blocked_proc_path(&canonical) {
        info!(
            canonical = %canonical.display(),
            "check_path: ACCESS DENIED - /proc secret pseudo-file"
        );
        return Err(ToolError::InvalidArgs(format!(
            "Access denied: {} exposes another process's secrets",
            path.display()
        )));
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

    // --- reject_unsafe_glob_pattern ---

    #[test]
    fn glob_guard_allows_relative_patterns() {
        assert!(
            reject_unsafe_glob_pattern("*.txt").is_ok(),
            "bare wildcard must be accepted"
        );
        assert!(
            reject_unsafe_glob_pattern("images/photo.jpg").is_ok(),
            "relative sub-path must be accepted"
        );
        assert!(
            reject_unsafe_glob_pattern("**/foo").is_ok(),
            "recursive glob must be accepted"
        );
    }

    #[test]
    fn glob_guard_rejects_root_anchored() {
        assert!(
            matches!(
                reject_unsafe_glob_pattern("/etc/*"),
                Err(ToolError::InvalidArgs(_))
            ),
            "/etc/* is root-anchored and must be rejected"
        );
    }

    #[test]
    fn glob_guard_rejects_parent_dir() {
        assert!(
            matches!(
                reject_unsafe_glob_pattern("../secrets"),
                Err(ToolError::InvalidArgs(_))
            ),
            "../secrets contains `..` and must be rejected"
        );
    }

    #[cfg(windows)]
    #[test]
    fn glob_guard_rejects_drive_relative_prefix() {
        // On Windows, `C:foo` has a Prefix component but no root — Path::join
        // with any base replaces the base entirely, so it must be rejected.
        assert!(
            matches!(
                reject_unsafe_glob_pattern("C:foo"),
                Err(ToolError::InvalidArgs(_))
            ),
            "C:foo is a drive-relative pattern and must be rejected on Windows"
        );
    }

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

    #[test]
    fn path_is_denied_matches_directory_prefix_not_string_prefix() {
        let root = tempdir().unwrap();
        let secret_dir = root.path().join("secret");
        fs::create_dir(&secret_dir).unwrap();
        let sibling = root.path().join("secret-sibling");
        fs::create_dir(&sibling).unwrap();
        let denied = vec![secret_dir.to_string_lossy().to_string()];
        // `path_is_denied` expects an already-canonical input (its contract);
        // canonicalize the dirs so a symlinked tempdir root (macOS
        // `/var` → `/private/var`) does not defeat the prefix compare.
        let secret_c = secret_dir.canonicalize().unwrap();
        let sibling_c = sibling.canonicalize().unwrap();

        assert!(path_is_denied(&secret_c.join("k.pem"), &denied));
        // A string-prefix sibling ("secret-sibling") must NOT match.
        assert!(!path_is_denied(&sibling_c.join("ok.txt"), &denied));
    }

    #[cfg(unix)]
    #[test]
    fn is_blocked_proc_path_flags_secret_leaves_only() {
        use std::path::Path;
        assert!(is_blocked_proc_path(Path::new("/proc/1234/environ")));
        assert!(is_blocked_proc_path(Path::new("/proc/self/maps")));
        assert!(is_blocked_proc_path(Path::new("/proc/1/mem")));
        // A benign /proc leaf and non-/proc paths are allowed.
        assert!(!is_blocked_proc_path(Path::new("/proc/1234/status")));
        assert!(!is_blocked_proc_path(Path::new("/proc/cpuinfo")));
        assert!(!is_blocked_proc_path(Path::new("/home/u/environ")));
    }

    #[cfg(unix)]
    #[test]
    fn denied_paths_cover_privilege_escalation_surfaces() {
        let denied = get_denied_paths();
        for p in ["/etc/sudoers", "/etc/cron.d", "/etc/pam.d", "/root/.ssh"] {
            assert!(
                denied.iter().any(|d| d == p),
                "{p} missing from denylist: {denied:?}"
            );
        }
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
