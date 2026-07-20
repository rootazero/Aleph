//! Type definitions for the discovery system

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Source of a discovered component
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DiscoverySource {
    /// Aleph native global (~/.aleph/)
    #[default]
    AlephGlobal,
    /// Claude Code global (~/.claude/)
    ClaudeGlobal,
    /// Project-level (./.claude/ in project directory)
    Project,
    /// Plugin-provided (from a loaded plugin)
    Plugin,
}

impl DiscoverySource {
    /// Whether this source is read-only (Claude Code directories)
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        matches!(self, Self::ClaudeGlobal | Self::Project)
    }
}

/// A directory to scan for components
#[derive(Debug, Clone)]
pub struct ScanDirectory {
    /// Path to the directory
    pub path: PathBuf,
    /// Source type
    pub source: DiscoverySource,
    /// Priority (higher = later in merge order, takes precedence)
    pub priority: u32,
}

impl ScanDirectory {
    /// Create a new scan directory
    #[must_use]
    pub const fn new(path: PathBuf, source: DiscoverySource, priority: u32) -> Self {
        Self {
            path,
            source,
            priority,
        }
    }

    /// Check if the directory exists
    #[must_use]
    pub fn exists(&self) -> bool {
        self.path.exists() && self.path.is_dir()
    }
}

/// A discovered path with metadata
#[derive(Debug, Clone)]
pub struct DiscoveredPath {
    /// Full path to the discovered item
    pub path: PathBuf,
    /// Source of the discovery
    pub source: DiscoverySource,
    /// Name derived from the path (e.g., skill name from directory)
    pub name: String,
    /// Priority for conflict resolution
    pub priority: u32,
}

impl DiscoveredPath {
    /// Create a new discovered path
    #[must_use]
    pub fn new(path: PathBuf, source: DiscoverySource, priority: u32) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or_else(|| path.to_string_lossy().into_owned(), |s| s.to_string());

        Self {
            path,
            source,
            name,
            priority,
        }
    }

    /// Create with explicit name
    #[must_use]
    pub const fn with_name(
        path: PathBuf,
        source: DiscoverySource,
        priority: u32,
        name: String,
    ) -> Self {
        Self {
            path,
            source,
            name,
            priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_source_read_only() {
        assert!(!DiscoverySource::AlephGlobal.is_read_only());
        assert!(DiscoverySource::ClaudeGlobal.is_read_only());
        assert!(DiscoverySource::Project.is_read_only());
        assert!(!DiscoverySource::Plugin.is_read_only());
    }
}
