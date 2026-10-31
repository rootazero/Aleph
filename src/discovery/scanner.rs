//! Directory scanner for discovering components
//!
//! Implements the multi-directory scanning strategy with upward traversal.

use super::paths::{
    agents_home_dir, aleph_home_dir, claude_home_dir, find_dir_upward, find_git_root,
    AGENTS_HOME_DIR, AGENT_FILE, ALEPH_HOME_DIR, CLAUDE_HOME_DIR, MCP_CONFIG_FILE, PLUGINS_DIR,
    PLUGIN_MANIFEST_DIR, PLUGIN_MANIFEST_FILE, SKILL_FILE,
};
use super::types::{
    DiscoveredPath, DiscoveryScope, GlobalRoot, PluginDiscovery, ProjectPluginParent, ScanDirectory,
};
use super::{DiscoveryConfig, DiscoveryError, DiscoveryResult};
use std::path::{Path, PathBuf};
use tracing::{debug, trace};

/// Directory scanner for discovering components across multiple locations
///
/// Crate-internal: external consumers go through [`DiscoveryManager`].
#[derive(Debug)]
pub(crate) struct DirectoryScanner {
    /// Aleph home directory (~/.aleph/)
    aleph_home: PathBuf,
    /// Claude home directory (~/.claude/)
    claude_home: Option<PathBuf>,
    /// Shared agents home directory (~/.agents/)
    agents_home: Option<PathBuf>,
    /// Git root directory (if found)
    git_root: Option<PathBuf>,
    /// Working directory
    working_dir: PathBuf,
    /// Configuration
    config: DiscoveryConfig,
}

impl DirectoryScanner {
    /// Create a new directory scanner
    pub fn new(config: &DiscoveryConfig) -> DiscoveryResult<Self> {
        let aleph_home = aleph_home_dir()?;

        // Claude home is optional (only scan if it exists). Tests may replace
        // the whole root; production has exactly one.
        #[cfg(test)]
        let resolved = config
            .claude_home_override
            .clone()
            .map_or_else(claude_home_dir, Ok);
        #[cfg(not(test))]
        let resolved = claude_home_dir();
        let claude_home = if config.scan_claude_dirs {
            #[cfg(test)]
            if let Ok(p) = &resolved {
                crate::utils::paths::assert_not_real_claude_home(p, "DirectoryScanner::new");
            }
            match resolved {
                Ok(p) if p.exists() => Some(p),
                Ok(_) => None,
                Err(e) => {
                    // An unresolvable home must not abort discovery, but a
                    // silent skip makes "why weren't my ~/.claude skills
                    // loaded" undebuggable.
                    debug!("claude home unavailable, skipping .claude scan: {e}");
                    None
                }
            }
        } else {
            None
        };

        // Shared agents home (~/.agents) is optional: scan only if it
        // exists. Independent of the Claude knob — it is its own root, not
        // part of the Claude compatibility surface.
        let agents_home = match agents_home_dir() {
            Ok(p) if p.exists() => Some(p),
            Ok(_) => None,
            Err(e) => {
                debug!("agents home unavailable, skipping .agents scan: {e}");
                None
            }
        };

        // Find git root if scanning project dirs
        let git_root = if config.scan_project_dirs {
            find_git_root(&config.working_dir)
        } else {
            None
        };

        debug!(
            aleph_home = ?aleph_home,
            claude_home = ?claude_home,
            agents_home = ?agents_home,
            git_root = ?git_root,
            working_dir = ?config.working_dir,
            "DirectoryScanner initialized"
        );

        Ok(Self {
            aleph_home,
            claude_home,
            agents_home,
            git_root,
            working_dir: config.working_dir.clone(),
            config: config.clone(),
        })
    }

    /// Cached Aleph home directory resolved at construction (honours the
    /// `ALEPH_HOME` override), so callers cannot drift from the scanner's
    /// view by re-resolving env at call time.
    pub(crate) fn aleph_home(&self) -> &Path {
        &self.aleph_home
    }

