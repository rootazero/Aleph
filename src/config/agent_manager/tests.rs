//! Tests for agent manager

use std::fs;

use tempfile::TempDir;
use toml_edit::DocumentMut;

use crate::config::types::agents_def::{
    AgentDefinition, AgentIdentity, AgentModelRef, SubagentPolicy,
};

use super::{AgentManager, AgentPatch};

/// Create a test environment with config file and directories
fn setup(config_content: &str) -> (TempDir, AgentManager) {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    let workspace_root = dir.path().join("workspaces");
    let agents_root = dir.path().join("agents");
    let trash_root = dir.path().join("trash");

    fs::create_dir_all(&workspace_root).unwrap();
    fs::create_dir_all(&agents_root).unwrap();
    fs::create_dir_all(&trash_root).unwrap();
    fs::write(&config_path, config_content).unwrap();

    let manager = AgentManager::new(config_path, workspace_root, agents_root, trash_root);
    (dir, manager)
}

fn base_config() -> &'static str {
    r#"
[agents]

[[agents.list]]
id = "main"
default = true
name = "Main Agent"

[[agents.list]]
id = "coder"
name = "Coder"
"#
}

// =========================================================================
// List / Get
// =========================================================================

#[test]
fn test_list_agents() {
    let (_dir, mgr) = setup(base_config());
    let agents = mgr.list().unwrap();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].id, "main");
    assert_eq!(agents[1].id, "coder");
}

#[test]
fn test_get_agent() {
    let (_dir, mgr) = setup(base_config());
    let agent = mgr.get("main").unwrap();
    assert_eq!(agent.id, "main");
    assert!(agent.default);
    assert_eq!(agent.name, Some("Main Agent".to_string()));
}

