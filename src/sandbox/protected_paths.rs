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
pub const PROTECTED_METADATA_SUBPATHS: &[&str] =
    &[".git", ".aleph", ".codex", ".agents"];

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
}
