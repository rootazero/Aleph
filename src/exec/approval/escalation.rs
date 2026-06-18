use super::types::{EscalationReason, EscalationTrigger};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Resolve a path by following symlinks where possible.
///
/// Tries `fs::canonicalize()` first (resolves all symlinks + normalizes).
/// If the full path doesn't exist, tries canonicalizing the parent and appending the filename.
/// Falls back to lexical normalization as a last resort.
fn resolve_path_with_symlinks(path: &Path) -> PathBuf {
    // Best case: full path exists, canonicalize resolves all symlinks
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }

    // Partial resolution: try to canonicalize the deepest existing ancestor
    // and resolve any symlinks in the prefix, then append the remaining suffix.
    // This prevents symlink bypasses like /tmp/safe_link -> /etc combined with
    // non-existent subpaths.
    let mut current = path.to_path_buf();
    while let Some(parent) = current.parent() {
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            if let Ok(suffix) = path.strip_prefix(parent) {
                return normalize_path_components(&canonical_parent.join(suffix));
            }
        }
        current = parent.to_path_buf();
    }

    // Fallback: lexical normalization (handles ".." without filesystem access)
    normalize_path_components(path)
}

/// Check if path escalation is needed
#[must_use]
pub fn check_path_escalation(
    params: &HashMap<String, String>,
    approved_paths: &[String],
) -> Option<EscalationTrigger> {
    for (key, value) in params {
        if value.contains('\0') {
            return Some(EscalationTrigger {
                reason: EscalationReason::PathOutOfScope,
                requested_path: Some(PathBuf::from(value)),
                approved_paths: approved_paths.to_vec(),
            });
        }

        if key.contains("path") || key.contains("file") || key.contains("dir") {
            // Resolve path with symlink resolution to prevent TOCTOU bypasses.
            // This ensures symlinks like "/tmp/safe -> /etc" are resolved before
            // comparing against approved paths.
            let path = resolve_path_with_symlinks(&PathBuf::from(value));
            let normalized_value = path.to_string_lossy();

            // Check if normalized path is within approved paths.
            // Approved path prefixes are also resolved through symlinks so that
            // both sides use the same canonical form (e.g. /tmp → /private/tmp on macOS).
            // Guard: reject empty paths before scope checking
            if value.is_empty() || normalized_value.is_empty() {
                return Some(EscalationTrigger {
                    reason: EscalationReason::PathOutOfScope,
                    requested_path: Some(path),
                    approved_paths: approved_paths.to_vec(),
                });
            }

            // Use Path::starts_with (component-based) instead of string starts_with
            // to prevent prefix confusion attacks (e.g. /tmp-evil matching /tmp/*)
            let is_approved = approved_paths.iter().any(|approved| {
                if approved.ends_with("/*") {
                    let prefix = approved.trim_end_matches("/*");
                    let resolved_prefix = resolve_path_with_symlinks(&PathBuf::from(prefix));
                    // Compare only canonical (symlink-resolved) forms on both
                    // sides — accepting the raw approved prefix would let a
                    // symlinked approved dir match a request whose resolved path
                    // lives elsewhere, undoing the TOCTOU canonicalization.
                    path.starts_with(&resolved_prefix)
                } else {
                    let resolved_approved = resolve_path_with_symlinks(&PathBuf::from(approved));
                    path == resolved_approved
                }
            });

            if !is_approved {
                return Some(EscalationTrigger {
                    reason: EscalationReason::PathOutOfScope,
                    requested_path: Some(path),
                    approved_paths: approved_paths.to_vec(),
                });
            }

            // Check if sensitive directory
            if is_sensitive_directory(&path) {
                return Some(EscalationTrigger {
                    reason: EscalationReason::SensitiveDirectory,
                    requested_path: Some(path),
                    approved_paths: approved_paths.to_vec(),
                });
            }
        }
    }

    None
}