    /// Get all directories to scan, in scan order.
    ///
    /// The number a directory carries is its SCAN POSITION, not a precedence
    /// rank: the list is sorted ascending, the lowest number is scanned first,
    /// and on an equal-source id clash the registry keeps the first-registered
    /// manifest (`SkillRegistry::register`) — so the lowest number is the one
    /// that wins that tie.
    ///
    /// Scan positions:
    /// 1. Agents global (~/.agents/) - 0
    /// 2. Claude global (~/.claude/) - 1
    /// 3. Aleph global (~/.aleph/) - 10
    /// 4. Project-level .agents/ directories - 20+
    /// 5. Project-level .claude/ directories - 30+
    /// 6. Project-level .aleph/ directories - 40+
    ///
    /// Two consequences a reader needs:
    /// - Within the compat families (`.agents`, `.claude`) a global copy is
    ///   scanned before a project copy and therefore wins an id tie: both are
    ///   `SkillSource::Compat(_)` (source priority 1), so the tie is settled
    ///   by the scan order alone.
    /// - The native `.aleph` family is NOT separated by these numbers: the
    ///   global copy is `SkillSource::Global` (2) and the project copy is
    ///   `SkillSource::Workspace` (4), so the project copy wins on source
    ///   priority whatever the scan order is.
    ///
    /// The `.agents` root sorts BEFORE `.claude` at both tiers on purpose:
    /// the registry keeps the first-registered manifest on a same-priority
    /// id clash (`SkillRegistry::register`), and `.agents` is the canonical
    /// install location of the shared-skills convention (`.claude` typically
    /// holds copies or symlinks of the same skills), so the scan order is
    /// what makes ".agents wins the tie" a rule rather than an accident.
    ///
    /// The `Result` is currently infallible — the only fallible step
    /// validates a constant component name — and is kept as the boundary for
    /// a caller-supplied component name.
    fn get_all_directories(&self) -> DiscoveryResult<Vec<ScanDirectory>> {
        let mut dirs = Vec::new();

        // 1. Agents global (shared cross-tool root; scanned first so its
        //    copy wins a Bundled-vs-Bundled id tie against the Claude one)
        if let Some(ref agents_home) = self.agents_home {
            dirs.push(ScanDirectory::new(
                agents_home.clone(),
                DiscoveryScope::Global(GlobalRoot::Agents),
                0,
            ));
        }

        // 2. Claude global (compat, read-only)
        if let Some(ref claude_home) = self.claude_home {
            dirs.push(ScanDirectory::new(
                claude_home.clone(),
                DiscoveryScope::Global(GlobalRoot::Claude),
                1,
            ));
        }

        // 2. Aleph global
        if self.aleph_home.exists() {
            dirs.push(ScanDirectory::new(
                self.aleph_home.clone(),
                DiscoveryScope::Global(GlobalRoot::Aleph),
                10,
            ));
        }

        // 3-5. Project-level `.agents/`, `.claude/`, `.aleph/` directories
        // (upward traversal). Same shape — find dirs upward, skip the one
        // that is the matching global so it is not double-counted with the
        // global entry added above, push the rest as project scopes with
        // priority `base + i` (deeper = higher number = scanned later, so a
        // shallower match wins an equal-source id tie). Base priorities
        // differ because:
        // - `.agents` sorts BEFORE `.claude` at both tiers on purpose
        //   (see function-level comment: registry keeps first-registered
        //   on same-priority id clash, and `.agents` is the canonical
        //   install location of the shared-skills convention).
        // - native Aleph project dirs outrank the `.claude/` compat dirs.
        if self.config.scan_project_dirs {
            self.collect_project_dirs(&mut dirs, AGENTS_HOME_DIR, 20, self.agents_home.as_deref())?;
            self.collect_project_dirs(&mut dirs, CLAUDE_HOME_DIR, 30, self.claude_home.as_deref())?;
            self.collect_project_dirs(&mut dirs, ALEPH_HOME_DIR, 40, Some(&self.aleph_home))?;
        }

        // Layer-0 dedup: the same physical directory reached through two
        // spellings (a symlinked $HOME, macOS `/var` vs `/private/var`, a
        // root reached both as global and via the upward walk) must scan
        // once. Canonical spelling is the identity; a root that fails to
        // canonicalize (does not exist yet) keeps its literal spelling.
        let mut seen: Vec<PathBuf> = Vec::with_capacity(dirs.len());
        dirs.retain(|d| {
            let key = std::fs::canonicalize(&d.path).unwrap_or_else(|_| d.path.clone());
            if seen.contains(&key) {
                debug!("scan root {:?} deduped against an earlier spelling", d.path);
                false
            } else {
                seen.push(key);
                true
            }
        });

        trace!("Scan directories: {:?}", dirs);
        Ok(dirs)
    }

