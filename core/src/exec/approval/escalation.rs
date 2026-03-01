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

    // Partial resolution: canonicalize parent directory + append filename
    if let Some(parent) = path.parent() {
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            if let Some(filename) = path.file_name() {
                return canonical_parent.join(filename);
            }
        }
    }

    // Fallback: lexical normalization (handles ".." without filesystem access)
    normalize_path_components(path)
}

/// Check if path escalation is needed
pub fn check_path_escalation(
    params: &HashMap<String, String>,
    approved_paths: &[String],
) -> Option<EscalationTrigger> {
    for (key, value) in params {
        if key.contains("path") || key.contains("file") || key.contains("dir") {
            // Resolve path with symlink resolution to prevent TOCTOU bypasses.
            // This ensures symlinks like "/tmp/safe -> /etc" are resolved before
            // comparing against approved paths.
            let path = resolve_path_with_symlinks(&PathBuf::from(value));
            let normalized_value = path.to_string_lossy();

            // Check if normalized path is within approved paths.
            // Approved path prefixes are also resolved through symlinks so that
            // both sides use the same canonical form (e.g. /tmp → /private/tmp on macOS).
            let is_approved = approved_paths.iter().any(|approved| {
                if approved.ends_with("/*") {
                    let prefix = approved.trim_end_matches("/*");
                    let resolved_prefix = resolve_path_with_symlinks(&PathBuf::from(prefix));
                    let resolved_prefix_str = resolved_prefix.to_string_lossy();
                    normalized_value.starts_with(resolved_prefix_str.as_ref())
                        || normalized_value.starts_with(prefix)
                } else {
                    let resolved_approved = resolve_path_with_symlinks(&PathBuf::from(approved));
                    *normalized_value == *resolved_approved.to_string_lossy()
                        || *normalized_value == *approved
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
pub fn is_sensitive_directory(path: &Path) -> bool {
    let sensitive_components = [".ssh", ".gnupg", ".aws"];
    let sensitive_substrings = ["Keychain.app", ".config/gcloud"];

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

    // Check full path string for multi-segment patterns
    let path_str = path.to_string_lossy();
    sensitive_substrings
        .iter()
        .any(|pattern| path_str.contains(pattern))
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
