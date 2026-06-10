//! Protected metadata subpaths (codex-inspired).
//!
//! Even when an agent is granted workspace-write access, certain
//! repository-level metadata directories must remain read-only so the
//! agent cannot rewrite its own history, audit trail, or extension
//! manifests. Mirrors codex's protection of `.git`, `.codex`, `.agents`.
//!
//! Each entry is a **relative** path segment that is joined onto every
//! writable root to produce the deny-write rule.

use std::path::{Path, PathBuf};

/// Names of subdirectories that stay read-only even inside writable
/// workspace roots. Stable list — adding entries is backwards-compatible
/// (tighter), removing entries is not.
pub const PROTECTED_METADATA_SUBPATHS: &[&str] = &[".git", ".aleph", ".codex", ".agents"];

/// Cartesian product `writable_roots × PROTECTED_METADATA_SUBPATHS`,
/// returning concrete absolute paths each driver should remount or deny
/// after its writable-root rule. Caller controls the iteration order.
pub fn protected_paths_for<'a, I, P>(writable_roots: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = &'a P>,
    P: AsRef<Path> + 'a + ?Sized,
{
    let mut out = Vec::new();
    for root in writable_roots {
        for sub in PROTECTED_METADATA_SUBPATHS {
            out.push(root.as_ref().join(sub));
        }
    }
    out
}

/// Walk the components of `path` and return the first one that is a
/// **symlink lying within one of `writable_roots`** — i.e. a component
/// the sandboxed process can rewrite at will.
///
/// This is the Rust equivalent of codex's
/// `first_writable_symlink_component_in_path`, and guards a TOCTOU class
/// the rest of Aleph's symlink defense misses: `WorkspaceSandbox` only
/// canonicalizes the *requested cwd* (`workspace.rs`), but a driver that
/// binds / denies a path (e.g. bubblewrap's `--ro-bind-try <ws>/.git`)
/// resolves the symlink target at **mount time**. If any ancestor of that
/// read-only / deny path is a writable symlink, the process can swap it
/// between policy generation and use, redirecting the protected mount to
/// an arbitrary target and silently defeating the protection.
///
/// Only components *inside* a writable root are hazardous: a symlink that
/// sits outside every writable root cannot be rewritten by the sandboxed
/// process, so it is not a TOCTOU vector and is ignored. Components whose
/// metadata cannot be read (absent paths, permission errors) are likewise
/// not symlinks-to-swap and are skipped — the absent-path case is handled
/// separately by synthetic read-only mounts.
///
/// Callers should treat a `Some(_)` result as fatal: a protection that
/// cannot be enforced must fail closed, never silently pass.
#[must_use]
pub fn first_writable_symlink_component(path: &Path, writable_roots: &[&Path]) -> Option<PathBuf> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        // Only attacker-controllable components matter: a symlink outside
        // every writable root cannot be rewritten from inside the sandbox.
        let within_writable = writable_roots.iter().any(|root| current.starts_with(root));
        if !within_writable {
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&current) {
            if meta.file_type().is_symlink() {
                return Some(current.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn constants_cover_codex_set_plus_aleph() {
        let names: Vec<&&str> = PROTECTED_METADATA_SUBPATHS.iter().collect();
        // codex protects .git, .codex, .agents — we mirror those and add
        // .aleph for our own metadata directory.
        assert!(names.iter().any(|n| ***n == *".git"));
        assert!(names.iter().any(|n| ***n == *".codex"));
        assert!(names.iter().any(|n| ***n == *".agents"));
        assert!(names.iter().any(|n| ***n == *".aleph"));
    }

    #[test]
    fn product_joins_each_root_with_each_subpath() {
        let roots = [Path::new("/ws/a"), Path::new("/ws/b")];
        let paths = protected_paths_for(roots.iter().copied());
        assert_eq!(paths.len(), 2 * PROTECTED_METADATA_SUBPATHS.len());
        assert!(paths.contains(&PathBuf::from("/ws/a/.git")));
        assert!(paths.contains(&PathBuf::from("/ws/b/.aleph")));
        assert!(paths.contains(&PathBuf::from("/ws/a/.codex")));
        assert!(paths.contains(&PathBuf::from("/ws/b/.agents")));
    }

    #[test]
    fn empty_roots_yields_empty_product() {
        let roots: [&Path; 0] = [];
        let paths = protected_paths_for(roots.iter().copied());
        assert!(paths.is_empty());
    }

    #[test]
    fn no_symlink_in_path_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        std::fs::create_dir(ws.join(".git")).unwrap();
        let roots = [ws];
        // A real (non-symlink) protected dir crossing only real components
        // is not a TOCTOU hazard.
        assert!(first_writable_symlink_component(&ws.join(".git"), &roots).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_protected_path_inside_writable_root_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        // Attacker turns the protected `.git` into a symlink pointing
        // elsewhere; bwrap would resolve+bind the target at mount time.
        std::os::unix::fs::symlink(&outside, ws.join(".git")).unwrap();
        let roots = [ws];
        let hit = first_writable_symlink_component(&ws.join(".git"), &roots);
        assert_eq!(hit.as_deref(), Some(ws.join(".git").as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_ancestor_inside_writable_root_is_flagged() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let real = ws.join("real");
        std::fs::create_dir(&real).unwrap();
        // A writable *ancestor* component is a symlink — the deepest
        // protected path inherits the hazard.
        std::os::unix::fs::symlink(&real, ws.join("link")).unwrap();
        let roots = [ws];
        let protected = ws.join("link").join(".git");
        let hit = first_writable_symlink_component(&protected, &roots);
        assert_eq!(hit.as_deref(), Some(ws.join("link").as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_outside_writable_roots_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        // The symlink lives under `host`, which is NOT a writable root, so
        // the sandboxed process cannot rewrite it — not a TOCTOU vector.
        let host = tmp.path().join("host");
        std::fs::create_dir(&host).unwrap();
        let target = tmp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, host.join(".git")).unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let roots = [ws.as_path()];
        assert!(first_writable_symlink_component(&host.join(".git"), &roots).is_none());
    }
}