    /// Walk upward from `working_dir`, push each `dirname` hit as a project
    /// scope at `base_priority + depth` (deeper = higher number = scanned
    /// later, so a shallower match wins an equal-source id tie). When no
    /// git root bounds the walk, the matching global root can be re-discovered
    /// — `global_to_skip` filters it so the global and project scopes do not
    /// both list the same directory (otherwise the project-priority copy
    /// would win the sort and global components would be mislabeled).
    fn collect_project_dirs(
        &self,
        out: &mut Vec<ScanDirectory>,
        dirname: &str,
        base_priority: u32,
        global_to_skip: Option<&Path>,
    ) -> DiscoveryResult<()> {
        let dirs = find_dir_upward(
            dirname,
            &self.working_dir,
            self.git_root.as_deref(),
            self.config.max_upward_depth,
        )?;
        for (i, dir) in dirs.into_iter().rev().enumerate() {
            if global_to_skip.is_some_and(|g| crate::utils::paths::equivalent(&dir, g)) {
                continue;
            }
            // A project root must be a REAL directory. `find_dir_upward`
            // accepts a symlinked `<project>/.aleph` because `is_dir()`
            // follows symlinks, and the link's TARGET would then be scanned
            // as a project root — the target's skills would be classified as
            // workspace skills, the highest-priority tier, ahead of the
            // user's own copies. This is the same boundary the file already
            // documents for the entries INSIDE a root.
            if !is_existing_dir_no_follow(&dir) {
                debug!("skipping symlinked project {dirname} root {:?}", dir);
                continue;
            }
            let priority = base_priority.saturating_add(i as u32);
            match ScanDirectory::project(dir, priority) {
                Some(sd) => out.push(sd),
                None => debug!("project {dirname} dir with no parent skipped"),
            }
        }
        Ok(())
    }

