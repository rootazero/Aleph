//! Directory scanner for discovering components
//!
//! Implements the multi-directory scanning strategy with upward traversal.

use super::paths::{
    aleph_home_dir, claude_home_dir, find_dir_upward, find_git_root, AGENT_FILE, ALEPH_HOME_DIR,
    CLAUDE_HOME_DIR, MCP_CONFIG_FILE, PLUGINS_DIR, PLUGIN_MANIFEST_DIR, PLUGIN_MANIFEST_FILE,
    SKILL_FILE,
};
use super::types::{DiscoveredPath, DiscoverySource, ScanDirectory};
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

        // Claude home is optional (only scan if it exists)
        let claude_home = if config.scan_claude_dirs {
            match claude_home_dir() {
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
                DiscoverySource::ClaudeGlobal,
                0,
            ));
        }

        // 2. Aleph global
        if self.aleph_home.exists() {
            dirs.push(ScanDirectory::new(
                self.aleph_home.clone(),
                DiscoverySource::AlephGlobal,
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
                dirs.push(ScanDirectory::new(dir, DiscoverySource::Project, priority));
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
                dirs.push(ScanDirectory::new(dir, DiscoverySource::Project, priority));
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

        // Sort by priority (lower first). The single consumer in
        // `extension::ExtensionManager` uses first-wins dedup via
        // `seen.insert(canonical)`, so the LOWER priority is the one that
        // survives a name/path collision — keep that invariant documented
        // here so callers know which direction the dedup goes.
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
    pub fn discover_plugins_with_extra(
        &self,
        extra_parents: &[PathBuf],
    ) -> DiscoveryResult<Vec<DiscoveredPath>> {
        let mut discovered = Vec::new();
        self.scan_plugin_parent(
            &self.aleph_home.join(PLUGINS_DIR),
            &mut discovered,
            DiscoverySource::AlephGlobal,
            10,
        );
        for parent in extra_parents {
            self.scan_plugin_parent(parent, &mut discovered, DiscoverySource::Project, 20);
        }
        // Match `discover_component`: ascending-priority sort so the single
        // downstream consumer's first-wins dedup (which keeps the FIRST
        // entry on a canonical-path collision) has the same semantics across
        // both APIs. Without this, `discover_plugins_with_extra` returns
        // global-then-project in raw read_dir order, which makes the dedup
        // pick GLOBAL on a project/global clash — the opposite of what
        // `discover_component` does for skills/commands/agents.
        discovered.sort_by_key(|d| d.priority);
        trace!(
            "Discovered {} plugins ({} extra parents)",
            discovered.len(),
            extra_parents.len()
        );
        Ok(discovered)
    }

    /// Scan a single plugin-parent directory, pushing each plugin root (direct
    /// manifest or one-level monorepo subdir) into `discovered`. A missing or
    /// unreadable parent is a silent no-op.
    fn scan_plugin_parent(
        &self,
        plugins_dir: &Path,
        discovered: &mut Vec<DiscoveredPath>,
        source: DiscoverySource,
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
                discovered.push(DiscoveredPath::new(path, source, priority));
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
                        discovered.push(DiscoveredPath::new(sub_path, source, priority));
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
        out.push(DiscoveredPath::new(
            path.to_path_buf(),
            scan_dir.source,
            scan_dir.priority,
        ));
    }
}

/// `DirEntry::path().is_dir()` follows symlinks; `symlink_metadata` reports
/// the link's own file type. This stops a symlink inside `~/.aleph/{skills,
/// commands, plugins}` pointing outside the expected tree from being
/// enumerated as a discovered component.
fn is_existing_dir_no_follow(path: &Path) -> bool {
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
            .any(|d| d.source == DiscoverySource::AlephGlobal));
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

        let plugins = scanner.discover_plugins_with_extra(&[]).unwrap();
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

        let plugins = scanner.discover_plugins_with_extra(&[]).unwrap();
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
        assert_eq!(scanner.discover_plugins_with_extra(&[]).unwrap().len(), 1);

        // With the project's plugin parent, both are discovered.
        let plugins = scanner
            .discover_plugins_with_extra(&[project.join(".aleph/plugins")])
            .unwrap();
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
                project.join(".aleph/plugins"),
                root.join("workspace/proj-missing/.aleph/plugins"),
            ])
            .unwrap();
        assert_eq!(plugins2.len(), 2);
    }
}
