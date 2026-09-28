//! Directory scanner for discovering components
//!
//! Implements the multi-directory scanning strategy with upward traversal.

use super::paths::{
    aleph_home_dir, claude_home_dir, find_dir_upward, find_git_root, AGENT_FILE, ALEPH_HOME_DIR,
    CLAUDE_HOME_DIR, MCP_CONFIG_FILE, PLUGINS_DIR, PLUGIN_MANIFEST_DIR, PLUGIN_MANIFEST_FILE,
    SKILL_FILE,
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

        // Find git root if scanning project dirs
        let git_root = if config.scan_project_dirs {
            find_git_root(&config.working_dir)
        } else {
            None
        };

        debug!(
            aleph_home = ?aleph_home,
            claude_home = ?claude_home,
            git_root = ?git_root,
            working_dir = ?config.working_dir,
            "DirectoryScanner initialized"
        );

        Ok(Self {
            aleph_home,
            claude_home,
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

    /// Get all directories to scan, in priority order
    ///
    /// Priority order (lowest to highest):
    /// 1. Claude global (~/.claude/) - priority 0
    /// 2. Aleph global (~/.aleph/) - priority 10
    /// 3. Project-level .claude/ directories - priority 20+
    /// 4. Project-level .aleph/ directories - priority 40+ (native, wins over
    ///    the project `.claude/` compat dirs on a name clash)
    fn get_all_directories(&self) -> DiscoveryResult<Vec<ScanDirectory>> {
        let mut dirs = Vec::new();

        // 1. Claude global (lowest priority, read-only)
        if let Some(ref claude_home) = self.claude_home {
            dirs.push(ScanDirectory::new(
                claude_home.clone(),
                DiscoveryScope::Global(GlobalRoot::Claude),
                0,
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

        // 3. Project-level .claude/ directories (upward traversal)
        if self.config.scan_project_dirs {
            let stop = self.git_root.as_deref();
            let claude_dirs = find_dir_upward(
                CLAUDE_HOME_DIR,
                &self.working_dir,
                stop,
                self.config.max_upward_depth,
            )?;

            // Reverse to get proper priority (deeper = higher priority)
            for (i, dir) in claude_dirs.into_iter().rev().enumerate() {
                // Same guard as the `.aleph` block below: when no git root
                // bounds the upward walk it can re-discover the global
                // `~/.claude`; skip it so it is not double-counted with the
                // ClaudeGlobal entry above (otherwise the Project-priority
                // copy wins the sort and global components are mislabeled).
                if self
                    .claude_home
                    .as_deref()
                    .is_some_and(|ch| crate::utils::paths::equivalent(&dir, ch))
                {
                    continue;
                }
                let priority = 20u32.saturating_add(i as u32);
                match ScanDirectory::project(dir, priority) {
                    Some(sd) => dirs.push(sd),
                    None => debug!("project .claude dir with no parent skipped"),
                }
            }

            // 4. Project-level .aleph/ directories (upward traversal). Native
            //    Aleph project skills/commands/agents — the parity counterpart of
            //    the `.claude/` block above, previously missing so a project's
            //    `.aleph/skills` never reached the `<available_skills>` index.
            //    Higher priority than the project `.claude/` compat dirs.
            let aleph_dirs = find_dir_upward(
                ALEPH_HOME_DIR,
                &self.working_dir,
                stop,
                self.config.max_upward_depth,
            )?;
            for (i, dir) in aleph_dirs.into_iter().rev().enumerate() {
                // The upward walk can re-discover the global `~/.aleph` when no
                // git root bounds it; skip it so it is not double-counted with
                // the AlephGlobal entry added above.
                if crate::utils::paths::equivalent(&dir, &self.aleph_home) {
                    continue;
                }
                let priority = 40u32.saturating_add(i as u32);
                match ScanDirectory::project(dir, priority) {
                    Some(sd) => dirs.push(sd),
                    None => debug!("project .aleph dir with no parent skipped"),
                }
            }
        }

        trace!("Scan directories: {:?}", dirs);
        Ok(dirs)
    }

    /// Discover a specific component type (skills, commands, agents, plugins)
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
        discovered.sort_by_key(|d| d.priority);

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
        // accident of scan order.
        discovered.sort_by_key(|d| d.priority);
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

            if has_plugin_manifest(&path) {
                // Direct plugin directory
                discovered.push(DiscoveredPath {
                    path,
                    priority,
                    scope: scope.clone(),
                });
            } else if let Ok(sub_entries) = std::fs::read_dir(&path) {
                // Check subdirectories (monorepo layout)
                for sub_entry in sub_entries {
                    let sub_path = match sub_entry {
                        Ok(e) => e.path(),
                        Err(e) => {
                            debug!("Failed to read entry in {:?}: {}", path, e);
                            continue;
                        }
                    };
                    if !is_existing_dir_no_follow(&sub_path) || is_hidden(&sub_path) {
                        continue;
                    }
                    if has_plugin_manifest(&sub_path) {
                        discovered.push(DiscoveredPath {
                            path: sub_path,
                            priority,
                            scope: scope.clone(),
                        });
                    }
                }
            }
        }
    }
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
    if !component_dir.is_dir() {
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
    let is_component = if meta.file_type().is_dir() {
        // Skip directories without a marker file for the component type.
        // e.g. ~/.aleph/agents/{id}/ without agent.md is Aleph's identity
        // system, not an extension agent.
        match component_marker_file(component_name) {
            Some(marker) => {
                let present = path.join(marker).exists();
                if !present {
                    trace!("Skipping {:?}: missing marker file '{}'", path, marker);
                }
                present
            }
            None => true,
        }
    } else {
        // Also include direct .md files (for commands/agents)
        meta.file_type().is_file() && path.extension().and_then(|e| e.to_str()) == Some("md")
    };
    if is_component {
        out.push(DiscoveredPath {
            path: path.to_path_buf(),
            priority: scan_dir.priority,
            scope: scan_dir.scope.clone(),
        });
    }
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
        // Auto-discover dirs use the no-follow check too: the plugin parent
        // itself is symlink-screened, and a `skills -> /elsewhere` symlink
        // must not qualify an arbitrary directory as a plugin either.
        || is_existing_dir_no_follow(&path.join(".cursor/rules"))
        || is_existing_dir_no_follow(&path.join("skills"))
        || is_existing_dir_no_follow(&path.join("commands"))
        || is_existing_dir_no_follow(&path.join("agents"))
        || is_existing_dir_no_follow(&path.join("hooks"))
}

#[cfg(test)]
mod tests {
    use super::super::types::DiscoverySource;
    use super::*;
    use tempfile::TempDir;

    fn create_test_structure(temp: &TempDir) -> PathBuf {
        let root = temp.path();

        // Create git root marker
        std::fs::create_dir(root.join(".git")).unwrap();

        // Create Aleph-like structure
        let aleph_dir = root.join(".aleph");
        std::fs::create_dir_all(aleph_dir.join("skills/my-skill")).unwrap();
        // Add required SKILL.md marker so discover_component recognizes this dir
        std::fs::write(
            aleph_dir.join("skills/my-skill/SKILL.md"),
            "---\nname: my-skill\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(aleph_dir.join("commands/my-cmd")).unwrap();
        std::fs::create_dir_all(aleph_dir.join("plugins")).unwrap();

        // Create project-level .claude
        std::fs::create_dir_all(root.join("project/.claude/skills/project-skill")).unwrap();
        // Add required SKILL.md marker for project-level skill
        std::fs::write(
            root.join("project/.claude/skills/project-skill/SKILL.md"),
            "---\nname: project-skill\n---\n",
        )
        .unwrap();

        root.to_path_buf()
    }

    #[test]
    fn aleph_home_is_authoritative_for_component_discovery() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let aleph_home = temp.path().join("aleph-home");
        let claude_home = home.join(".claude");
        std::fs::create_dir_all(aleph_home.join("skills")).unwrap();
        std::fs::create_dir_all(claude_home.join("skills")).unwrap();

        let scanner = {
            let _env =
                crate::runtimes::post_install::HomeEnvGuards::acquire_and_set(&aleph_home, &home);
            DirectoryScanner::new(&DiscoveryConfig {
                working_dir: temp.path().to_path_buf(),
                scan_claude_dirs: true,
                scan_project_dirs: false,
                max_upward_depth: 10,
                claude_home_override: None,
            })
            .unwrap()
        };

        assert_eq!(scanner.aleph_home, aleph_home);
        assert_eq!(scanner.claude_home, Some(claude_home));
    }

    #[test]
    fn test_scanner_get_directories() {
        let temp = TempDir::new().unwrap();
        let root = create_test_structure(&temp);

        let config = DiscoveryConfig {
            working_dir: root.join("project"),
            scan_claude_dirs: true,
            scan_project_dirs: true,
            max_upward_depth: 10,
            claude_home_override: None,
        };

        // Override aleph home for testing
        let scanner = DirectoryScanner {
            aleph_home: root.join(".aleph"),
            claude_home: None,
            git_root: Some(root.clone()),
            working_dir: root.join("project"),
            config,
        };

        let dirs = scanner.get_all_directories().unwrap();

        // Should have Aleph global + project .claude
        assert!(!dirs.is_empty());
        assert!(dirs
            .iter()
            .any(|d| d.scope.source() == DiscoverySource::AlephGlobal));
    }

    #[test]
    fn test_scanner_discover_skills() {
        let temp = TempDir::new().unwrap();
        let root = create_test_structure(&temp);

        let config = DiscoveryConfig {
            working_dir: root.join("project"),
            scan_claude_dirs: true,
            scan_project_dirs: true,
            max_upward_depth: 10,
            claude_home_override: None,
        };

        let scanner = DirectoryScanner {
            aleph_home: root.join(".aleph"),
            claude_home: None,
            git_root: Some(root.clone()),
            working_dir: root.join("project"),
            config,
        };

        let skills = scanner.discover_component("skills").unwrap();

        // Should find my-skill and project-skill
        assert!(!skills.is_empty());
        assert!(skills.iter().any(|s| s.path.ends_with("my-skill")));
    }

    #[test]
    fn test_scanner_discovers_project_aleph_skills() {
        // Regression: a project's `.aleph/skills` must be discovered (it was
        // previously missing — only project `.claude/` was walked — so native
        // project skills never reached the SkillSystem `<available_skills>`).
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join(".git")).unwrap();

        let skill = root.join("project/.aleph/skills/proj-aleph-skill");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: proj-aleph-skill\n---\n").unwrap();

        // Aleph global points at a separate empty dir so the only hit is the
        // project-level `.aleph/skills`.
        let empty_home = TempDir::new().unwrap();
        let scanner = DirectoryScanner {
            aleph_home: empty_home.path().to_path_buf(),
            claude_home: None,
            git_root: Some(root.to_path_buf()),
            working_dir: root.join("project"),
            config: DiscoveryConfig {
                working_dir: root.join("project"),
                scan_claude_dirs: false,
                scan_project_dirs: true,
                max_upward_depth: 10,
                claude_home_override: None,
            },
        };

        let skills = scanner.discover_component("skills").unwrap();
        assert!(
            skills.iter().any(|s| s.path.ends_with("proj-aleph-skill")),
            "project .aleph/skills must be discovered, got {:?}",
            skills
                .iter()
                .map(|s| s.path.file_name().unwrap_or(s.path.as_os_str()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_project_aleph_outranks_project_claude_on_clash() {
        // Same skill id in both project `.aleph/skills` and `.claude/skills`:
        // the native `.aleph` entry must carry the higher priority so it wins
        // conflict resolution.
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join(".git")).unwrap();

        for flavor in [".aleph", ".claude"] {
            let d = root.join(format!("project/{flavor}/skills/dup-skill"));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("SKILL.md"), "---\nname: dup-skill\n---\n").unwrap();
        }

        let empty_home = TempDir::new().unwrap();
        let scanner = DirectoryScanner {
            aleph_home: empty_home.path().to_path_buf(),
            claude_home: None,
            git_root: Some(root.to_path_buf()),
            working_dir: root.join("project"),
            config: DiscoveryConfig {
                working_dir: root.join("project"),
                scan_claude_dirs: true,
                scan_project_dirs: true,
                max_upward_depth: 10,
                claude_home_override: None,
            },
        };

        let skills = scanner.discover_component("skills").unwrap();
        let aleph_entry = skills
            .iter()
            .find(|s| s.path.to_string_lossy().contains(".aleph"))
            .expect("aleph entry present");
        let claude_entry = skills
            .iter()
            .find(|s| s.path.to_string_lossy().contains(".claude"))
            .expect("claude entry present");
        assert!(
            aleph_entry.priority > claude_entry.priority,
            "native .aleph (prio {}) must outrank .claude (prio {})",
            aleph_entry.priority,
            claude_entry.priority
        );
    }

    #[test]
    fn test_discover_component_skips_hidden_md_files() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join(".aleph/commands")).unwrap();
        std::fs::write(root.join(".aleph/commands/visible.md"), "cmd").unwrap();
        std::fs::write(root.join(".aleph/commands/.hidden.md"), "cmd").unwrap();

        let scanner = DirectoryScanner {
            aleph_home: root.join(".aleph"),
            claude_home: None,
            git_root: None,
            working_dir: root.to_path_buf(),
            config: DiscoveryConfig {
                working_dir: root.to_path_buf(),
                scan_claude_dirs: false,
                scan_project_dirs: false,
                max_upward_depth: 10,
                claude_home_override: None,
            },
        };

        let cmds = scanner.discover_component("commands").unwrap();
        let names: Vec<String> = cmds
            .iter()
            .map(|c| c.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "visible.md"), "got {names:?}");
        assert!(
            !names.iter().any(|n| n == ".hidden.md"),
            "hidden file leaked: {names:?}"
        );
    }

    #[test]
    fn test_discover_plugins_toml_manifest() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create plugin with aleph.plugin.toml
        let plugins_dir = root.join(".aleph/plugins/my-plugin");
        std::fs::create_dir_all(&plugins_dir).unwrap();
        std::fs::write(
            plugins_dir.join("aleph.plugin.toml"),
            "[plugin]\nid = \"my-plugin\"",
        )
        .unwrap();

        let scanner = DirectoryScanner {
            aleph_home: root.join(".aleph"),
            claude_home: None,
            git_root: None,
            working_dir: root.to_path_buf(),
            config: DiscoveryConfig::default(),
        };

        let plugins = scanner.discover_plugins_with_extra(&[]).unwrap().found;
        assert_eq!(plugins.len(), 1);
        assert!(plugins[0].path.ends_with("my-plugin"));
    }

    #[test]
    fn test_discover_plugins_monorepo() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Create monorepo structure: plugins/Aleph-plugins/{diagnostics,llm-task}
        let mono_dir = root.join(".aleph/plugins/Aleph-plugins");
        let diag = mono_dir.join("diagnostics");
        let llm = mono_dir.join("llm-task");
        std::fs::create_dir_all(&diag).unwrap();
        std::fs::create_dir_all(&llm).unwrap();
        std::fs::write(
            diag.join("aleph.plugin.toml"),
            "[plugin]\nid = \"diagnostics\"",
        )
        .unwrap();
        std::fs::write(llm.join("aleph.plugin.toml"), "[plugin]\nid = \"llm-task\"").unwrap();
        // Also create a non-plugin dir (README, etc) — should be skipped
        std::fs::write(mono_dir.join("README.md"), "readme").unwrap();

        let scanner = DirectoryScanner {
            aleph_home: root.join(".aleph"),
            claude_home: None,
            git_root: None,
            working_dir: root.to_path_buf(),
            config: DiscoveryConfig::default(),
        };

        let plugins = scanner.discover_plugins_with_extra(&[]).unwrap().found;
        assert_eq!(plugins.len(), 2);
        let names: Vec<String> = plugins
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "diagnostics"));
        assert!(names.iter().any(|n| n == "llm-task"));
    }

    #[test]
    fn test_discover_plugins_with_extra_project_dirs() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // Global plugin under ~/.aleph/plugins.
        let global = root.join(".aleph/plugins/global-plugin");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("aleph.plugin.toml"),
            "[plugin]\nid = \"global-plugin\"",
        )
        .unwrap();

        // Project-local plugin under <project>/.aleph/plugins.
        let project = root.join("workspace/proj-a");
        let proj_plugin = project.join(".aleph/plugins/proj-plugin");
        std::fs::create_dir_all(&proj_plugin).unwrap();
        std::fs::write(
            proj_plugin.join("aleph.plugin.toml"),
            "[plugin]\nid = \"proj-plugin\"",
        )
        .unwrap();

        let scanner = DirectoryScanner {
            aleph_home: root.join(".aleph"),
            claude_home: None,
            git_root: None,
            working_dir: root.to_path_buf(),
            config: DiscoveryConfig::default(),
        };

        // Plain discover_plugins sees only the global one.
        assert_eq!(
            scanner
                .discover_plugins_with_extra(&[])
                .unwrap()
                .found
                .len(),
            1
        );

        // With the project's plugin parent, both are discovered.
        let plugins = scanner
            .discover_plugins_with_extra(&[ProjectPluginParent {
                project_root: project.clone(),
                dir: project.join(".aleph/plugins"),
            }])
            .unwrap()
            .found;
        let names: Vec<String> = plugins
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "global-plugin"), "got {names:?}");
        assert!(names.iter().any(|n| n == "proj-plugin"), "got {names:?}");

        // A non-existent extra parent is a silent no-op (still just global +
        // the present project one).
        let plugins2 = scanner
            .discover_plugins_with_extra(&[
                ProjectPluginParent {
                    project_root: project.clone(),
                    dir: project.join(".aleph/plugins"),
                },
                ProjectPluginParent {
                    project_root: root.join("workspace/proj-missing"),
                    dir: root.join("workspace/proj-missing/.aleph/plugins"),
                },
            ])
            .unwrap()
            .found;
        assert_eq!(plugins2.len(), 2);
    }

    /// Project plugin parents are scanned with the root they were handed, and
    /// the root survives onto every discovered plugin — a plugin found under
    /// `<root>/.aleph/plugins.local/` reports `<root>`, not its own directory
    /// and not the parent's.
    #[test]
    fn project_plugin_parents_carry_their_root_onto_discovered_plugins() {
        let home = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let local = root.path().join(".aleph/plugins.local/proj-local");
        std::fs::create_dir_all(local.join(".claude-plugin")).unwrap();
        std::fs::write(
            local.join(".claude-plugin/plugin.toml"),
            "name = \"proj-local\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        // Same construction the sibling plugin tests use: the home is a
        // struct field, so no process-global `ALEPH_HOME` is touched.
        let scanner = DirectoryScanner {
            aleph_home: home.path().join(".aleph"),
            claude_home: None,
            git_root: None,
            working_dir: home.path().to_path_buf(),
            config: DiscoveryConfig {
                working_dir: home.path().to_path_buf(),
                scan_claude_dirs: false,
                scan_project_dirs: false,
                max_upward_depth: 0,
                claude_home_override: None,
            },
        };
        let found = scanner
            .discover_plugins_with_extra(&[ProjectPluginParent {
                project_root: root.path().to_path_buf(),
                dir: root.path().join(".aleph/plugins.local"),
            }])
            .unwrap()
            .found;
        let hit = found
            .iter()
            .find(|d| d.path.ends_with("proj-local"))
            .expect("plugin discovered");
        assert_eq!(
            hit.scope,
            DiscoveryScope::Project {
                root: root.path().to_path_buf()
            }
        );
        assert_eq!(hit.source(), DiscoverySource::Project);
    }

    /// A Claude home with one Claude Code install (`qa@m` 1.0.0).
    fn claude_home_with_one_install(home: &Path) -> PathBuf {
        let plugins = home.join("plugins");
        let dir = plugins.join("cache/m/qa/1.0.0");
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::write(dir.join(".claude-plugin/plugin.json"), r#"{"name":"qa"}"#).unwrap();
        std::fs::write(
            plugins.join("installed_plugins.json"),
            r#"{"version":2,"plugins":{"qa@m":[{"scope":"user","version":"1.0.0"}]}}"#,
        )
        .unwrap();
        dir
    }

    /// The production path: no override, the scanner resolves `$HOME/.claude`
    /// at construction and plugin discovery reads Claude Code's cache from
    /// it — below `~/.aleph` in the ascending sort, so on an id contest the
    /// Aleph copy is walked first by `discover_and_mount`.
    #[test]
    fn plugin_discovery_reads_the_claude_cache_of_the_resolved_claude_home() {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let aleph_home = temp.path().join("aleph-home");
        let cached = claude_home_with_one_install(&home.join(".claude"));
        let own = aleph_home.join("plugins/own");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("aleph.plugin.toml"), "[plugin]\nid = \"own\"").unwrap();

        let scanner = {
            let _env =
                crate::runtimes::post_install::HomeEnvGuards::acquire_and_set(&aleph_home, &home);
            DirectoryScanner::new(&DiscoveryConfig {
                working_dir: temp.path().to_path_buf(),
                scan_claude_dirs: true,
                scan_project_dirs: false,
                max_upward_depth: 0,
                claude_home_override: None,
            })
            .unwrap()
        };

        let found = scanner.discover_plugins_with_extra(&[]).unwrap().found;
        let paths: Vec<&Path> = found.iter().map(|d| d.path.as_path()).collect();
        assert_eq!(paths, vec![cached.as_path(), own.as_path()], "{found:?}");
        assert_eq!(found[0].source(), DiscoverySource::ClaudeCache);
        assert_eq!(found[1].source(), DiscoverySource::AlephGlobal);
    }

    /// The override replaces `~/.claude` for the whole Claude root, and the
    /// one knob that turns the Claude root off turns the cache off with it.
    #[test]
    fn the_claude_cache_follows_the_claude_root_override_and_its_switch() {
        // `DirectoryScanner::new` resolves `~/.aleph` from the environment.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let temp = TempDir::new().unwrap();
        let claude = temp.path().join("claude");
        let cached = claude_home_with_one_install(&claude);
        let config = |scan_claude_dirs| DiscoveryConfig {
            working_dir: temp.path().to_path_buf(),
            scan_claude_dirs,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: Some(claude.clone()),
        };

        let on = DirectoryScanner::new(&config(true)).unwrap();
        assert_eq!(on.claude_home.as_deref(), Some(claude.as_path()));
        let found = on.discover_plugins_with_extra(&[]).unwrap().found;
        assert!(found.iter().any(|d| d.path == cached), "{found:?}");

        let off = DirectoryScanner::new(&config(false)).unwrap();
        assert!(off.claude_home.is_none());
        let found = off.discover_plugins_with_extra(&[]).unwrap().found;
        assert!(
            !found
                .iter()
                .any(|d| d.source() == DiscoverySource::ClaudeCache),
            "{found:?}"
        );
    }

    /// An index that exists and cannot be read contributes no plugin AND is
    /// named in `unreadable`, so the load that follows can say "unknown"
    /// instead of "none installed" (ruling 5).
    #[test]
    fn an_unreadable_claude_cache_index_is_named_not_emptied() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let temp = TempDir::new().unwrap();
        let claude = temp.path().join("claude");
        claude_home_with_one_install(&claude);
        let index = claude.join("plugins/installed_plugins.json");
        std::fs::write(&index, "{ not json").unwrap();
        let scanner = DirectoryScanner::new(&DiscoveryConfig {
            working_dir: temp.path().to_path_buf(),
            scan_claude_dirs: true,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: Some(claude.clone()),
        })
        .unwrap();
        let discovery = scanner.discover_plugins_with_extra(&[]).unwrap();
        assert!(
            !discovery
                .found
                .iter()
                .any(|d| d.source() == DiscoverySource::ClaudeCache),
            "{discovery:?}"
        );
        assert_eq!(discovery.unreadable, vec![index]);
    }
}