    /// Discover a specific component type (skills, commands, agents, plugins)
    ///
    /// The `Result` is the boundary for a caller-supplied component name: the
    /// name is validated (and length-limited) here, so a bad name is reported
    /// instead of being turned into a path. The discovery pass itself cannot
    /// fail.
    pub fn discover_component(&self, component_name: &str) -> DiscoveryResult<Vec<DiscoveredPath>> {
        const MAX_COMPONENT_NAME_LEN: usize = 256;

        super::paths::validate_path_component(component_name)?;

        if component_name.len() > MAX_COMPONENT_NAME_LEN {
            return Err(DiscoveryError::InvalidPath(format!(
                "component name too long (max {} bytes): got {} bytes",
                MAX_COMPONENT_NAME_LEN,
                component_name.len()
            )));
        }

        let mut discovered = Vec::new();
        let scan_dirs = self.get_all_directories()?;

        for scan_dir in &scan_dirs {
            scan_component_dir(scan_dir, component_name, &mut discovered);
        }

        // Sort by priority (lower first). The single consumer,
        // `projection.rs::republish_plugin_projections`, folds this into the
        // dir list `SkillSystem::init` scans (`skill/mod.rs::rescan_dirs`) —
        // this sort just gives that scan a deterministic order. It is NOT a
        // dedup key for that consumer: a same-id skill collision is resolved
        // by `SkillRegistry::register` via `SkillSource::priority()`
        // (workspace > plugin > global > bundled), independent of scan order.
        //
        // The path is the tie-break, so the order is TOTAL: a stable sort
        // alone leaves equal-priority entries in `read_dir` order, which is
        // filesystem-dependent — two directories under the SAME root that
        // declare the same skill id would get a winner that varies between
        // machines.
        discovered.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.path.cmp(&b.path))
        });

        trace!(
            "Discovered {} {} components",
            discovered.len(),
            component_name
        );
        Ok(discovered)
    }

    /// Discover plugins (special handling for plugin structure)
    ///
    /// Supports multiple manifest formats:
    /// - `aleph.plugin.toml` (V2 preferred)
    /// - `aleph.plugin.json` (V1)
    /// - `.claude-plugin/plugin.json` (legacy)
    ///
    /// Also scans one level deeper for monorepo layouts where each subdirectory
    /// of a cloned repo is an individual plugin.
    ///
    /// With the Claude root on, Claude Code's own installs are added too
    /// (`claude_cache::discover_claude_cache`, read-only): an index resolved
    /// to plugin dirs, not a parent to enumerate. An index that exists but
    /// cannot be read is named in [`PluginDiscovery::unreadable`] — its
    /// plugins are unknown this pass, not absent.
    pub fn discover_plugins_with_extra(
        &self,
        extra_parents: &[ProjectPluginParent],
    ) -> DiscoveryResult<PluginDiscovery> {
        let mut discovered = Vec::new();
        let mut unreadable = Vec::new();
        self.scan_plugin_parent(
            &self.aleph_home.join(PLUGINS_DIR),
            &mut discovered,
            &DiscoveryScope::Global(GlobalRoot::Aleph),
            10,
        );
        for parent in extra_parents {
            self.scan_plugin_parent(
                &parent.dir,
                &mut discovered,
                &DiscoveryScope::Project {
                    root: parent.project_root.clone(),
                },
                20,
            );
        }
        // Claude Code's own installs, read-only. Gated by the same knob as
        // `~/.claude/{skills,commands,agents}` (`scan_claude_dirs` is what
        // leaves `claude_home` unset).
        if let Some(claude_home) = self.claude_home.as_deref() {
            match super::claude_cache::discover_claude_cache(claude_home) {
                Ok(found) => discovered.extend(found),
                Err(index) => unreadable.push(index),
            }
        }
        // Ascending-priority sort (Claude Code cache 5 < global 10 < project
        // 20): `collect_plugin_dirs` — the only consumer — dedups by
        // canonical path with first-wins (`seen.insert(canonical)`), so the
        // LOWER-priority entry survives a same-path collision. The same-ID
        // contest is decided later, by `discover_and_mount` walking this list
        // in reverse. The sort is what makes both a guarantee rather than an
        // accident of scan order. The path is the tie-break, so the order is
        // TOTAL: a stable sort alone leaves equal-priority entries in
        // `read_dir` order, which is filesystem-dependent.
        discovered.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.path.cmp(&b.path))
        });
        trace!(
            "Discovered {} plugins ({} extra parents)",
            discovered.len(),
            extra_parents.len()
        );
        Ok(PluginDiscovery {
            found: discovered,
            unreadable,
        })
    }

    /// Scan a single plugin-parent directory, pushing each plugin root (direct
    /// manifest or one-level monorepo subdir) into `discovered`, every one
    /// stamped with the `scope` this parent was handed. A missing or
    /// unreadable parent is a silent no-op.
    fn scan_plugin_parent(
        &self,
        plugins_dir: &Path,
        discovered: &mut Vec<DiscoveredPath>,
        scope: &DiscoveryScope,
        priority: u32,
    ) {
        // The top-level parent follows symlinks (`is_dir()`), mirroring
        // `scan_component_dir`'s behaviour for `~/.aleph/skills` etc. A
        // symlinked `~/.aleph/plugins` pointing to a real directory on
        // another filesystem is a legitimate layout (e.g. shared plugins
        // volume on macOS) and must NOT be silently skipped. The per-entry
        // symlink screening below still catches a *child* symlink pointing
        // outside the expected tree — that is the security boundary this
        // helper guards, not the parent itself.
        if !plugins_dir.is_dir() {
            return;
        }
        let entries = match std::fs::read_dir(plugins_dir) {
            Ok(e) => e,
            Err(e) => {
                debug!("Failed to read plugins directory {:?}: {}", plugins_dir, e);
                return;
            }
        };
        for entry in entries {
            let path = match entry {
                Ok(e) => e.path(),
                Err(e) => {
                    debug!("Failed to read entry in {:?}: {}", plugins_dir, e);
                    continue;
                }
            };
            // Use `symlink_metadata` so a symlinked "plugin" directory
            // pointing outside the expected tree is rejected, not enumerated
            // as if it lived inside `plugins_dir`.
            if !is_existing_dir_no_follow(&path) || is_hidden(&path) {
                continue;
            }

            for candidate in plugin_candidates(&path) {
                discovered.push(DiscoveredPath {
                    path: candidate,
                    priority,
                    scope: scope.clone(),
                });
            }
        }
    }
}

