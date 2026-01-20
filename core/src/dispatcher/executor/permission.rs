//! File operation permission system
//!
//! Provides path-based permission checking for file operations.
//! Uses allowed/denied path lists with glob pattern support.

use std::path::{Path, PathBuf};

use glob::Pattern;
use tracing::{debug, warn};

/// Error types for file operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum FileOpError {
    #[error("Permission denied: path not in allowed list")]
    PermissionDenied,

    #[error("Path denied: matches blocked pattern")]
    PathDenied,

    #[error("File not found: {0}")]
    NotFound(PathBuf),

    #[error("File size exceeds limit: {size} bytes > {limit} bytes")]
    SizeLimitExceeded { size: u64, limit: u64 },

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("IO error: {0}")]
    IoError(String),

    #[error("Operation requires confirmation")]
    RequiresConfirmation,

    #[error("Path traversal detected")]
    PathTraversal,
}

impl From<std::io::Error> for FileOpError {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => FileOpError::NotFound(PathBuf::from(err.to_string())),
            std::io::ErrorKind::PermissionDenied => FileOpError::PermissionDenied,
            _ => FileOpError::IoError(err.to_string()),
        }
    }
}

/// File operation permission checker
///
/// Validates paths against allowed and denied lists before allowing operations.
#[derive(Debug, Clone)]
pub struct PathPermissionChecker {
    /// Allowed path patterns (glob patterns) - kept for debugging/display
    #[allow(dead_code)]
    allowed_paths: Vec<String>,

    /// Denied path patterns (glob patterns) - kept for debugging/display
    #[allow(dead_code)]
    denied_paths: Vec<String>,

    /// Maximum file size in bytes (0 = unlimited)
    max_file_size: u64,

    /// Compiled allowed patterns
    allowed_patterns: Vec<Pattern>,

    /// Compiled denied patterns
    denied_patterns: Vec<Pattern>,
}

impl Default for PathPermissionChecker {
    fn default() -> Self {
        Self::new(vec![], vec![], 0)
    }
}

impl PathPermissionChecker {
    /// Get default denied paths for security (platform-aware)
    ///
    /// Returns a list of sensitive paths that should always be denied,
    /// including platform-specific paths for Windows.
    pub fn default_denied_paths() -> Vec<String> {
        let mut paths = vec![
            "~/.ssh/**".to_string(),
            "~/.gnupg/**".to_string(),
            "~/.aws/**".to_string(),
            "~/.kube/**".to_string(),
        ];

        // Add Aether config directory dynamically (cross-platform)
        if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
            paths.push(format!("{}/**", config_dir.display()));
        }

        // Platform-specific paths
        #[cfg(unix)]
        paths.extend([
            "/etc/passwd".to_string(),
            "/etc/shadow".to_string(),
            "/etc/sudoers".to_string(),
        ]);

        #[cfg(target_os = "windows")]
        paths.extend([
            "%APPDATA%\\Microsoft\\Credentials\\**".to_string(),
            "%LOCALAPPDATA%\\Microsoft\\Credentials\\**".to_string(),
            "C:\\Windows\\System32\\config\\**".to_string(),
        ]);

