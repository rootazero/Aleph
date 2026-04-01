//! Tests for agent manager

use std::fs;

use tempfile::TempDir;
use toml_edit::DocumentMut;

use crate::config::types::agents_def::{
    AgentDefinition, AgentIdentity, AgentModelConfig, AgentParams, SubagentPolicy,
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
            theme: Some("dark".to_string()),
        }),
        model_config: Some(AgentModelConfig {
            primary: "claude-opus-4".to_string(),
            fallbacks: vec!["gpt-4o".to_string()],
        }),
        params: Some(AgentParams {
            temperature: Some(0.7),
            max_tokens: Some(4096),
            top_p: None,
            top_k: None,
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
    assert!(agent.model_config.is_some());
    assert_eq!(
        agent.model_config.as_ref().unwrap().primary,
        "claude-opus-4"
    );
    assert!(agent.params.is_some());
    // f32 -> f64 conversion may lose precision, check approximately
    let temp = agent.params.as_ref().unwrap().temperature.unwrap();
    assert!((temp - 0.7).abs() < 0.01);
    assert_eq!(agent.params.as_ref().unwrap().max_tokens, Some(4096));
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
        params: Some(AgentParams {
            temperature: Some(0.5),
            max_tokens: Some(2048),
            ..Default::default()
        }),
        skills: Some(vec!["git".to_string(), "rust".to_string()]),
        ..Default::default()
    };

    mgr.update("coder", patch).unwrap();

    let agent = mgr.get("coder").unwrap();
    assert_eq!(agent.name, Some("Updated Coder".to_string()));
    assert!(agent.params.is_some());
    let temp = agent.params.as_ref().unwrap().temperature.unwrap();
    assert!((temp - 0.5).abs() < 0.01);
    assert_eq!(agent.params.as_ref().unwrap().max_tokens, Some(2048));
    assert_eq!(
        agent.skills,
        Some(vec!["git".to_string(), "rust".to_string()])
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