/// The plugin roots inside one plugin-parent entry.
///
/// The entry itself when it carries a manifest (the direct plugin
/// directory), otherwise the entries one level deeper that do (the monorepo
/// layout: one level deeper). The direct case wins: an entry that has its own
/// manifest is not also scanned for sub-plugins.
fn plugin_candidates(path: &Path) -> Vec<PathBuf> {
    if has_plugin_manifest(path) {
        return vec![path.to_path_buf()];
    }
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) => {
            debug!("Failed to read directory {:?}: {}", path, e);
            return Vec::new();
        }
    };
    entries
        .filter_map(|entry| match entry {
            Ok(e) => Some(e.path()),
            Err(e) => {
                debug!("Failed to read entry in {:?}: {}", path, e);
                None
            }
        })
        .filter(|sub_path| {
            is_existing_dir_no_follow(sub_path)
                && !is_hidden(sub_path)
                && has_plugin_manifest(sub_path)
        })
        .collect()
}

/// Scan a single scan-dir's `<dir>/<component_name>` subdirectory, pushing
/// each valid component into `out`. A missing or unreadable directory is a
/// silent no-op (debug-logged).
fn scan_component_dir(
    scan_dir: &ScanDirectory,
    component_name: &str,
    out: &mut Vec<DiscoveredPath>,
) {
    let component_dir = scan_dir.path.join(component_name);
    // A project scope must be a REAL directory: a project root that is a
    // symlink (`<project>/.aleph -> /elsewhere`) would otherwise have its
    // target scanned as a project scope, and the target's skills would be
    // classified as workspace skills — the highest-priority tier, ahead of
    // the user's own copies. A global scope may legitimately be a symlink (a
    // dotfiles layout), so it keeps the link-following check.
    let is_component_dir = match &scan_dir.scope {
        DiscoveryScope::Project { .. } => is_existing_dir_no_follow(&component_dir),
        DiscoveryScope::Global(_) => component_dir.is_dir(),
    };
    if !is_component_dir {
        return;
    }
    let entries = match std::fs::read_dir(&component_dir) {
        Ok(entries) => entries,
        Err(e) => {
            debug!("Failed to read directory {:?}: {}", component_dir, e);
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                debug!("Failed to read entry in {:?}: {}", component_dir, e);
                continue;
            }
        };
        classify_entry(&entry.path(), scan_dir, component_name, out);
    }
}

/// Classify a single directory entry as a component (marker-checked
/// subdirectory or direct `.md` file) and push it into `out` when it
/// qualifies.
fn classify_entry(
    path: &Path,
    scan_dir: &ScanDirectory,
    component_name: &str,
    out: &mut Vec<DiscoveredPath>,
) {
    // `symlink_metadata` reports the link's own type, so a symlinked
    // skill/command/agent pointing outside the expected tree is rejected
    // instead of being enumerated as a discovered component.
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            trace!("stat {:?} failed, skipping entry: {}", path, e);
            return;
        }
    };
    if is_hidden(path) {
        return;
    }
    // Layer-1 dedup observability: a symlinked component is skipped by
    // design — every known root is scanned at its real location, so the
    // link's target is discovered there exactly once and the link itself
    // would only duplicate it. A link whose target lives OUTSIDE every
    // known root stays invisible (the documented security boundary); say so
    // in the log, or "why didn't my skill load" is undebuggable.
    if meta.file_type().is_symlink() {
        debug!(
            "skipping symlinked {} entry {:?}; the target root is scanned at its real location",
            component_name, path
        );
        return;
    }
    let is_component = if meta.file_type().is_dir() {
        // Skip directories without a marker file for the component type.
        // e.g. ~/.aleph/agents/{id}/ without agent.md is Aleph's identity
        // system, not an extension agent.
        match component_marker_file(component_name) {
            Some(marker) => {
                let present = has_marker_file(path, marker);
                if !present {
                    trace!("Skipping {:?}: missing marker file '{}'", path, marker);
                }
                present
            }
            None => true,
        }
    } else {
        // Also include direct .md files (for commands/agents). A skill is a
        // DIRECTORY carrying a marker file, so a stray `README.md` in a
        // skills directory is not a skill: the consumer would scan a file
        // path and find no manifest in it.
        component_name != "skills"
            && meta.file_type().is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("md")
    };
    if is_component {
        out.push(DiscoveredPath {
            path: path.to_path_buf(),
            priority: scan_dir.priority,
            scope: scan_dir.scope.clone(),
        });
    }
}

