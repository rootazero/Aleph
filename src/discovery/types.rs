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

/// Which global root a global discovery came from. Every variant maps to
/// `ScopeKey::Global`; the enum exists so that mapping is written per name
/// (no wildcard) — a new root (P4.10's `ClaudeCache`) must be placed by a
/// human, and the compiler refuses to build until it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalRoot {
    /// `~/.aleph` (`DiscoverySource::AlephGlobal`).
    Aleph,
    /// `~/.claude` (`DiscoverySource::ClaudeGlobal`).
    Claude,
}

/// Where a scan started, as the visibility key needs it. A project scope
/// carries its root by value: there is no way to say "project, root unknown".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryScope {
    Global(GlobalRoot),
    Project { root: PathBuf },
}

impl DiscoveryScope {
    /// The legacy `DiscoverySource` label, derived. Every consumer that used
    /// to read a stored `source` field reads this instead.
    #[must_use]
    pub const fn source(&self) -> DiscoverySource {
        match self {
            Self::Global(GlobalRoot::Aleph) => DiscoverySource::AlephGlobal,
            Self::Global(GlobalRoot::Claude) => DiscoverySource::ClaudeGlobal,
            Self::Project { .. } => DiscoverySource::Project,
        }
    }
}

/// A directory to scan for components (scanner-internal).
#[derive(Debug, Clone)]
pub(crate) struct ScanDirectory {
    pub path: PathBuf,
    pub scope: DiscoveryScope,
    pub priority: u32,
}

impl ScanDirectory {
    #[must_use]
    pub(crate) const fn new(path: PathBuf, scope: DiscoveryScope, priority: u32) -> Self {
        Self {
            path,
            scope,
            priority,
        }
    }

    /// A project-level `.claude` / `.aleph` dir found by the upward walk. The
    /// root is the dir's parent — recorded here, at the one place that knows
    /// which walk produced it, never re-derived downstream. `None` only for
    /// a filesystem root, which the upward walk cannot produce; the caller
    /// skips such a dir rather than scanning it as global.
    #[must_use]
    pub(crate) fn project(dir: PathBuf, priority: u32) -> Option<Self> {
        let root = dir.parent()?.to_path_buf();
        Some(Self {
            path: dir,
            scope: DiscoveryScope::Project { root },
            priority,
        })
    }
}

/// A registered project's plugin parent directory, with the project it belongs to.
#[derive(Debug, Clone)]
pub struct ProjectPluginParent {
    pub project_root: PathBuf,
    /// `<project_root>/.aleph/plugins` or `<project_root>/.aleph/plugins.local`.
    pub dir: PathBuf,
}

/// A discovered path with metadata
#[derive(Debug, Clone)]
pub struct DiscoveredPath {
    /// Full path to the discovered item
    pub path: PathBuf,
    /// Priority for conflict resolution
    pub priority: u32,
    /// Where discovery found it. The visibility key
    /// (`extension::visibility::ScopeKey::from_discovery`) is derived from
    /// this and nothing else.
    pub scope: DiscoveryScope,
}

impl DiscoveredPath {
    /// A global discovery (`~/.aleph` or `~/.claude`).
    #[must_use]
    pub fn global(path: PathBuf, root: GlobalRoot, priority: u32) -> Self {
        Self {
            path,
            priority,
            scope: DiscoveryScope::Global(root),
        }
    }

    /// A project discovery: the root is required.
    #[must_use]
    pub fn in_project(path: PathBuf, project_root: PathBuf, priority: u32) -> Self {
        Self {
            path,
            priority,
            scope: DiscoveryScope::Project { root: project_root },
        }
    }

    /// The legacy label; see [`DiscoveryScope::source`].
    #[must_use]
    pub const fn source(&self) -> DiscoverySource {
        self.scope.source()
    }
}
