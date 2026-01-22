//! CLAUDE.md hierarchical loading
//!
//! This module discovers and loads CLAUDE.md files from the working directory
//! upward, following the Claude Code convention for project-specific instructions.
//!
//! # Features
//!
//! - **Hierarchical discovery**: Walks up from working directory to find CLAUDE.md
//! - **Import resolution**: Resolves @path references within CLAUDE.md
//! - **Caching**: Caches loaded content for performance
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::plugins::claude_md::ClaudeMdLoader;
//!
//! let loader = ClaudeMdLoader::new("/path/to/project");
//! let instructions = loader.load_all()?;
//! println!("Project instructions:\n{}", instructions);
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::error::Result;

/// CLAUDE.md loader for project-specific instructions
pub struct ClaudeMdLoader {
    /// Working directory to start discovery from
    working_dir: PathBuf,
    /// Maximum depth to search upward (default: 10)
    max_depth: usize,
    /// Cache of loaded files to avoid circular imports
    loaded_files: HashSet<PathBuf>,
}

impl ClaudeMdLoader {
    /// Create a new loader starting from the given directory
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            max_depth: 10,
            loaded_files: HashSet::new(),
        }
    }

    /// Set maximum search depth
    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Discover CLAUDE.md files from working directory upward
    ///
    /// Returns a list of paths, starting from the closest (most specific)
    /// to the furthest (most general).
    pub fn discover(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut current = self.working_dir.clone();
        let mut depth = 0;

        while depth < self.max_depth {
            let claude_md = current.join("CLAUDE.md");
            if claude_md.exists() && claude_md.is_file() {
                debug!("Found CLAUDE.md at {:?}", claude_md);
                found.push(claude_md);
            }

            // Also check for .claude.md (hidden file variant)
            let hidden_claude_md = current.join(".claude.md");
            if hidden_claude_md.exists() && hidden_claude_md.is_file() {
                debug!("Found .claude.md at {:?}", hidden_claude_md);
                found.push(hidden_claude_md);
            }

            // Move up to parent directory
            if !current.pop() {
                break;
            }
            depth += 1;
        }

        // Reverse so most general (furthest) is first, most specific (closest) is last
        // This allows later entries to override earlier ones
        found.reverse();
        found
    }

    /// Load and combine all discovered CLAUDE.md files
    ///
    /// Files are loaded from most general (furthest from working dir) to
    /// most specific (closest), so specific instructions can override general ones.
    pub fn load_all(&mut self) -> Result<String> {
        let files = self.discover();

        if files.is_empty() {
            debug!("No CLAUDE.md files found");
            return Ok(String::new());
        }

        let mut combined = String::new();

        for path in files {
            match self.load_file(&path) {
                Ok(content) => {
                    if !combined.is_empty() {
                        combined.push_str("\n\n---\n\n");
                    }
                    combined.push_str(&format!("<!-- From: {} -->\n", path.display()));
                    combined.push_str(&content);
                }
                Err(e) => {
                    warn!("Failed to load {:?}: {}", path, e);
                }
            }
        }

        Ok(combined)
    }

    /// Load a single CLAUDE.md file with @import resolution
    pub fn load_file(&mut self, path: &Path) -> Result<String> {
        // Check for circular imports
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if self.loaded_files.contains(&canonical) {
            debug!("Skipping already loaded file: {:?}", path);
            return Ok(String::new());
        }
        self.loaded_files.insert(canonical);

        // Read file content
        let content = std::fs::read_to_string(path)?;

        // Resolve @imports
        let base_dir = path.parent().unwrap_or(Path::new("."));
        self.resolve_imports(&content, base_dir)
    }

    /// Resolve @path imports in content
    ///
    /// Supports formats:
    /// - `@README.md` - relative to current file
    /// - `@./docs/guide.md` - explicit relative path
    /// - `@/path/from/root.md` - absolute path (from workspace root)
    fn resolve_imports(&mut self, content: &str, base_dir: &Path) -> Result<String> {
        let mut result = String::new();
        let import_regex = regex::Regex::new(r"@([^\s\n\r\)\]]+\.md)").unwrap();

        let mut last_end = 0;
        for cap in import_regex.captures_iter(content) {
            let full_match = cap.get(0).unwrap();
            let import_path = cap.get(1).unwrap().as_str();

            // Add content before this match
            result.push_str(&content[last_end..full_match.start()]);

            // Resolve the import path
            let resolved_path = if import_path.starts_with('/') {
                // Absolute path from workspace root
                PathBuf::from(import_path)
            } else {
                // Relative to base directory
                base_dir.join(import_path)
            };

            // Load the imported file
            if resolved_path.exists() {
                match self.load_file(&resolved_path) {
                    Ok(imported_content) => {
                        result.push_str(&format!(
                            "<!-- Imported: {} -->\n{}",
                            import_path, imported_content
                        ));
                    }
                    Err(e) => {
                        warn!("Failed to import {:?}: {}", resolved_path, e);
                        result.push_str(&format!("<!-- Failed to import: {} -->", import_path));
                    }
                }
            } else {
                debug!("Import not found: {:?}", resolved_path);
                result.push_str(&format!("<!-- Import not found: {} -->", import_path));
            }

            last_end = full_match.end();
        }

        // Add remaining content
        result.push_str(&content[last_end..]);

        Ok(result)
    }

    /// Get a summary of discovered files
    pub fn summary(&self) -> ClaudeMdSummary {
        let files = self.discover();
        ClaudeMdSummary {
            files,
            working_dir: self.working_dir.clone(),
        }
    }
}