        paths
    }

    /// Create a new permission checker
    pub fn new(allowed_paths: Vec<String>, denied_paths: Vec<String>, max_file_size: u64) -> Self {
        // Expand ~ and canonicalize base paths
        let allowed_expanded: Vec<String> = allowed_paths
            .iter()
            .map(|p| Self::expand_and_canonicalize(p))
            .collect();

        let denied_expanded: Vec<String> = denied_paths
            .iter()
            .cloned()
            .chain(Self::default_denied_paths())
            .map(|p| Self::expand_and_canonicalize(&p))
            .collect();

        // Compile patterns
        let allowed_patterns = allowed_expanded
            .iter()
            .filter_map(|p| {
                Pattern::new(p)
                    .map_err(|e| warn!("Invalid allowed pattern '{}': {}", p, e))
                    .ok()
            })
            .collect();

        let denied_patterns = denied_expanded
            .iter()
            .filter_map(|p| {
                Pattern::new(p)
                    .map_err(|e| warn!("Invalid denied pattern '{}': {}", p, e))
                    .ok()
            })
            .collect();

        Self {
            allowed_paths: allowed_expanded,
            denied_paths: denied_expanded,
            max_file_size,
            allowed_patterns,
            denied_patterns,
        }
    }

    /// Expand ~ to home directory
    fn expand_tilde(path: &str) -> String {
        if path.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return path.replacen("~", home.to_string_lossy().as_ref(), 1);
            }
        } else if path == "~" {
            if let Some(home) = dirs::home_dir() {
                return home.to_string_lossy().to_string();
            }
        }
        path.to_string()
    }

    /// Expand ~ and canonicalize the base path of a pattern
    ///
    /// For patterns like "/tmp/**", extracts "/tmp", canonicalizes it,
    /// and returns the canonicalized path with the suffix.
    fn expand_and_canonicalize(path: &str) -> String {
        let expanded = Self::expand_tilde(path);

        // Extract suffix (e.g., "/**" or "/*")
        let (base, suffix) = if expanded.ends_with("/**") {
            (&expanded[..expanded.len() - 3], "/**")
        } else if expanded.ends_with("/*") {
            (&expanded[..expanded.len() - 2], "/*")
        } else {
            (expanded.as_str(), "")
        };

        // Try to canonicalize the base path
        let base_path = Path::new(base);
        if let Ok(canonical) = base_path.canonicalize() {
            format!("{}{}", canonical.to_string_lossy(), suffix)
        } else {
            // If canonicalization fails (path doesn't exist), return expanded path
            expanded
        }
    }

    /// Canonicalize a path (resolve symlinks, .., etc.)
    fn canonicalize_path(&self, path: &Path) -> Result<PathBuf, FileOpError> {
        // First expand ~ if present
        let path_str = path.to_string_lossy();
        let expanded = Self::expand_tilde(&path_str);
        let expanded_path = PathBuf::from(&expanded);

        // Try to canonicalize. If the file doesn't exist, resolve what we can
        match expanded_path.canonicalize() {
            Ok(canonical) => Ok(canonical),
            Err(_) => {
                // File doesn't exist yet, canonicalize parent and append filename
                if let Some(parent) = expanded_path.parent() {
                    if let Ok(canonical_parent) = parent.canonicalize() {
                        if let Some(filename) = expanded_path.file_name() {
                            return Ok(canonical_parent.join(filename));
                        }
                    }
                }
                // Fall back to the expanded path
                Ok(expanded_path)
            }
        }
    }

    /// Check if a path is within allowed paths
    fn is_path_allowed(&self, canonical_path: &Path) -> bool {
        // If no allowed paths configured, allow all (except denied)
        if self.allowed_patterns.is_empty() {
            return true;
        }

        let path_str = canonical_path.to_string_lossy();

        for pattern in &self.allowed_patterns {
            if pattern.matches(&path_str) {
                return true;
            }

            // Also check if path starts with allowed pattern (for directories)
            // Convert pattern to prefix check
            let pattern_str = pattern.as_str();
            let pattern_prefix = pattern_str.trim_end_matches("/**").trim_end_matches("/*");
            if path_str.starts_with(pattern_prefix) {
                return true;
            }
        }

        false
    }

    /// Check if a path is in denied paths
    fn is_path_denied(&self, canonical_path: &Path) -> bool {
        let path_str = canonical_path.to_string_lossy();

        for pattern in &self.denied_patterns {
            if pattern.matches(&path_str) {
                return true;
            }

            // Also check prefix match
            let pattern_str = pattern.as_str();
            let pattern_prefix = pattern_str.trim_end_matches("/**").trim_end_matches("/*");
            if path_str.starts_with(pattern_prefix) {
                return true;
            }
        }

        false
    }

    /// Check if a path contains traversal attempts
    fn has_path_traversal(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        path_str.contains("..") || path_str.contains("./")
    }

    /// Check if a path is allowed for the given permission level
    pub fn check_path(&self, path: &Path) -> Result<PathBuf, FileOpError> {
        // Check for path traversal
        if self.has_path_traversal(path) {
            warn!("Path traversal attempt detected: {:?}", path);
            return Err(FileOpError::PathTraversal);
        }

        // Canonicalize path
        let canonical = self.canonicalize_path(path)?;

        // Check denied paths first (takes precedence)
        if self.is_path_denied(&canonical) {
            debug!("Path denied: {:?}", canonical);
            return Err(FileOpError::PathDenied);
        }

        // Check allowed paths
        if !self.is_path_allowed(&canonical) {
            debug!("Path not in allowed list: {:?}", canonical);
            return Err(FileOpError::PermissionDenied);
        }

        Ok(canonical)
    }

    /// Check if a file size is within limits
    pub fn check_file_size(&self, size: u64) -> Result<(), FileOpError> {
        if self.max_file_size > 0 && size > self.max_file_size {
            return Err(FileOpError::SizeLimitExceeded {
                size,
                limit: self.max_file_size,
            });
        }
        Ok(())
    }

    /// Get the max file size limit
    pub fn max_file_size(&self) -> u64 {
        self.max_file_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde() {
        let expanded = PathPermissionChecker::expand_tilde("~/Documents");
        assert!(!expanded.starts_with("~"));
        assert!(expanded.contains("Documents"));
    }

    #[test]
    fn test_default_denied_paths() {
        let checker = PathPermissionChecker::new(vec!["~/**".to_string()], vec![], 0);

        // Should deny ~/.ssh even if ~ is allowed
        let ssh_path = PathPermissionChecker::expand_tilde("~/.ssh/id_rsa");
        let result = checker.check_path(Path::new(&ssh_path));
        assert!(matches!(result, Err(FileOpError::PathDenied)));
    }

    #[test]
    fn test_allowed_paths() {
        let checker = PathPermissionChecker::new(vec!["/tmp/**".to_string()], vec![], 0);

        // Should allow /tmp paths
        let result = checker.check_path(Path::new("/tmp/test.txt"));
        assert!(result.is_ok());

        // Should deny paths outside /tmp
        let result = checker.check_path(Path::new("/etc/passwd"));
        // This will be denied because it matches default denied paths
        assert!(result.is_err());
    }

    #[test]
    fn test_file_size_limit() {
        let checker = PathPermissionChecker::new(
            vec![],
            vec![],
            1024 * 1024, // 1MB
        );

        assert!(checker.check_file_size(1024).is_ok());
        assert!(checker.check_file_size(1024 * 1024 + 1).is_err());
    }

    #[test]
    fn test_path_traversal_detection() {
        let checker = PathPermissionChecker::new(vec!["/tmp/**".to_string()], vec![], 0);

        let result = checker.check_path(Path::new("/tmp/../etc/passwd"));
        assert!(matches!(result, Err(FileOpError::PathTraversal)));
    }
}
