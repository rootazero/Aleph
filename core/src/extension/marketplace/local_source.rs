//! Local source — resolve and validate local filesystem marketplace paths.

use std::path::PathBuf;

// =============================================================================
// Resolution
// =============================================================================

/// Resolve a local marketplace source path.
///
/// Expands a leading `~` to the user's home directory and validates that the
/// resulting path is an existing directory.
pub fn resolve_local_marketplace(source: &str) -> Result<PathBuf, String> {
    let path = expand_tilde(source);

    if !path.exists() {
        return Err(format!(
            "Local marketplace path does not exist: {}",
            path.display()
        ));
    }

    if !path.is_dir() {
        return Err(format!(
            "Local marketplace path is not a directory: {}",
            path.display()
        ));
    }

    Ok(path)
}

// =============================================================================
// Helpers
// =============================================================================

/// Expand a leading `~` in `source` to the user's home directory.
fn expand_tilde(source: &str) -> PathBuf {
    if source == "~" {
        return home_dir();
    }

    if let Some(rest) = source.strip_prefix("~/") {
        return home_dir().join(rest);
    }

    PathBuf::from(source)
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_existing_dir() {
        let tmp = TempDir::new().unwrap();
        let result = resolve_local_marketplace(tmp.path().to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_nonexistent_dir() {
        let result = resolve_local_marketplace("/nonexistent/path/that/should/not/exist");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("does not exist"), "got: {msg}");
    }

    #[test]
    fn test_resolve_file_not_dir() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("not_a_dir.txt");
        std::fs::write(&file, "hello").unwrap();
        let result = resolve_local_marketplace(file.to_str().unwrap());
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("not a directory"), "got: {msg}");
    }

    #[test]
    fn test_expand_tilde_prefix() {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let expanded = resolve_local_marketplace("~/nonexistent-xyz-abc");
        // Should fail because path doesn't exist, but the error message should
        // reference the home-expanded path, not the literal "~".
        let msg = expanded.unwrap_err();
        assert!(!msg.contains('~'), "tilde should have been expanded, got: {msg}");
        assert!(msg.contains(home.to_str().unwrap()), "got: {msg}");
    }
}