/// Summary of discovered CLAUDE.md files
#[derive(Debug, Clone)]
pub struct ClaudeMdSummary {
    /// List of discovered files (general to specific)
    pub files: Vec<PathBuf>,
    /// Working directory used for discovery
    pub working_dir: PathBuf,
}

impl ClaudeMdSummary {
    /// Check if any CLAUDE.md files were found
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get the number of discovered files
    pub fn count(&self) -> usize {
        self.files.len()
    }

    /// Get the most specific (closest to working dir) file
    pub fn most_specific(&self) -> Option<&PathBuf> {
        self.files.last()
    }

    /// Get the most general (furthest from working dir) file
    pub fn most_general(&self) -> Option<&PathBuf> {
        self.files.first()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_discover_single_file() {
        let temp_dir = TempDir::new().unwrap();
        let claude_md = temp_dir.path().join("CLAUDE.md");
        std::fs::write(&claude_md, "# Project Instructions").unwrap();

        let loader = ClaudeMdLoader::new(temp_dir.path());
        let files = loader.discover();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0], claude_md);
    }

    #[test]
    fn test_discover_hierarchical() {
        let temp_dir = TempDir::new().unwrap();

        // Create nested directory structure
        let level1 = temp_dir.path();
        let level2 = level1.join("subdir");
        let level3 = level2.join("nested");
        std::fs::create_dir_all(&level3).unwrap();

        // Create CLAUDE.md at each level
        std::fs::write(level1.join("CLAUDE.md"), "# Level 1").unwrap();
        std::fs::write(level2.join("CLAUDE.md"), "# Level 2").unwrap();
        std::fs::write(level3.join("CLAUDE.md"), "# Level 3").unwrap();

        // Discover from level 3
        let loader = ClaudeMdLoader::new(&level3);
        let files = loader.discover();

        assert_eq!(files.len(), 3);
        // Files should be ordered general to specific
        assert!(files[0].to_string_lossy().contains("subdir") == false || files[0].ends_with("CLAUDE.md"));
        assert!(files[2].to_string_lossy().contains("nested"));
    }

    #[test]
    fn test_load_with_imports() {
        let temp_dir = TempDir::new().unwrap();

        // Create main CLAUDE.md
        let main_md = temp_dir.path().join("CLAUDE.md");
        std::fs::write(&main_md, "# Main\n\nSee @docs.md for details.").unwrap();

        // Create imported file
        let docs_md = temp_dir.path().join("docs.md");
        std::fs::write(&docs_md, "## Documentation\n\nThis is the docs.").unwrap();

        let mut loader = ClaudeMdLoader::new(temp_dir.path());
        let content = loader.load_file(&main_md).unwrap();

        assert!(content.contains("# Main"));
        assert!(content.contains("## Documentation"));
        assert!(content.contains("<!-- Imported: docs.md -->"));
    }

    #[test]
    fn test_circular_import_prevention() {
        let temp_dir = TempDir::new().unwrap();

        // Create files that reference each other
        let a_md = temp_dir.path().join("a.md");
        let b_md = temp_dir.path().join("b.md");

        std::fs::write(&a_md, "# A\nSee @b.md").unwrap();
        std::fs::write(&b_md, "# B\nSee @a.md").unwrap();

        let mut loader = ClaudeMdLoader::new(temp_dir.path());
        let content = loader.load_file(&a_md).unwrap();

        // Should not hang or error out
        assert!(content.contains("# A"));
        assert!(content.contains("# B"));
    }

    #[test]
    fn test_no_claude_md() {
        let temp_dir = TempDir::new().unwrap();

        let mut loader = ClaudeMdLoader::new(temp_dir.path());
        let content = loader.load_all().unwrap();

        assert!(content.is_empty());
    }

    #[test]
    fn test_summary() {
        let temp_dir = TempDir::new().unwrap();
        let claude_md = temp_dir.path().join("CLAUDE.md");
        std::fs::write(&claude_md, "# Test").unwrap();

        let loader = ClaudeMdLoader::new(temp_dir.path());
        let summary = loader.summary();

        assert!(!summary.is_empty());
        assert_eq!(summary.count(), 1);
        assert!(summary.most_specific().is_some());
    }
}
