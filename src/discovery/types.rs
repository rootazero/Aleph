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
}

/// A directory to scan for components (scanner-internal).
#[derive(Debug, Clone)]
pub(crate) struct ScanDirectory {
    pub path: PathBuf,
    pub source: DiscoverySource,
    pub priority: u32,
}

impl ScanDirectory {
    #[must_use]
    pub(crate) const fn new(path: PathBuf, source: DiscoverySource, priority: u32) -> Self {
        Self {
            path,
            source,
            priority,
        }
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
        // An absent `file_name()` (e.g. `/`, `.`, `..`, Windows device roots)
        // must NOT fall back to the full absolute path — the `name` field
        // surfaces in logs, tool catalogs, and the `<available_skills>`
        // prompt index, and a leaked absolute path is a debugging hazard
        // (and looks indistinguishable from a real name in UIs that just
        // render the field). Empty name is honest: downstream code that
        // needs a displayable label can decide what to do (and there is
        // exactly one test assertion to update if a future display layer
        // requires a sentinel like `<unnamed>`).
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or(String::new(), |s| s.to_string());

        Self {
            path,
            source,
            name,
            priority,
        }
    }
}