/// Normalize path by resolving ".." components without filesystem access.
/// This prevents path traversal attacks (e.g., "/tmp/../etc/passwd" → "/etc/passwd").
fn normalize_path_components(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Pop the last component (if possible)
                normalized.pop();
            }
            Component::CurDir => {
                // Skip "." components
            }
            _ => {
                normalized.push(component);
            }
        }
    }
    normalized
}

/// Check if path is in sensitive directory
#[must_use]
pub fn is_sensitive_directory(path: &Path) -> bool {
    let sensitive_components = [
        ".ssh",
        ".gnupg",
        ".aws",
        ".docker",
        ".kube",
        ".azure",
        ".oci",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".git-credentials",
        ".bash_history",
        ".zsh_history",
        ".python_history",
        "Keychain.app",
    ];
    // Multi-segment sensitive paths that must match as complete path components
    let sensitive_path_segments = [(".config", "gcloud"), (".config", "gh")];

    // Check path components for exact directory matches
    let has_sensitive_component = path.components().any(|comp| {
        if let std::path::Component::Normal(name) = comp {
            let name_str = name.to_string_lossy();
            sensitive_components.iter().any(|&dir| name_str == dir)
        } else {
            false
        }
    });

    if has_sensitive_component {
        return true;
    }

    // Check for multi-segment sensitive paths using component-level matching
    // to avoid substring false positives (e.g. ".config/gcloud-backup" should not match)
    let components: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();

    for i in 0..components.len().saturating_sub(1) {
        for &(parent, child) in &sensitive_path_segments {
            if components[i] == parent && components[i + 1] == child {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_path_out_of_scope_detection() {
        let approved_paths = vec!["/tmp/*".to_string()];
        let mut params = HashMap::new();
        params.insert("file_path".to_string(), "/etc/passwd".to_string());

        let trigger = check_path_escalation(&params, &approved_paths);
        assert!(trigger.is_some());
        assert_eq!(trigger.unwrap().reason, EscalationReason::PathOutOfScope);
    }

    #[test]
    fn test_sensitive_directory_detection() {
        let path = PathBuf::from("/Users/test/.ssh/id_rsa");
        assert!(is_sensitive_directory(&path));

        let path = PathBuf::from("/Users/test/Documents/file.txt");
        assert!(!is_sensitive_directory(&path));
    }

    #[test]
    fn test_sensitive_directory_no_false_positive() {
        // Substring matches should not trigger (e.g. "gcloud-backup" vs "gcloud")
        let path = PathBuf::from("/Users/test/.config/gcloud-backup/data");
        assert!(!is_sensitive_directory(&path));

        // Exact match should still trigger
        let path = PathBuf::from("/Users/test/.config/gcloud/credentials");
        assert!(is_sensitive_directory(&path));
    }

    #[test]
    #[cfg(unix)] // POSIX-only: asserts /etc/passwd-style lexical path normalization
    fn test_resolve_path_with_symlinks_nonexistent() {
        // Non-existent path falls back to lexical normalization
        let path = PathBuf::from("/nonexistent/../etc/passwd");
        let resolved = resolve_path_with_symlinks(&path);
        assert_eq!(resolved, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn test_resolve_path_with_symlinks_real_path() {
        // Real path should be canonicalized
        let path = PathBuf::from("/tmp/./");
        let resolved = resolve_path_with_symlinks(&path);
        // canonicalize should resolve /tmp to its real path (e.g. /private/tmp on macOS)
        assert!(!resolved.to_string_lossy().contains("./"));
    }

    #[test]
    fn test_symlink_escalation_detection() {
        // Even through symlink resolution, /etc/passwd should not be in /tmp/*
        let approved_paths = vec!["/tmp/*".to_string()];
        let mut params = HashMap::new();
        params.insert("file_path".to_string(), "/tmp/../etc/passwd".to_string());

        let trigger = check_path_escalation(&params, &approved_paths);
        assert!(trigger.is_some());
        assert_eq!(trigger.unwrap().reason, EscalationReason::PathOutOfScope);
    }
}
