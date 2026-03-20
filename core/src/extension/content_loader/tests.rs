use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_load_skill_from_directory() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join("my-skill");
    std::fs::create_dir(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
description: Test skill
---

Hello $ARGUMENTS!"#,
    )
    .unwrap();

    let loader = ContentLoader::new();
    let skill = loader.load_skill(&skill_dir).await.unwrap();

    assert_eq!(skill.name, "my-skill");
    assert_eq!(skill.description, "Test skill");
    assert!(skill.content.contains("Hello $ARGUMENTS!"));
}

#[tokio::test]
async fn test_load_agent() {
    let temp = TempDir::new().unwrap();
    let agent_file = temp.path().join("reviewer.md");
    std::fs::write(
        &agent_file,
        r#"---
mode: subagent
description: Code reviewer
model: anthropic/claude-haiku
temperature: 0.3
---

You are a code reviewer..."#,
    )
    .unwrap();

    let loader = ContentLoader::new();
    let agent = loader.load_agent(&agent_file).await.unwrap();

    assert_eq!(agent.name, "reviewer");
    assert_eq!(agent.mode, AgentMode::Subagent);
    assert_eq!(agent.model, Some("anthropic/claude-haiku".to_string()));
    assert_eq!(agent.temperature, Some(0.3));
}

#[tokio::test]
async fn test_load_v2_prompt() {
    use crate::extension::manifest::PromptSection;
    use crate::extension::types::PluginKind;

    let temp = TempDir::new().unwrap();
    let plugin_dir = temp.path();

    // Write the prompt file
    std::fs::write(
        plugin_dir.join("SYSTEM.md"),
        r#"---
name: my-system-prompt
description: A system prompt
---

You are a helpful assistant."#,
    )
    .unwrap();

    // Create a manifest with prompt_v2
    let manifest = crate::extension::manifest::PluginManifest {
        id: "test-plugin".to_string(),
        name: "Test Plugin".to_string(),
        version: Some("1.0.0".to_string()),
        description: None,
        kind: PluginKind::Static,
        entry: ".".into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: None,
        config_ui_hints: std::collections::HashMap::new(),
        permissions: vec![],
        author: None,
        homepage: None,
        repository: None,
        license: None,
        keywords: vec![],
        extensions: vec![],
        tools_v2: None,
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: Some(PromptSection {
            file: "SYSTEM.md".to_string(),
            scope: "system".to_string(),
        }),
        capabilities_v2: None,
        wasm_capabilities: None,
        wasm_resource_limits: None,
        // P2 fields
        channels_v2: None,
        providers_v2: None,
        http_routes_v2: None,
        // CC-compat extensions
        aleph_extensions: None,
    };

    let loader = ContentLoader::new();
    let skill = loader.load_v2_prompt(&manifest, plugin_dir).await.unwrap();

    assert!(skill.is_some());
    let skill = skill.unwrap();
    assert_eq!(skill.name, "my-system-prompt");
    assert_eq!(skill.description, "A system prompt");
    assert_eq!(skill.plugin_name, Some("test-plugin".to_string()));
    assert_eq!(skill.scope, PromptScope::System);
    assert!(skill.content.contains("You are a helpful assistant."));
}

#[tokio::test]
async fn test_load_v2_prompt_disabled() {
    use crate::extension::manifest::PromptSection;
    use crate::extension::types::PluginKind;

    let temp = TempDir::new().unwrap();
    let plugin_dir = temp.path();

    // Create a manifest with disabled prompt
    let manifest = crate::extension::manifest::PluginManifest {
        id: "test-plugin".to_string(),
        name: "Test Plugin".to_string(),
        version: None,
        description: None,
        kind: PluginKind::Static,
        entry: ".".into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: None,
        config_ui_hints: std::collections::HashMap::new(),
        permissions: vec![],
        author: None,
        homepage: None,
        repository: None,
        license: None,
        keywords: vec![],
        extensions: vec![],
        tools_v2: None,
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: Some(PromptSection {
            file: "SYSTEM.md".to_string(),
            scope: "disabled".to_string(),
        }),
        capabilities_v2: None,
        wasm_capabilities: None,
        wasm_resource_limits: None,
        // P2 fields
        channels_v2: None,
        providers_v2: None,
        http_routes_v2: None,
        // CC-compat extensions
        aleph_extensions: None,
    };

    let loader = ContentLoader::new();
    let skill = loader.load_v2_prompt(&manifest, plugin_dir).await.unwrap();

    // Should return None for disabled prompt
    assert!(skill.is_none());
}

#[tokio::test]
async fn test_load_v2_prompt_no_frontmatter() {
    use crate::extension::manifest::PromptSection;
    use crate::extension::types::PluginKind;

    let temp = TempDir::new().unwrap();
    let plugin_dir = temp.path();

    // Write a prompt file without frontmatter
    std::fs::write(
        plugin_dir.join("SIMPLE.md"),
        "Just plain content without frontmatter.",
    )
    .unwrap();

    let manifest = crate::extension::manifest::PluginManifest {
        id: "simple-plugin".to_string(),
        name: "Simple Plugin".to_string(),
        version: None,
        description: None,
        kind: PluginKind::Static,
        entry: ".".into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: None,
        config_ui_hints: std::collections::HashMap::new(),
        permissions: vec![],
        author: None,
        homepage: None,
        repository: None,
        license: None,
        keywords: vec![],
        extensions: vec![],
        tools_v2: None,
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: Some(PromptSection {
            file: "SIMPLE.md".to_string(),
            scope: "user".to_string(),
        }),
        capabilities_v2: None,
        wasm_capabilities: None,
        wasm_resource_limits: None,
        // P2 fields
        channels_v2: None,
        providers_v2: None,
        http_routes_v2: None,
        // CC-compat extensions
        aleph_extensions: None,
    };

    let loader = ContentLoader::new();
    let skill = loader.load_v2_prompt(&manifest, plugin_dir).await.unwrap();

    assert!(skill.is_some());
    let skill = skill.unwrap();
    // Name should default to plugin id
    assert_eq!(skill.name, "simple-plugin");
    assert_eq!(skill.content, "Just plain content without frontmatter.");
}

