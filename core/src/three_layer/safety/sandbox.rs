//! Path Sandbox - restricts file system access to allowed directories

use regex::Regex;
use std::path::{Path, PathBuf};

/// Violation of sandbox rules
#[derive(Debug, Clone)]
pub enum SandboxViolation {
    /// Path is outside allowed root directories
    OutsideAllowedRoots,
    /// Path matches a denied pattern
    DeniedPattern { pattern: String },
    /// Symlink escape attempt detected
    SymlinkEscape,
    /// Path traversal attempt detected (e.g., ..)
    PathTraversal,
    /// Path does not exist
    NotFound,
    /// IO error during validation
    IoError(String),
}

impl std::fmt::Display for SandboxViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxViolation::OutsideAllowedRoots => {
                write!(f, "Path is outside allowed root directories")
            }
            SandboxViolation::DeniedPattern { pattern } => {
                write!(f, "Path matches denied pattern: {}", pattern)
            }
            SandboxViolation::SymlinkEscape => {
                write!(f, "Symlink escape attempt detected")
            }
            SandboxViolation::PathTraversal => {
                write!(f, "Path traversal attempt detected")
            }
            SandboxViolation::NotFound => {
                write!(f, "Path does not exist")
            }
            SandboxViolation::IoError(e) => {
                write!(f, "IO error: {}", e)
            }
        }
    }
}

impl std::error::Error for SandboxViolation {}

/// Sandbox that restricts file system access
///
/// Only allows access to files within specified root directories,
/// and denies access to files matching certain patterns (e.g., .env, .git).
#[derive(Debug, Clone)]
pub struct PathSandbox {
    /// Allowed root directories
    allowed_roots: Vec<PathBuf>,
    /// Denied path patterns (regex)
    denied_patterns: Vec<String>,
    /// Compiled regex patterns (not Clone, so we store strings and compile on demand)
    #[allow(dead_code)]
    compiled_patterns: Vec<Regex>,
}

impl PathSandbox {
    /// Create a new sandbox with specified allowed roots
    pub fn new(allowed_roots: Vec<PathBuf>) -> Self {
        Self {
            allowed_roots,
            denied_patterns: Vec::new(),
            compiled_patterns: Vec::new(),
        }
    }

    /// Create a sandbox with sensible default denied patterns
    ///
    /// Default denied patterns:
    /// - `.git/` directories
    /// - `.env` files
    /// - `credentials` files
    /// - `.ssh/` directories
    /// - `*.pem` and `*.key` files
    pub fn with_defaults(allowed_roots: Vec<PathBuf>) -> Self {
        Self::new(allowed_roots).with_denied_patterns(vec![
            r"\.git(/|$)".to_string(),
            r"\.env$".to_string(),
            r"\.env\.".to_string(),
            r"credentials".to_string(),
            r"\.ssh(/|$)".to_string(),
            r"\.pem$".to_string(),
            r"\.key$".to_string(),
            r"id_rsa".to_string(),
            r"id_ed25519".to_string(),
        ])
    }

    /// Add denied patterns
    pub fn with_denied_patterns(mut self, patterns: Vec<String>) -> Self {
        self.compiled_patterns = patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();
        self.denied_patterns = patterns;
        self
    }

    /// Validate a path against sandbox rules
    ///
    /// Returns the canonicalized path if valid, or a SandboxViolation if not.
    pub fn validate(&self, path: &Path) -> Result<PathBuf, SandboxViolation> {
        // 1. Canonicalize to resolve symlinks and .. components
        let canonical = path.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SandboxViolation::NotFound
            } else {
                SandboxViolation::IoError(e.to_string())
            }
        })?;

        // 2. Check if within allowed roots
        let in_allowed = self.allowed_roots.iter().any(|root| {
            if let Ok(canonical_root) = root.canonicalize() {
                canonical.starts_with(&canonical_root)
            } else {
                false
            }
        });

        if !in_allowed {
            return Err(SandboxViolation::OutsideAllowedRoots);
        }

        // 3. Check denied patterns
        let path_str = canonical.to_string_lossy();
        for (i, pattern) in self.compiled_patterns.iter().enumerate() {
            if pattern.is_match(&path_str) {
                return Err(SandboxViolation::DeniedPattern {
                    pattern: self
                        .denied_patterns
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string()),
                });
            }
        }

        Ok(canonical)
    }

    /// Check if a path is allowed (without canonicalization for non-existent files)
    ///
    /// Use this for checking paths before creating files.
    pub fn validate_parent(&self, path: &Path) -> Result<(), SandboxViolation> {
        if let Some(parent) = path.parent() {
            if parent.exists() {
                self.validate(parent)?;
            }
        }

        // Check denied patterns on the raw path
        let path_str = path.to_string_lossy();
        for (i, pattern) in self.compiled_patterns.iter().enumerate() {
            if pattern.is_match(&path_str) {
                return Err(SandboxViolation::DeniedPattern {
                    pattern: self
                        .denied_patterns
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string()),
                });
            }
        }

        Ok(())
    }

    /// Get allowed roots
    pub fn allowed_roots(&self) -> &[PathBuf] {
        &self.allowed_roots
    }

    /// Add an allowed root
    pub fn add_root(&mut self, root: PathBuf) {
        self.allowed_roots.push(root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_sandbox_allows_valid_path() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::new(vec![temp.path().to_path_buf()]);

        let file_path = temp.path().join("test.txt");
        std::fs::write(&file_path, "test").unwrap();

        let result = sandbox.validate(&file_path);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sandbox_denies_outside_root() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::new(vec![temp.path().to_path_buf()]);

        let outside_path = PathBuf::from("/etc/passwd");
        let result = sandbox.validate(&outside_path);

        assert!(matches!(result, Err(SandboxViolation::OutsideAllowedRoots)));
    }

    #[test]
    fn test_sandbox_denies_path_traversal() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::new(vec![temp.path().to_path_buf()]);

        let traversal_path = temp.path().join("..").join("..").join("etc").join("passwd");
        let result = sandbox.validate(&traversal_path);

        assert!(result.is_err());
    }

    #[test]
    fn test_sandbox_denies_pattern() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::new(vec![temp.path().to_path_buf()])
            .with_denied_patterns(vec![r"\.env$".to_string()]);

        let env_path = temp.path().join(".env");
        std::fs::write(&env_path, "SECRET=xxx").unwrap();

        let result = sandbox.validate(&env_path);
        assert!(matches!(result, Err(SandboxViolation::DeniedPattern { .. })));
    }

    #[test]
    fn test_sandbox_default_denied_patterns() {
        let temp = TempDir::new().unwrap();
        let sandbox = PathSandbox::with_defaults(vec![temp.path().to_path_buf()]);

        // .git should be denied by default
        let git_path = temp.path().join(".git").join("config");
        std::fs::create_dir_all(git_path.parent().unwrap()).unwrap();
        std::fs::write(&git_path, "test").unwrap();

        let result = sandbox.validate(&git_path);
        assert!(matches!(result, Err(SandboxViolation::DeniedPattern { .. })));
    }
}
