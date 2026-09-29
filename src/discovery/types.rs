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
    /// Shared agents global (~/.agents/) — the cross-tool convention root.
    AgentsGlobal,
    /// Project-level (./.claude/ in project directory)
    Project,
    /// Claude Code's installed-plugin cache — the derived label for
    /// `GlobalRoot::ClaudeCache`; consumed by `PluginOrigin::classify`.
    ClaudeCache,
}

/// Which global root a global discovery came from. Every variant maps to
/// `ScopeKey::Global`; the enum exists so that mapping is written per name
/// (no wildcard) — a new root must be placed by a human, and the compiler
/// refuses to build until it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalRoot {
    /// `~/.aleph` (`DiscoverySource::AlephGlobal`).
    Aleph,
    /// `~/.claude` (`DiscoverySource::ClaudeGlobal`).
    Claude,
    /// `~/.agents` (`DiscoverySource::AgentsGlobal`) — the shared cross-tool
    /// root, read with the same compatibility semantics as the Claude root.
    Agents,
    /// `~/.claude/plugins/cache/<marketplace>/<plugin>/<version>` — Claude
    /// Code's own installed-plugin cache, read-only, discovered from
    /// `installed_plugins.json` (`DiscoverySource::ClaudeCache`).
    ClaudeCache,
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
            Self::Global(GlobalRoot::Agents) => DiscoverySource::AgentsGlobal,
            Self::Global(GlobalRoot::ClaudeCache) => DiscoverySource::ClaudeCache,
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

/// One plugin-discovery pass: the plugin roots it found, and the sources it
/// could not read.
///
/// `unreadable` is the "unknown, not none" half (ruling 5): a source listed
/// here exists but was not understood this pass (Claude Code's
/// `installed_plugins.json` unreadable or of an unknown shape), so the
/// plugins it would name are absent from `found` because they are UNKNOWN —
/// not because they were uninstalled. It is the same read that produced
/// `found`, never a second look at the file.
#[derive(Debug, Default)]
pub struct PluginDiscovery {
    pub found: Vec<DiscoveredPath>,
    pub unreadable: Vec<PathBuf>,
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
    /// A global discovery (`~/.aleph`, `~/.claude`, or Claude Code's
    /// plugin cache).
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
