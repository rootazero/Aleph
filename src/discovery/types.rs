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
}
