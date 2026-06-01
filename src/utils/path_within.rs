//! Lexical path-containment check (no filesystem touch, no symlink TOCTOU).

use std::path::{Component, Path, PathBuf};

/// Returns true iff `target`, lexically normalized, stays within `base`.
pub fn is_path_within(base: &Path, target: &Path) -> bool {
    if base.as_os_str().is_empty() {
        return false;
    }
    let mut normalized = PathBuf::new();
    for component in target.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(c) => normalized.push(c),
            Component::RootDir => normalized.push(Component::RootDir.as_os_str()),
            Component::Prefix(p) => normalized.push(p.as_os_str()),
            Component::CurDir => {}
        }
    }

    // strip_prefix succeeds only when base is a true prefix (component-level),
    // so any Ok result means target is either base itself or inside it.
    normalized.strip_prefix(base).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_and_outside() {
        let base = PathBuf::from("/skills/demo");
        assert!(is_path_within(&base, &base.join("references/x.md")));
        assert!(!is_path_within(
            &base,
            &PathBuf::from("/skills/demo/../other/x")
        ));
    }

    #[test]
    fn prefix_confusion_not_allowed() {
        let base = PathBuf::from("/skills/demo");
        assert!(!is_path_within(
            &base,
            &PathBuf::from("/skills/demonstration/x.md")
        ));
        assert!(!is_path_within(&base, &PathBuf::from("/skills/demo2/x.md")));
    }

    #[test]
    fn exact_base_is_within() {
        let base = PathBuf::from("/skills/demo");
        assert!(is_path_within(&base, &base));
    }
}