/// Whether `dir` carries the marker file its component type requires.
///
/// The comparison mirrors the loader that consumes the directory, per
/// component type:
/// - `SKILL.md`: `skill/mod.rs::is_skill_file` compares the file name with
///   `eq_ignore_ascii_case`, so a skill stored as `skill.md` is acceptable
///   there — it must be visible here too, or the same skill exists on macOS
///   (case-insensitive filesystem) and does not exist on Linux.
/// - `agent.md`: `extension/manifest/parsers.rs` looks for exactly that
///   name, so a directory holding `AGENT.md` is not an agent and must not
///   be discovered as one.
fn has_marker_file(dir: &Path, marker: &str) -> bool {
    if dir.join(marker).exists() {
        return true;
    }
    if marker != SKILL_FILE {
        return false;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            debug!("Failed to read directory {:?}: {}", dir, e);
            return false;
        }
    };
    entries
        .filter_map(|entry| match entry {
            Ok(e) => Some(e),
            Err(e) => {
                debug!("Failed to read entry in {:?}: {}", dir, e);
                None
            }
        })
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(marker))
        })
}

/// `DirEntry::path().is_dir()` follows symlinks; `symlink_metadata` reports
/// the link's own file type. This stops a symlink inside `~/.aleph/{skills,
/// commands, plugins}` pointing outside the expected tree from being
/// enumerated as a discovered component.
pub(crate) fn is_existing_dir_no_follow(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}

/// Check if a path represents a hidden directory (name starts with '.')
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .map(|n| n.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

/// Return the expected marker file for a component type directory.
///
/// If the directory lacks this file, it's not a valid extension component and
/// should be skipped during discovery (e.g. Aleph identity agent dirs).
fn component_marker_file(component_name: &str) -> Option<&'static str> {
    match component_name {
        "agents" => Some(AGENT_FILE),
        "skills" => Some(SKILL_FILE),
        // plugins use has_plugin_manifest() via discover_plugins_with_extra()
        _ => None,
    }
}

/// Check if a directory contains a valid plugin manifest.
///
/// Supports all adapter-recognized formats:
/// - Claude Code: `.claude-plugin/plugin.toml`, `.claude-plugin/plugin.json`
/// - Codex CLI: `.codex-plugin/plugin.json`
/// - Cursor IDE: `.cursor-plugin/plugin.json`, `.cursorrules`, `.cursor/rules/`
/// - Legacy: `aleph.plugin.toml`, `aleph.plugin.json`
/// - Auto-discover: `skills/`, `commands/`, `agents/`, `hooks/`, `.mcp.json`
///
/// One policy, stated in one place: a capability DIRECTORY may not be a
/// symlink, a manifest FILE may.
///
/// A capability dir (`skills/`, `commands/`, `agents/`, `hooks/`) is CONTENT
/// that gets loaded and executed, so a `skills -> /elsewhere` link must not
/// qualify an arbitrary directory as a plugin. A manifest file is DATA that
/// describes the plugin, and the plugin directory it lives in is already
/// screened no-follow one level up — so a symlinked manifest (a dotfiles
/// layout pointing at a shared file) still qualifies its own directory.
/// Screening it as well would silently drop such a plugin, which is a
/// behaviour change this module has no reason to make.
fn has_plugin_manifest(path: &Path) -> bool {
    let cc_dir = path.join(PLUGIN_MANIFEST_DIR);
    [
        cc_dir.join("plugin.toml"),
        cc_dir.join(PLUGIN_MANIFEST_FILE),
        path.join(".codex-plugin/plugin.json"),
        path.join(".cursor-plugin/plugin.json"),
        path.join(".cursorrules"),
        path.join("aleph.plugin.toml"),
        path.join("aleph.plugin.json"),
        path.join(MCP_CONFIG_FILE),
    ]
    .iter()
    .any(|p| p.exists())
        || is_existing_dir_no_follow(&path.join(".cursor/rules"))
        || is_existing_dir_no_follow(&path.join("skills"))
        || is_existing_dir_no_follow(&path.join("commands"))
        || is_existing_dir_no_follow(&path.join("agents"))
        || is_existing_dir_no_follow(&path.join("hooks"))
}

#[cfg(test)]
mod tests;