#[test]
fn test_get_agent_not_found() {
    let (_dir, mgr) = setup(base_config());
    let err = mgr.get("nonexistent").unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// =========================================================================
// Create
// =========================================================================

#[test]
fn test_create_agent() {
    let (_dir, mgr) = setup(base_config());
    let def = AgentDefinition {
        id: "researcher".to_string(),
        name: Some("Research Agent".to_string()),
        skills: Some(vec!["search".to_string()]),
        ..Default::default()
    };

    mgr.create(def).unwrap();

    // Verify agent was added
    let agents = mgr.list().unwrap();
    assert_eq!(agents.len(), 3);
    let new_agent = agents.iter().find(|a| a.id == "researcher").unwrap();
    assert_eq!(new_agent.name, Some("Research Agent".to_string()));

    // Verify agent identity directory was created with SOUL.md
    let agent_dir = mgr.agents_root.join("researcher");
    assert!(agent_dir.exists());

    let soul = fs::read_to_string(agent_dir.join("SOUL.md")).unwrap();
    assert!(soul.contains("Research Agent"));

    // Verify TOML is valid by re-parsing
    let content = fs::read_to_string(&mgr.config_path).unwrap();
    let _doc: DocumentMut = content.parse().unwrap();
}

#[test]
fn test_create_duplicate_fails() {
    let (_dir, mgr) = setup(base_config());
    let def = AgentDefinition {
        id: "main".to_string(),
        ..Default::default()
    };

    let err = mgr.create(def).unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn test_create_invalid_id_empty() {
    let (_dir, mgr) = setup(base_config());
    let def = AgentDefinition {
        id: "".to_string(),
        ..Default::default()
    };
    let err = mgr.create(def).unwrap_err();
    assert!(err.to_string().contains("1-32"));
}

#[test]
fn test_create_invalid_id_too_long() {
    let (_dir, mgr) = setup(base_config());
    let def = AgentDefinition {
        id: "a".repeat(33),
        ..Default::default()
    };
    let err = mgr.create(def).unwrap_err();
    assert!(err.to_string().contains("1-32"));
}

#[test]
fn test_create_invalid_id_special_chars() {
    let (_dir, mgr) = setup(base_config());
    let def = AgentDefinition {
        id: "agent/evil".to_string(),
        ..Default::default()
    };
    let err = mgr.create(def).unwrap_err();
    assert!(err.to_string().contains("invalid characters"));
}

#[test]
fn test_create_agent_with_all_fields() {
    let (_dir, mgr) = setup(base_config());
    let def = AgentDefinition {
        id: "full-agent".to_string(),
        name: Some("Full Agent".to_string()),
        default: false,
        identity: Some(AgentIdentity {
            emoji: Some("\u{1f916}".to_string()),
            description: Some("A full agent".to_string()),
            avatar: None,
        }),
        skills: Some(vec!["code".to_string(), "search".to_string()]),
        subagents: Some(SubagentPolicy {
            allow: vec!["helper".to_string()],
        }),
        ..Default::default()
    };

    mgr.create(def).unwrap();

    // Re-read and verify
    let agent = mgr.get("full-agent").unwrap();
    assert_eq!(agent.name, Some("Full Agent".to_string()));
    assert!(agent.identity.is_some());
    assert_eq!(
        agent.identity.as_ref().unwrap().emoji,
        Some("\u{1f916}".to_string())
    );
    assert_eq!(
        agent.skills,
        Some(vec!["code".to_string(), "search".to_string()])
    );
    assert!(agent.subagents.is_some());
    assert_eq!(agent.subagents.as_ref().unwrap().allow, vec!["helper"]);
}

#[test]
fn test_create_creates_both_directories() {
    let (_dir, mgr) = setup(base_config());
    let def = AgentDefinition {
        id: "dual".to_string(),
        name: Some("Dual Agent".to_string()),
        ..Default::default()
    };

    mgr.create(def).unwrap();

    // Agent identity dir has identity files + sessions
    assert!(mgr.agents_root.join("dual").join("SOUL.md").exists());
    assert!(mgr.agents_root.join("dual").join("MEMORY.md").exists());
    assert!(mgr.agents_root.join("dual").join("sessions").is_dir());

    // Identity files should NOT be in workspace
    assert!(!mgr.workspace_root.join("dual").join("SOUL.md").exists());
}

#[test]
fn test_delete_trashes_both_directories() {
    let (_dir, mgr) = setup(base_config());

    // Pre-create both dirs for coder
    fs::create_dir_all(mgr.workspace_root.join("coder")).unwrap();
    fs::create_dir_all(mgr.agents_root.join("coder").join("sessions")).unwrap();
    fs::write(mgr.agents_root.join("coder").join("SOUL.md"), "test").unwrap();

    mgr.delete("coder").unwrap();

    assert!(!mgr.workspace_root.join("coder").exists());
    assert!(!mgr.agents_root.join("coder").exists());
}

// =========================================================================
// Update
// =========================================================================

#[test]
fn test_update_agent() {
    let (_dir, mgr) = setup(base_config());

    let patch = AgentPatch {
        name: Some("Updated Coder".to_string()),
        skills: Some(vec!["git".to_string(), "rust".to_string()]),
        ..Default::default()
    };

    mgr.update("coder", patch).unwrap();

    let agent = mgr.get("coder").unwrap();
    assert_eq!(agent.name, Some("Updated Coder".to_string()));
    assert_eq!(
        agent.skills,
        Some(vec!["git".to_string(), "rust".to_string()])
    );
}

#[test]
fn test_update_identity_preserves_unpatched_fields() {
    let (_dir, mgr) = setup(base_config());

    // Seed a full identity including an avatar.
    mgr.update(
        "coder",
        AgentPatch {
            identity: Some(AgentIdentity {
                emoji: Some("\u{1f916}".to_string()),
                description: Some("first".to_string()),
                avatar: Some("avatar.png".to_string()),
            }),
            ..Default::default()
        },
    )
    .unwrap();

    // Patch identity with only emoji/description — mirrors the Overview tab,
    // which never sends avatar. The avatar must survive (merge, not replace).
    mgr.update(
        "coder",
        AgentPatch {
            identity: Some(AgentIdentity {
                emoji: Some("\u{1f980}".to_string()),
                description: Some("second".to_string()),
                avatar: None,
            }),
            ..Default::default()
        },
    )
    .unwrap();

    let agent = mgr.get("coder").unwrap();
    let identity = agent.identity.expect("identity present");
    assert_eq!(identity.emoji.as_deref(), Some("\u{1f980}"));
    assert_eq!(identity.description.as_deref(), Some("second"));
    assert_eq!(
        identity.avatar.as_deref(),
        Some("avatar.png"),
        "avatar must be preserved when a later identity patch omits it"
    );
}

#[test]
fn test_update_nonexistent_fails() {
    let (_dir, mgr) = setup(base_config());
    let patch = AgentPatch {
        name: Some("Ghost".to_string()),
        ..Default::default()
    };
    let err = mgr.update("ghost", patch).unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// =========================================================================
// Delete
// =========================================================================

#[test]
fn test_delete_agent() {
    let (_dir, mgr) = setup(base_config());

    // Create workspace for coder
    let ws_dir = mgr.workspace_root.join("coder");
    fs::create_dir_all(&ws_dir).unwrap();
    fs::write(ws_dir.join("test.txt"), "hello").unwrap();

    mgr.delete("coder").unwrap();

    // Verify removed from list
    let agents = mgr.list().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, "main");

    // Verify workspace moved to trash
    assert!(!ws_dir.exists());
    let trash_entries: Vec<_> = fs::read_dir(&mgr.trash_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(trash_entries.len(), 1);
    let trash_name = trash_entries[0].file_name().to_string_lossy().to_string();
    assert!(trash_name.starts_with("coder_"));
}

#[test]
fn test_delete_only_agent_fails() {
    let config = r#"
[agents]

[[agents.list]]
id = "solo"
default = true
name = "Solo Agent"
"#;
    let (_dir, mgr) = setup(config);
    let err = mgr.delete("solo").unwrap_err();
    assert!(err.to_string().contains("only agent"));
}

#[test]
fn test_delete_default_agent_fails() {
    let (_dir, mgr) = setup(base_config());
    let err = mgr.delete("main").unwrap_err();
    assert!(err.to_string().contains("default agent"));
}

#[test]
fn test_delete_nonexistent_fails() {
    let (_dir, mgr) = setup(base_config());
    let err = mgr.delete("ghost").unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// =========================================================================
// Set Default
// =========================================================================

#[test]
fn test_set_default() {
    let (_dir, mgr) = setup(base_config());

    mgr.set_default("coder").unwrap();

    let agents = mgr.list().unwrap();
    let main = agents.iter().find(|a| a.id == "main").unwrap();
    let coder = agents.iter().find(|a| a.id == "coder").unwrap();
    assert!(!main.default);
    assert!(coder.default);
}

#[test]
fn test_set_default_nonexistent_fails() {
    let (_dir, mgr) = setup(base_config());
    let err = mgr.set_default("ghost").unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// =========================================================================
// Workspace file operations
// =========================================================================

#[test]
fn test_workspace_file_operations() {
    let (_dir, mgr) = setup(base_config());

    // Write a file
    mgr.write_file("main", "test.md", "# Test\nHello world")
        .unwrap();

    // List files
    let files = mgr.list_files("main").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename, "test.md");
    assert!(!files[0].is_bootstrap);
    assert!(files[0].size_bytes > 0);

    // Read file
    let content = mgr.read_file("main", "test.md").unwrap();
    assert_eq!(content, "# Test\nHello world");

    // Delete file
    mgr.delete_file("main", "test.md").unwrap();
    let files = mgr.list_files("main").unwrap();
    assert!(files.is_empty());
}

#[test]
fn test_workspace_bootstrap_file_detection() {
    let (_dir, mgr) = setup(base_config());

    mgr.write_file("main", "SOUL.md", "# Soul").unwrap();
    mgr.write_file("main", "custom.md", "# Custom").unwrap();

    let files = mgr.list_files("main").unwrap();
    assert_eq!(files.len(), 2);

    let soul = files.iter().find(|f| f.filename == "SOUL.md").unwrap();
    assert!(soul.is_bootstrap);

    let custom = files.iter().find(|f| f.filename == "custom.md").unwrap();
    assert!(!custom.is_bootstrap);
}

#[test]
fn test_workspace_list_nonexistent_returns_empty() {
    let (_dir, mgr) = setup(base_config());
    let files = mgr.list_files("nonexistent").unwrap();
    assert!(files.is_empty());
}

#[test]
fn test_filename_path_traversal_blocked() {
    let (_dir, mgr) = setup(base_config());

    // Various path traversal attempts
    assert!(mgr.read_file("main", "../secret").is_err());
    assert!(mgr.read_file("main", "foo/bar").is_err());
    assert!(mgr.read_file("main", "foo\\bar").is_err());
    assert!(mgr.read_file("main", "..").is_err());

    assert!(mgr.write_file("main", "../evil", "pwned").is_err());
    assert!(mgr.write_file("main", "a/b", "pwned").is_err());

    assert!(mgr.delete_file("main", "../gone").is_err());
}

// =========================================================================
// Validate ID
// =========================================================================

#[test]
fn test_validate_id_valid() {
    let (_dir, mgr) = setup(base_config());
    assert!(mgr.validate_id("my-agent").is_ok());
    assert!(mgr.validate_id("agent_1").is_ok());
    assert!(mgr.validate_id("a").is_ok());
    assert!(mgr.validate_id("Agent-X_99").is_ok());
}

#[test]
fn test_validate_id_invalid() {
    let (_dir, mgr) = setup(base_config());
    assert!(mgr.validate_id("").is_err());
    assert!(mgr.validate_id("has space").is_err());
    assert!(mgr.validate_id("has.dot").is_err());
    assert!(mgr.validate_id("has/slash").is_err());
    assert!(mgr.validate_id(&"x".repeat(33)).is_err());
}

// =========================================================================
// Empty config edge case
// =========================================================================

#[test]
fn test_empty_config_create_first_agent() {
    let (_dir, mgr) = setup("");
    let def = AgentDefinition {
        id: "first".to_string(),
        default: true,
        name: Some("First Agent".to_string()),
        ..Default::default()
    };

    mgr.create(def).unwrap();

    let agents = mgr.list().unwrap();
    assert_eq!(agents.len(), 2); // "main" auto-created + "first"
    assert!(
        agents.iter().any(|a| a.id == "main"),
        "auto-created main agent"
    );
    assert!(
        agents.iter().any(|a| a.id == "first"),
        "explicitly created first agent"
    );
}

// =========================================================================
// Model patch — tri-state semantics
// =========================================================================

#[test]
fn update_sets_qualified_model() {
    let (_dir, mgr) = setup(base_config());
    let patch = AgentPatch {
        model: Some(Some(AgentModelRef::Qualified {
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
        })),
        ..Default::default()
    };
    mgr.update("main", patch).unwrap();
    let def = mgr.get("main").unwrap();
    assert_eq!(
        def.model,
        Some(AgentModelRef::Qualified {
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
        })
    );
}

#[test]
fn update_clears_model_to_inherit() {
    let (_dir, mgr) = setup(base_config());
    // First set a model
    mgr.update(
        "main",
        AgentPatch {
            model: Some(Some(AgentModelRef::Legacy("gpt-5".into()))),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(mgr.get("main").unwrap().model.is_some());
    // Now clear it
    mgr.update(
        "main",
        AgentPatch {
            model: Some(None),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(mgr.get("main").unwrap().model.is_none());
}

#[test]
fn update_absent_model_leaves_it_untouched() {
    let (_dir, mgr) = setup(base_config());
    // Set a model
    mgr.update(
        "main",
        AgentPatch {
            model: Some(Some(AgentModelRef::Legacy("gpt-5".into()))),
            ..Default::default()
        },
    )
    .unwrap();
    // Update name only (model key absent)
    mgr.update(
        "main",
        AgentPatch {
            name: Some("Renamed".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        mgr.get("main").unwrap().model,
        Some(AgentModelRef::Legacy("gpt-5".into()))
    );
}

#[test]
fn agent_patch_model_double_option_wire() {
    // absent → None
    let p: AgentPatch = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(p.model.is_none());
    // explicit null → Some(None) (clear)
    let p: AgentPatch = serde_json::from_value(serde_json::json!({ "model": null })).unwrap();
    assert_eq!(p.model, Some(None));
    // object → Some(Some(Qualified))
    let p: AgentPatch = serde_json::from_value(
        serde_json::json!({"model": {"provider": "openai", "model": "gpt-5"}}),
    )
    .unwrap();
    assert_eq!(
        p.model,
        Some(Some(AgentModelRef::Qualified {
            provider: "openai".into(),
            model: "gpt-5".into(),
        }))
    );
}

// =========================================================================
// TOML format preservation
// =========================================================================

#[test]
fn test_toml_roundtrip_preserves_other_sections() {
    let config = r#"
[general]
language = "zh"

[agents]

[[agents.list]]
id = "main"
default = true
name = "Main Agent"

[memory]
enabled = true
"#;
    let (_dir, mgr) = setup(config);

    // Create a new agent
    let def = AgentDefinition {
        id: "new-one".to_string(),
        name: Some("New".to_string()),
        ..Default::default()
    };
    mgr.create(def).unwrap();

    // Verify other sections are preserved
    let content = fs::read_to_string(&mgr.config_path).unwrap();
    assert!(content.contains("[general]"));
    assert!(content.contains("language = \"zh\""));
    assert!(content.contains("[memory]"));
    assert!(content.contains("enabled = true"));
}

// =============================================================================
// Provisioning roots ↔ resolver agreement
// =============================================================================

/// Where a tool *writes* an agent must equal where the resolver *rebuilds* it.
///
/// These are the two halves of one agent's life: `agent_create` / `team_create`
/// / template materialization create the directories, and on the next boot
/// `AgentDefinitionResolver` decides where that agent's `agent_dir` and
/// `workspace` are. The provisioning half used to restate the rule instead of
/// applying it, and the restatement dropped `[agents.defaults] agents_root /
/// workspace_root`: with either configured, the tool wrote one tree and the
/// resolver addressed another. Nothing errored — the agent simply came back
/// with no SOUL.md and an empty workspace.
///
/// Both roots are configured to *non-default* locations on purpose. Leaving
/// them unset makes the two sides agree for the uninteresting reason (they
/// both fall back to the same default), which is precisely the shape of the
/// bug: it is invisible on any machine that never sets these keys.
#[test]
fn provisioning_writes_where_the_resolver_will_look() {
    use crate::config::agent_resolver::{
        agents_root_for, workspace_root_for, AgentDefinitionResolver,
    };
    use crate::config::types::agents_def::AgentDefaults;

    let dir = TempDir::new().unwrap();
    let configured_agents = dir.path().join("elsewhere").join("agent-state");
    let configured_workspaces = dir.path().join("somewhere-else").join("ws");

    let defaults = AgentDefaults {
        agents_root: Some(configured_agents.clone()),
        workspace_root: Some(configured_workspaces.clone()),
        ..Default::default()
    };

    // Boot's step: apply the rule once, hand the result to the manager.
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, base_config()).unwrap();
    let manager = AgentManager::new(
        config_path,
        workspace_root_for(&defaults),
        agents_root_for(&defaults),
        dir.path().join("trash"),
    );

    // A provisioning tool's step: read the roots back off the manager.
    let roots = super::provisioning_roots(Some(&manager));

    // The resolver's step, on the next boot.
    let agent = AgentDefinition {
        id: "member-a".to_string(),
        ..Default::default()
    };
    let resolver = AgentDefinitionResolver::new();

    assert_eq!(
        roots.agents.join(&agent.id),
        resolver.resolve_agent_dir(&agent, &defaults),
        "agent_create/team_create write the state dir somewhere the resolver \
         will not rebuild it from"
    );
    assert_eq!(
        roots.workspaces.join(&agent.id),
        resolver.resolve_workspace_path(&agent, &defaults),
        "provisioning creates the workspace somewhere the resolver will not \
         address it"
    );

    // And the configured roots really are the ones in play — if this were
    // reading the unconfigured default the assertions above would still pass.
    assert!(roots.agents.starts_with(&configured_agents));
    assert!(roots.workspaces.starts_with(&configured_workspaces));
}

/// Without a manager the roots must still be the *unconfigured* defaults —
/// the answer those callers (tests, embedded hosts, minimal servers) resolved
/// before `provisioning_roots` existed. A `None` that silently became
/// something else would move every one of them.
#[test]
fn no_manager_falls_back_to_the_unconfigured_defaults() {
    let roots = super::provisioning_roots(None);
    assert_eq!(
        roots.agents,
        crate::config::agent_resolver::default_agents_root()
    );
    assert_eq!(
        roots.workspaces,
        crate::config::agent_resolver::default_workspace_root()
    );
}

/// Files allowed to reach `agent_resolver::default_{agents,workspace}_root()`
/// directly instead of going through [`super::provisioning_roots`], each with
/// the reason it is not answering the provisioning question.
///
/// Entries state why the *configured* roots are not the right answer there —
/// not "fix this later". A list of known-wrong sites is a licence that expires
/// only when someone re-reads it, which is how the sixteen entries of
/// `utils::paths`' `HOME_JOIN_PENDING_FIX` shipped for months.
const DIRECT_DEFAULT_ROOT_ALLOWLIST: &[(&str, &str)] = &[
    (
        "src/gateway/agent_instance.rs",
        "`Default` takes no arguments, so it cannot be handed resolved roots; \
         its two path fields have no production reader (every `..default()` \
         site is under `#[cfg(test)]`, and the one production caller reads \
         only `.model`)",
    ),
    (
        "src/gateway/config.rs",
        "the legacy `[agents.<id>]` schema, which predates `[agents.defaults]` \
         and lives on `GatewayConfig` — a different struct from the app \
         `Config` that carries the defaults",
    ),
    (
        "src/memory/scratchpad/manager.rs",
        "a per-project scratchpad keyed by project_id, not an agent directory; \
         see that module's doc, which names the rule deliberately",
    ),
    (
        "src/utils/paths.rs",
        "the ALEPH_HOME containment test, which asserts the *unconfigured* \
         defaults land inside a relocated home",
    ),
];

/// Split a source file into the lines that are real code.
///
/// Deliberately not anchored to `\n`: this repo is checked out CRLF on
/// Windows, where a separator written as `"\n..."` matches nothing and the
/// scan silently covers the whole file.
fn provisioning_code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> + '_ {
    text.lines().enumerate().filter_map(|(i, line)| {
        let code = line.trim_start();
        if code.starts_with("//") || code.starts_with('*') {
            None
        } else {
            Some((i + 1, line))
        }
    })
}

fn sources_reaching_the_unconfigured_default() -> Vec<(String, Vec<String>)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &mut files);
    assert!(files.len() > 100, "walk found suspiciously few sources");

    let mut hits = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(env!("CARGO_MANIFEST_DIR"))
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        // The two modules that own the rule are allowed to state it.
        if rel == "src/config/agent_resolver/mod.rs" || rel.starts_with("src/config/agent_manager/")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Qualified on purpose: `sandbox/config.rs` has a *local*
        // `default_workspace_root` for the unrelated `[sandbox] workspace_root`
        // knob, and matching on the bare name would rope it in.
        let sites: Vec<String> = provisioning_code_lines(&text)
            .filter(|(_, l)| {
                l.contains("agent_resolver::default_agents_root()")
                    || l.contains("agent_resolver::default_workspace_root()")
            })
            .map(|(n, l)| format!("{rel}:{n}: {}", l.trim()))
            .collect();
        if !sites.is_empty() {
            hits.push((rel, sites));
        }
    }
    hits
}

/// Provisioning must not reach past the resolved roots.
///
/// `agent_create`, `team_create` and template materialization all create an
/// agent's directories; `agent_delete` archives them. Every one of them once
/// restated the layout rule instead of applying it, and each restatement
/// dropped `[agents.defaults]`. Reaching for the *unconfigured* default is the
/// shape of that bug, so it is what this scans for — a new provisioning site
/// that does it has to say why in the allowlist, in front of a reviewer.
#[test]
fn no_provisioning_site_reaches_past_the_resolved_roots() {
    let offenders: Vec<String> = sources_reaching_the_unconfigured_default()
        .into_iter()
        .filter(|(rel, _)| !DIRECT_DEFAULT_ROOT_ALLOWLIST.iter().any(|(f, _)| f == rel))
        .map(|(_, sites)| sites.join("\n    "))
        .collect();

    assert!(
        offenders.is_empty(),
        "these use the unconfigured default root instead of \
         config::agent_manager::provisioning_roots(), so they ignore \
         `[agents.defaults] agents_root / workspace_root` and will create or \
         archive agent directories where AgentDefinitionResolver does not look \
         — with no error on either side:\n  {}",
        offenders.join("\n  ")
    );
}

/// An allowlist entry that no longer applies is a licence nobody asked for.
#[test]
fn every_direct_default_root_exemption_is_still_used() {
    let reaching: Vec<String> = sources_reaching_the_unconfigured_default()
        .into_iter()
        .map(|(rel, _)| rel)
        .collect();
    let stale: Vec<&str> = DIRECT_DEFAULT_ROOT_ALLOWLIST
        .iter()
        .map(|(f, _)| *f)
        .filter(|f| !reaching.iter().any(|r| r == f))
        .collect();
    assert!(
        stale.is_empty(),
        "these no longer call the unconfigured default root (fixed, moved, or \
         deleted) — delete their DIRECT_DEFAULT_ROOT_ALLOWLIST entry so the \
         list keeps meaning what it says:\n  {}",
        stale.join("\n  ")
    );
}
