use super::types::{EscalationReason, EscalationTrigger};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Check if path escalation is needed
pub fn check_path_escalation(
    params: &HashMap<String, String>,
    approved_paths: &[String],
) -> Option<EscalationTrigger> {
    for (key, value) in params {
        if key.contains("path") || key.contains("file") || key.contains("dir") {
            let path = PathBuf::from(value);

            // Check if path is within approved paths
            let is_approved = approved_paths.iter().any(|approved| {
                // Simple glob matching (simplified)
                if approved.ends_with("/*") {
                    let prefix = approved.trim_end_matches("/*");
                    value.starts_with(prefix)
                } else {
                    value == approved
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
}