#[tokio::test]
async fn test_load_v2_tool_prompts() {
    use crate::extension::manifest::ToolSection;
    use crate::extension::types::PluginKind;

    let temp = TempDir::new().unwrap();
    let plugin_dir = temp.path();

    // Create instructions directory
    let instructions_dir = plugin_dir.join("instructions");
    std::fs::create_dir(&instructions_dir).unwrap();

    // Write instruction files
    std::fs::write(
        instructions_dir.join("my-tool.md"),
        "Instructions for using my-tool.",
    )
    .unwrap();
    std::fs::write(
        instructions_dir.join("other-tool.md"),
        "Instructions for using other-tool.",
    )
    .unwrap();

    let manifest = crate::extension::manifest::PluginManifest {
        id: "tool-plugin".to_string(),
        name: "Tool Plugin".to_string(),
        version: None,
        description: None,
        kind: PluginKind::Wasm,
        entry: "plugin.wasm".into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: None,
        config_ui_hints: std::collections::HashMap::new(),
        permissions: vec![],
        author: None,
        homepage: None,
        repository: None,
        license: None,
        keywords: vec![],
        extensions: vec![],
        tools_v2: Some(vec![
            ToolSection {
                name: "my-tool".to_string(),
                description: Some("My tool".to_string()),
                handler: Some("handle_my_tool".to_string()),
                instruction_file: Some("instructions/my-tool.md".to_string()),
                parameters: None,
            },
            ToolSection {
                name: "other-tool".to_string(),
                description: Some("Other tool".to_string()),
                handler: Some("handle_other_tool".to_string()),
                instruction_file: Some("instructions/other-tool.md".to_string()),
                parameters: None,
            },
            ToolSection {
                name: "no-instruction-tool".to_string(),
                description: Some("Tool without instructions".to_string()),
                handler: Some("handle_no_instruction".to_string()),
                instruction_file: None,
                parameters: None,
            },
        ]),
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: None,
        capabilities_v2: None,
        wasm_capabilities: None,
        wasm_resource_limits: None,
        // P2 fields
        channels_v2: None,
        providers_v2: None,
        http_routes_v2: None,
        // CC-compat extensions
        aleph_extensions: None,
    };

    let loader = ContentLoader::new();
    let skills = loader.load_v2_tool_prompts(&manifest, plugin_dir).await.unwrap();

    // Should load 2 instruction files (the third tool has no instruction_file)
    assert_eq!(skills.len(), 2);

    // Check first skill
    let my_tool_skill = skills.iter().find(|s| s.name == "my-tool_instructions").unwrap();
    assert_eq!(my_tool_skill.plugin_name, Some("tool-plugin".to_string()));
    assert_eq!(my_tool_skill.scope, PromptScope::Tool);
    assert_eq!(my_tool_skill.bound_tool, Some("my-tool".to_string()));
    assert!(my_tool_skill.disable_model_invocation);
    assert!(my_tool_skill.content.contains("Instructions for using my-tool."));

    // Check second skill
    let other_tool_skill = skills.iter().find(|s| s.name == "other-tool_instructions").unwrap();
    assert_eq!(other_tool_skill.bound_tool, Some("other-tool".to_string()));
}

#[tokio::test]
async fn test_load_v2_tool_prompts_missing_file() {
    use crate::extension::manifest::ToolSection;
    use crate::extension::types::PluginKind;

    let temp = TempDir::new().unwrap();
    let plugin_dir = temp.path();

    // No instruction file exists
    let manifest = crate::extension::manifest::PluginManifest {
        id: "tool-plugin".to_string(),
        name: "Tool Plugin".to_string(),
        version: None,
        description: None,
        kind: PluginKind::Wasm,
        entry: "plugin.wasm".into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: None,
        config_ui_hints: std::collections::HashMap::new(),
        permissions: vec![],
        author: None,
        homepage: None,
        repository: None,
        license: None,
        keywords: vec![],
        extensions: vec![],
        tools_v2: Some(vec![
            ToolSection {
                name: "my-tool".to_string(),
                description: Some("My tool".to_string()),
                handler: Some("handle_my_tool".to_string()),
                instruction_file: Some("nonexistent.md".to_string()),
                parameters: None,
            },
        ]),
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: None,
        capabilities_v2: None,
        wasm_capabilities: None,
        wasm_resource_limits: None,
        // P2 fields
        channels_v2: None,
        providers_v2: None,
        http_routes_v2: None,
        // CC-compat extensions
        aleph_extensions: None,
    };

    let loader = ContentLoader::new();
    let skills = loader.load_v2_tool_prompts(&manifest, plugin_dir).await.unwrap();

    // Should gracefully skip missing files
    assert_eq!(skills.len(), 0);
}
