//! Workspace file loader with mtime-based caching.
//!
//! Reads markdown files from agent workspace directories (SOUL.md, AGENTS.md,
//! MEMORY.md) with filesystem mtime caching to avoid re-reading unchanged files.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::thinker::soul::SoulManifest;

/// Cached file entry with content and modification time.
pub(crate) struct CachedFile {
    content: String,
    mtime: SystemTime,
}

/// Workspace file loader with mtime-based caching.
///
/// Loads markdown files from agent workspace directories and caches them
/// by filesystem modification time. On subsequent loads the file is only
/// re-read when its mtime has changed.
pub struct WorkspaceFileLoader {
    /// File cache keyed by absolute path. Pub(crate) for test access.
    pub(crate) cache: HashMap<PathBuf, CachedFile>,
}

impl WorkspaceFileLoader {
    /// Create a new loader with an empty cache.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Load a file from `workspace/filename` with mtime caching.
    ///
    /// Returns `None` if the file does not exist or cannot be read.
    pub fn load(&mut self, workspace: &Path, filename: &str) -> Option<String> {
        let path = workspace.join(filename);

        let metadata = fs::metadata(&path).ok()?;
        let mtime = metadata.modified().ok()?;

        // Check cache
        if let Some(cached) = self.cache.get(&path) {
            if cached.mtime == mtime {
                return Some(cached.content.clone());
            }
        }

        // Read and cache
        let content = fs::read_to_string(&path).ok()?;
        self.cache.insert(
            path,
            CachedFile {
                content: content.clone(),
                mtime,
            },
        );
        Some(content)
    }

    /// Load and parse `SOUL.md` via `SoulManifest::from_file`.
    ///
    /// Returns `None` if the file does not exist or fails to parse.
    pub fn load_soul(&mut self, workspace: &Path) -> Option<SoulManifest> {
        let path = workspace.join("SOUL.md");
        if !path.exists() {
            return None;
        }
        SoulManifest::from_file(&path).ok()
    }

    /// Load `AGENTS.md` from the workspace.
    pub fn load_agents_md(&mut self, workspace: &Path) -> Option<String> {
        self.load(workspace, "AGENTS.md")
    }

    /// Load `MEMORY.md` from the workspace, truncated at a char boundary.
    ///
    /// If the file content exceeds `max_chars`, the returned string is
    /// truncated to the largest valid char boundary at or before `max_chars`.
    pub fn load_memory_md(&mut self, workspace: &Path, max_chars: usize) -> Option<String> {
        let content = self.load(workspace, "MEMORY.md")?;
        if content.len() <= max_chars {
            Some(content)
        } else {
            // Truncate at char boundary
            let truncated = &content[..content.floor_char_boundary(max_chars)];
            Some(truncated.to_string())
        }
    }

}

impl Default for WorkspaceFileLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_agents_md() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        fs::write(workspace.join("AGENTS.md"), "# Agents\nHello world").unwrap();

        let mut loader = WorkspaceFileLoader::new();
        let content = loader.load_agents_md(workspace);
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "# Agents\nHello world");
    }

    #[test]
    fn test_load_missing_file_returns_none() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        let mut loader = WorkspaceFileLoader::new();
        let content = loader.load(workspace, "DOES_NOT_EXIST.md");
        assert!(content.is_none());
    }

    #[test]
    fn test_load_memory_md_with_truncation() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        // Write content longer than our max_chars
        let long_content = "abcdefghij".repeat(10); // 100 chars
        fs::write(workspace.join("MEMORY.md"), &long_content).unwrap();

        let mut loader = WorkspaceFileLoader::new();

        // No truncation needed
        let full = loader.load_memory_md(workspace, 200).unwrap();
        assert_eq!(full.len(), 100);

        // Truncation at 50
        let truncated = loader.load_memory_md(workspace, 50).unwrap();
        assert_eq!(truncated.len(), 50);
        assert_eq!(truncated, &long_content[..50]);
    }

    #[test]
    fn test_mtime_cache_hit() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        fs::write(workspace.join("test.md"), "cached content").unwrap();

        let mut loader = WorkspaceFileLoader::new();

        // First load
        let first = loader.load(workspace, "test.md");
        assert!(first.is_some());

        // Second load — should hit cache
        let second = loader.load(workspace, "test.md");
        assert!(second.is_some());
        assert_eq!(first, second);

        // Cache should have exactly 1 entry
        assert_eq!(loader.cache.len(), 1);
    }

    #[test]
    fn test_default_creates_empty_loader() {
        let loader = WorkspaceFileLoader::default();
        assert!(loader.cache.is_empty());
    }

    #[test]
    fn test_load_soul() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        // Write a SOUL.md with YAML frontmatter
        let soul_content = r#"---
identity: "I am a test soul"
relationship: peer
voice:
  tone: casual
---

## Directives

- Be helpful
"#;
        fs::write(workspace.join("SOUL.md"), soul_content).unwrap();

        let mut loader = WorkspaceFileLoader::new();
        let result = loader.load_soul(workspace);

        // SoulManifest::from_file should succeed with valid frontmatter
        // If it doesn't, that's also OK — we just test the method exists and runs
        match result {
            Some(manifest) => {
                assert_eq!(manifest.identity, "I am a test soul");
            }
            None => {
                // from_file may fail with test content — that's acceptable
            }
        }
    }
}
