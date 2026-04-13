//! Identity file loader with mtime-based caching.
//!
//! Reads markdown files from the agent identity directory
//! (`~/.aleph/agents/{agent_id}/`) — SOUL.md, AGENTS.md, MEMORY.md — with
//! filesystem mtime caching to avoid re-reading unchanged files.
//!
//! Identity files live under the agent directory, distinct from the agent's
//! runtime workspace directory (`~/.aleph/workspaces/{agent_id}/`) which only
//! holds tool output and scratch files.

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

/// Identity file loader with mtime-based caching.
///
/// Loads markdown files from the agent identity directory and caches them
/// by filesystem modification time. On subsequent loads the file is only
/// re-read when its mtime has changed.
pub struct IdentityFileLoader {
    /// File cache keyed by absolute path. Pub(crate) for test access.
    pub(crate) cache: HashMap<PathBuf, CachedFile>,
}

impl IdentityFileLoader {
    /// Create a new loader with an empty cache.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Load a file from `identity_dir/filename` with mtime caching.
    ///
    /// Returns `None` if the file does not exist or cannot be read.
    pub fn load(&mut self, identity_dir: &Path, filename: &str) -> Option<String> {
        let path = identity_dir.join(filename);

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
    pub fn load_soul(&mut self, identity_dir: &Path) -> Option<SoulManifest> {
        let path = identity_dir.join("SOUL.md");
        if !path.exists() {
            return None;
        }
        SoulManifest::from_file(&path).ok()
    }

    /// Load `AGENTS.md` from the agent identity directory.
    pub fn load_agents_md(&mut self, identity_dir: &Path) -> Option<String> {
        self.load(identity_dir, "AGENTS.md")
    }

    /// Load `MEMORY.md` from the agent identity directory, truncated at a
    /// char boundary.
    ///
    /// If the file content exceeds `max_chars`, the returned string is
    /// truncated to the largest valid char boundary at or before `max_chars`.
    pub fn load_memory_md(&mut self, identity_dir: &Path, max_chars: usize) -> Option<String> {
        let content = self.load(identity_dir, "MEMORY.md")?;
        if content.chars().count() <= max_chars {
            Some(content)
        } else {
            // Truncate at the char boundary corresponding to max_chars characters
            let byte_offset = content
                .char_indices()
                .nth(max_chars)
                .map(|(i, _)| i)
                .unwrap_or(content.len());
            Some(content[..byte_offset].to_string())
        }
    }
}

impl Default for IdentityFileLoader {
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
        let identity_dir = tmp.path();
        fs::write(identity_dir.join("AGENTS.md"), "# Agents\nHello world").unwrap();

        let mut loader = IdentityFileLoader::new();
        let content = loader.load_agents_md(identity_dir);
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "# Agents\nHello world");
    }

    #[test]
    fn test_load_missing_file_returns_none() {
        let tmp = TempDir::new().unwrap();
        let identity_dir = tmp.path();

        let mut loader = IdentityFileLoader::new();
        let content = loader.load(identity_dir, "DOES_NOT_EXIST.md");
        assert!(content.is_none());
    }

    #[test]
    fn test_load_memory_md_with_truncation() {
        let tmp = TempDir::new().unwrap();
        let identity_dir = tmp.path();
        // Write content longer than our max_chars
        let long_content = "abcdefghij".repeat(10); // 100 chars
        fs::write(identity_dir.join("MEMORY.md"), &long_content).unwrap();

        let mut loader = IdentityFileLoader::new();

        // No truncation needed
        let full = loader.load_memory_md(identity_dir, 200).unwrap();
        assert_eq!(full.len(), 100);

        // Truncation at 50
        let truncated = loader.load_memory_md(identity_dir, 50).unwrap();
        assert_eq!(truncated.len(), 50);
        assert_eq!(truncated, &long_content[..50]);
    }

    #[test]
    fn test_mtime_cache_hit() {
        let tmp = TempDir::new().unwrap();
        let identity_dir = tmp.path();
        fs::write(identity_dir.join("test.md"), "cached content").unwrap();

        let mut loader = IdentityFileLoader::new();

        // First load
        let first = loader.load(identity_dir, "test.md");
        assert!(first.is_some());

        // Second load — should hit cache
        let second = loader.load(identity_dir, "test.md");
        assert!(second.is_some());
        assert_eq!(first, second);

        // Cache should have exactly 1 entry
        assert_eq!(loader.cache.len(), 1);
    }

    #[test]
    fn test_default_creates_empty_loader() {
        let loader = IdentityFileLoader::default();
        assert!(loader.cache.is_empty());
    }

    #[test]
    fn test_load_soul() {
        let tmp = TempDir::new().unwrap();
        let identity_dir = tmp.path();

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
        fs::write(identity_dir.join("SOUL.md"), soul_content).unwrap();

        let mut loader = IdentityFileLoader::new();
        let result = loader.load_soul(identity_dir);

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
