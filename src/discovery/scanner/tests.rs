use super::*;

use super::super::types::DiscoverySource;
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
fn test_scanner_discovers_global_and_project_scopes_and_skills() {
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
        agents_home: None,
        git_root: Some(root.clone()),
        working_dir: root.join("project"),
        config,
    };

    // Should have Aleph global + project .claude
    let dirs = scanner.get_all_directories().unwrap();
    assert!(!dirs.is_empty());
    assert!(
        dirs.iter()
            .any(|d| d.scope.source() == DiscoverySource::AlephGlobal),
        "the Aleph global scope must be scanned: {dirs:?}"
    );
    assert!(
        dirs.iter()
            .any(|d| d.scope.source() == DiscoverySource::Project),
        "a project scope must be scanned: {dirs:?}"
    );

    // Should find my-skill and project-skill
    let skills = scanner.discover_component("skills").unwrap();
    assert!(!skills.is_empty());
    assert!(
        skills.iter().any(|s| s.path.ends_with("my-skill")),
        "the global skill must be discovered: {skills:?}"
    );
    assert!(
        skills.iter().any(|s| s.path.ends_with("project-skill")),
        "the project skill must be discovered: {skills:?}"
    );
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
        agents_home: None,
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
        agents_home: None,
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
fn test_scanner_discovers_agents_global_and_project_with_tie_order() {
    // The shared `~/.agents` root is scanned at both tiers, and at each
    // tier its priority sorts BEFORE the `.claude` compat band — the
    // scan order is what makes ".agents wins the same-id Bundled tie"
    // (registry keeps the first-registered manifest) a rule, not an
    // accident of directory enumeration.
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join(".git")).unwrap();

    let agents_home = root.join("home/.agents");
    let claude_home = root.join("home/.claude");
    for (dir, name) in [
        (agents_home.join("skills/agents-global"), "agents-global"),
        (claude_home.join("skills/claude-global"), "claude-global"),
        (
            root.join("project/.agents/skills/agents-proj"),
            "agents-proj",
        ),
        (
            root.join("project/.claude/skills/claude-proj"),
            "claude-proj",
        ),
    ] {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
    }

    let empty_home = TempDir::new().unwrap();
    let scanner = DirectoryScanner {
        aleph_home: empty_home.path().to_path_buf(),
        claude_home: Some(claude_home),
        agents_home: Some(agents_home),
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
    let entry = |name: &str| {
        skills
            .iter()
            .find(|s| s.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} must be discovered: {skills:?}"))
    };
    assert_eq!(
        entry("agents-global").scope,
        DiscoveryScope::Global(GlobalRoot::Agents)
    );
    assert!(entry("agents-global").priority < entry("claude-global").priority);
    assert!(entry("agents-proj").priority < entry("claude-proj").priority);
    assert!(entry("agents-proj").priority > entry("claude-global").priority);
}

/// The user's canonical dedup scenario: a skill really installed under
/// `~/.agents/skills`, with a symlink of the same name under
/// `~/.claude/skills`. The target is discovered once at its real
/// location; the symlink is skipped — exactly one entry, no duplicate.
#[cfg(unix)]
#[test]
fn test_symlinked_skill_is_discovered_once_at_its_real_location() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let agents_home = root.join("home/.agents");
    let claude_home = root.join("home/.claude");

    let real = agents_home.join("skills/shared-skill");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("SKILL.md"), "---\nname: shared-skill\n---\n").unwrap();
    std::fs::create_dir_all(claude_home.join("skills")).unwrap();
    std::os::unix::fs::symlink(&real, claude_home.join("skills/shared-skill")).unwrap();

    let empty_home = TempDir::new().unwrap();
    let scanner = DirectoryScanner {
        aleph_home: empty_home.path().to_path_buf(),
        claude_home: Some(claude_home),
        agents_home: Some(agents_home),
        git_root: None,
        working_dir: root.to_path_buf(),
        config: DiscoveryConfig {
            working_dir: root.to_path_buf(),
            scan_claude_dirs: true,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: None,
        },
    };

    let skills = scanner.discover_component("skills").unwrap();
    assert_eq!(
        skills.len(),
        1,
        "symlinked copy must not double-discover: {skills:?}"
    );
    assert!(skills[0].path.ends_with("shared-skill"));
    assert_eq!(
        skills[0].scope,
        DiscoveryScope::Global(GlobalRoot::Agents),
        "the discovered copy is the real one, reached via the .agents root"
    );
}

/// Layer-0 root dedup: two roots naming the same physical directory
/// (here via a symlinked spelling, as with a symlinked $HOME) scan once.
#[cfg(unix)]
#[test]
fn test_scan_roots_dedup_by_canonical_identity() {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let real = root.join("real-shared");
    std::fs::create_dir_all(real.join("skills/s")).unwrap();
    std::fs::write(real.join("skills/s/SKILL.md"), "---\nname: s\n---\n").unwrap();
    let alias = root.join("alias-shared");
    std::os::unix::fs::symlink(&real, &alias).unwrap();

    let empty_home = TempDir::new().unwrap();
    let scanner = DirectoryScanner {
        aleph_home: empty_home.path().to_path_buf(),
        claude_home: Some(alias),
        agents_home: Some(real),
        git_root: None,
        working_dir: root.to_path_buf(),
        config: DiscoveryConfig {
            working_dir: root.to_path_buf(),
            scan_claude_dirs: true,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: None,
        },
    };

    let skills = scanner.discover_component("skills").unwrap();
    assert_eq!(
        skills.len(),
        1,
        "same physical root via two spellings must scan once: {skills:?}"
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
        agents_home: None,
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
        agents_home: None,
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
        agents_home: None,
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
        agents_home: None,
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
        agents_home: None,
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
