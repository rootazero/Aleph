use super::super::types::{ChannelType, ConflictInfo, ConflictResolution, ToolSource};
use super::*;
use crate::tool_metadata::types::ToolPriority;

#[tokio::test]
async fn test_registry_new() {
    let registry = ToolCatalog::new();
    assert_eq!(registry.count().await, 0);
}

#[tokio::test]
async fn test_register_builtin_tools() {
    let registry = ToolCatalog::new();
    registry.register_builtin_tools().await;

    // Should register 8 curated multi-word commands (2 skill + groupchat +
    // session_new [aliases: new, clear] + cron + voice + goal + help). Media
    // generation (/image /video /audio /speech) and agent switching (/agent)
    // are surfaced via SHORTHAND_ALIASES seeding, NOT curated here.
    assert_eq!(registry.count().await, 8);

    let builtins = registry.list_builtin_tools().await;
    assert_eq!(builtins.len(), 8);

    // Verify tool names
    let names: Vec<_> = builtins.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"skill_read"));
    assert!(names.contains(&"skill_list"));
    assert!(names.contains(&"goal"));
    assert!(names.contains(&"help"));

    // Round-3 CUT: these dead curated commands (no execute_tool arm / no
    // backing) must NOT be registered — they only ever errored on invocation.
    assert!(!names.contains(&"generate_image"));
    assert!(!names.contains(&"generate_speech"));
    assert!(!names.contains(&"snapshot_capture"));
    assert!(!names.contains(&"switch"));
}

#[tokio::test]
async fn test_list_root_commands() {
    let registry = ToolCatalog::new();
    registry.register_builtin_tools().await;

    let rules = vec![RoutingRuleConfig {
        regex: "^/en".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Translate to English".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    let roots = registry.list_root_commands().await;
    // 8 curated builtins + 1 custom = 9 (aliases are not separate tools)
    assert_eq!(roots.len(), 9);

    // First should be builtins (sorted by priority)
    assert!(roots.iter().any(|t| t.name == "skill_list"));
    assert!(roots.iter().any(|t| t.name == "help"));
    assert!(roots.iter().any(|t| t.name == "en"));
}

#[tokio::test]
async fn test_register_skills() {
    let registry = ToolCatalog::new();

    let skills = vec![
        SkillInfo {
            id: "refine-text".to_string(),
            name: "Refine Text".to_string(),
            description: "Improve and polish writing".to_string(),
            ecosystem: "aleph".to_string(),
        },
        SkillInfo {
            id: "code-review".to_string(),
            name: "Code Review".to_string(),
            description: "Review code for issues".to_string(),
            ecosystem: "aleph".to_string(),
        },
    ];

    registry.register_skills(&skills).await;

    assert_eq!(registry.count().await, 2);

    let tool = registry.get_by_id("skill:refine-text").await;
    assert!(tool.is_some());
    let tool = tool.unwrap();
    assert!(matches!(tool.source, ToolSource::Skill { .. }));
}

#[tokio::test]
async fn test_register_custom_commands() {
    let registry = ToolCatalog::new();

    let rules = vec![
        RoutingRuleConfig {
            regex: "^/translate".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("You are a translator.".to_string()),
            ..Default::default()
        },
        RoutingRuleConfig {
            regex: "^/code".to_string(),
            provider: Some("claude".to_string()),
            system_prompt: Some("You are a code assistant.".to_string()),
            ..Default::default()
        },
        RoutingRuleConfig {
            regex: ".*".to_string(), // Catch-all, should not be registered
            provider: Some("openai".to_string()),
            system_prompt: None,
            ..Default::default()
        },
    ];

    registry.register_custom_commands(&rules).await;

    assert_eq!(registry.count().await, 2); // Only slash commands

    let translate = registry.get_by_name("translate").await;
    assert!(translate.is_some());
    assert!(matches!(
        translate.unwrap().source,
        ToolSource::Custom { rule_index: 0 }
    ));
}

#[tokio::test]
async fn test_search() {
    let registry = ToolCatalog::new();

    // Register a custom command to test search
    let rules = vec![RoutingRuleConfig {
        regex: "^/search".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Search assistant".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    let results = registry.search("search").await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "search");
}

#[tokio::test]
async fn test_set_tool_active() {
    let registry = ToolCatalog::new();

    // Register a custom command to test
    let rules = vec![RoutingRuleConfig {
        regex: "^/test".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Test assistant".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    // Deactivate test command (id is `custom:{rule_index}:{name}`)
    let updated = registry.set_tool_active("custom:0:test", false).await;
    assert!(updated);

    // Should not appear in active list
    let all = registry.list_all().await;
    assert!(!all.iter().any(|t| t.id == "custom:0:test"));

    // Should appear in full list
    let all_with_inactive = registry.list_all_with_inactive().await;
    assert!(all_with_inactive.iter().any(|t| t.id == "custom:0:test"));
}

// =========================================================================
// Conflict Resolution Tests
// =========================================================================

#[tokio::test]
async fn test_check_conflict_no_conflict() {
    let registry = ToolCatalog::new();

    // Register a custom command
    let rules = vec![RoutingRuleConfig {
        regex: "^/translate".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Translate".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    // No conflict for a new unique name
    let conflict = registry.check_conflict("git").await;
    assert!(conflict.is_none());
}

#[tokio::test]
async fn test_check_conflict_exists() {
    let registry = ToolCatalog::new();

    // Register a custom command
    let rules = vec![RoutingRuleConfig {
        regex: "^/search".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Search".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    // Conflict with custom "search"
    let conflict = registry.check_conflict("search").await;
    assert!(conflict.is_some());

    let info = conflict.unwrap();
    assert_eq!(info.existing_name, "search");
    assert_eq!(info.existing_priority, ToolPriority::Custom);
}

#[tokio::test]
async fn test_check_conflict_case_insensitive() {
    let registry = ToolCatalog::new();

    // Register a custom command
    let rules = vec![RoutingRuleConfig {
        regex: "^/search".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Search".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    // Should find conflict even with different case
    let conflict = registry.check_conflict("SEARCH").await;
    assert!(conflict.is_some());
    assert_eq!(conflict.unwrap().existing_name, "search");
}

#[test]
fn test_resolve_conflict_new_wins() {
    let registry = ToolCatalog::new();

    // MCP tool exists, Builtin tries to register
    let conflict = ConflictInfo {
        existing_id: "mcp:server:search".to_string(),
        existing_name: "search".to_string(),
        existing_source: ToolSource::Mcp {
            server: "server".into(),
        },
        existing_priority: ToolPriority::Mcp,
    };

    let resolution = registry.resolve_conflict("search", &conflict, &ToolSource::Builtin);

    // Builtin has higher priority, should rename existing
    match resolution {
        ConflictResolution::RenameExisting {
            original_name,
            new_name,
        } => {
            assert_eq!(original_name, "search");
            assert_eq!(new_name, "search-mcp");
        }
        _ => panic!("Expected RenameExisting"),
    }
}

#[test]
fn test_resolve_conflict_existing_wins() {
    let registry = ToolCatalog::new();

    // Builtin exists, MCP tries to register
    let conflict = ConflictInfo {
        existing_id: "builtin:search".to_string(),
        existing_name: "search".to_string(),
        existing_source: ToolSource::Builtin,
        existing_priority: ToolPriority::Builtin,
    };

    let resolution = registry.resolve_conflict(
        "search",
        &conflict,
        &ToolSource::Mcp {
            server: "server".into(),
        },
    );

    // Builtin has higher priority, should rename new
    match resolution {
        ConflictResolution::RenameNew {
            original_name,
            new_name,
        } => {
            assert_eq!(original_name, "search");
            assert_eq!(new_name, "search-mcp");
        }
        _ => panic!("Expected RenameNew"),
    }
}

#[test]
fn test_resolve_conflict_same_priority() {
    let registry = ToolCatalog::new();

    // Two MCP tools with same priority
    let conflict = ConflictInfo {
        existing_id: "mcp:server1:status".to_string(),
        existing_name: "status".to_string(),
        existing_source: ToolSource::Mcp {
            server: "server1".into(),
        },
        existing_priority: ToolPriority::Mcp,
    };

    let resolution = registry.resolve_conflict(
        "status",
        &conflict,
        &ToolSource::Mcp {
            server: "server2".into(),
        },
    );

    // Same priority - new tool gets renamed (first registered wins)
    match resolution {
        ConflictResolution::RenameNew {
            original_name,
            new_name,
        } => {
            assert_eq!(original_name, "status");
            assert_eq!(new_name, "status-mcp");
        }
        _ => panic!("Expected RenameNew"),
    }
}

#[tokio::test]
async fn test_register_with_conflict_resolution_no_conflict() {
    let registry = ToolCatalog::new();

    let tool = UnifiedTool::new(
        "mcp:server:git",
        "git",
        "Git operations",
        ToolSource::Mcp {
            server: "server".into(),
        },
    );

    let id = registry.register_with_conflict_resolution(tool).await;

    // No conflict, original ID used
    assert_eq!(id, "mcp:server:git");

    let registered = registry.get_by_id(&id).await;
    assert!(registered.is_some());
    assert_eq!(registered.unwrap().name, "git");
}

#[tokio::test]
async fn test_register_with_conflict_resolution_new_renamed() {
    let registry = ToolCatalog::new();

    // Register Custom tool first (higher priority than MCP)
    let custom_tool = UnifiedTool::new(
        "custom:search",
        "search",
        "Custom Search",
        ToolSource::Custom { rule_index: 0 },
    );
    registry
        .register_with_conflict_resolution(custom_tool)
        .await;

    // Try to register MCP tool with same name as custom
    let mcp_tool = UnifiedTool::new(
        "mcp:server:search",
        "search",
        "MCP Search",
        ToolSource::Mcp {
            server: "server".into(),
        },
    );

    let id = registry.register_with_conflict_resolution(mcp_tool).await;

    // MCP tool should be renamed (Custom has higher priority)
    assert_eq!(id, "mcp:server:search-mcp");

    let registered = registry.get_by_id(&id).await.unwrap();
    assert_eq!(registered.name, "search-mcp");
    assert_eq!(registered.original_name, Some("search".to_string()));
    assert!(registered.was_renamed);

    // Custom should still have original name
    let custom = registry.get_by_id("custom:search").await.unwrap();
    assert_eq!(custom.name, "search");
    assert!(!custom.was_renamed);
}

#[tokio::test]
async fn test_register_with_conflict_resolution_existing_renamed() {
    let registry = ToolCatalog::new();

    // Register MCP tool first
    let mcp_tool = UnifiedTool::new(
        "mcp:server:test",
        "test",
        "MCP Test",
        ToolSource::Mcp {
            server: "server".into(),
        },
    );
    registry.register_with_conflict_resolution(mcp_tool).await;

    // Register Custom tool with same name (higher priority)
    let custom_tool = UnifiedTool::new(
        "custom:test",
        "test",
        "Custom Test",
        ToolSource::Custom { rule_index: 0 },
    );
    let id = registry
        .register_with_conflict_resolution(custom_tool)
        .await;

    // Custom tool takes the name
    assert_eq!(id, "custom:test");
    let custom = registry.get_by_id(&id).await.unwrap();
    assert_eq!(custom.name, "test");
    assert!(!custom.was_renamed);

    // MCP tool should be renamed
    let mcp = registry.get_by_id("mcp:server:test-mcp").await;
    assert!(mcp.is_some());
    let mcp = mcp.unwrap();
    assert_eq!(mcp.name, "test-mcp");
    assert_eq!(mcp.original_name, Some("test".to_string()));
    assert!(mcp.was_renamed);
}

// =========================================================================
// Atomic Refresh Tests (Phase 3.4)
// =========================================================================

#[tokio::test]
async fn test_refresh_atomic_replaces_all_tools() {
    let registry = ToolCatalog::new();

    // Register some initial tools
    let rules = vec![RoutingRuleConfig {
        regex: "^/old".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Old command".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;
    let initial_count = registry.count().await;
    assert_eq!(initial_count, 1);

    // Create new tool list
    let new_tools = vec![
        UnifiedTool::new(
            "test:tool1",
            "tool1",
            "Test Tool 1",
            ToolSource::Custom { rule_index: 0 },
        ),
        UnifiedTool::new(
            "test:tool2",
            "tool2",
            "Test Tool 2",
            ToolSource::Custom { rule_index: 1 },
        ),
    ];

    // Atomic refresh should replace all tools
    registry.refresh_atomic(new_tools).await;

    // Should have exactly 2 tools now
    assert_eq!(registry.count().await, 2);

    // Old custom tools should be gone
    assert!(registry.get_by_id("custom:old").await.is_none());

    // New tools should exist
    assert!(registry.get_by_id("test:tool1").await.is_some());
    assert!(registry.get_by_id("test:tool2").await.is_some());
}

#[tokio::test]
async fn test_refresh_atomic_empty_list() {
    let registry = ToolCatalog::new();

    // Register some tools first
    let rules = vec![RoutingRuleConfig {
        regex: "^/test".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Test".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;
    assert!(registry.count().await > 0);

    // Refresh with empty list
    registry.refresh_atomic(vec![]).await;

    // Should have 0 tools
    assert_eq!(registry.count().await, 0);
}

#[tokio::test]
async fn test_refresh_atomic_preserves_tool_properties() {
    let registry = ToolCatalog::new();

    // Create tool with all properties
    let tool = UnifiedTool::new(
        "custom:mytool",
        "mytool",
        "My Tool Description",
        ToolSource::Custom { rule_index: 0 },
    )
    .with_display_name("My Tool")
    .with_icon("star.fill")
    .with_usage("/mytool [args]")
    .with_requires_confirmation(true);

    registry.refresh_atomic(vec![tool]).await;

    let retrieved = registry.get_by_id("custom:mytool").await.unwrap();
    assert_eq!(retrieved.name, "mytool");
    assert_eq!(retrieved.display_name, "My Tool");
    assert_eq!(retrieved.description, "My Tool Description");
    assert_eq!(retrieved.icon, Some("star.fill".to_string()));
    assert_eq!(retrieved.usage, Some("/mytool [args]".to_string()));
    assert!(retrieved.requires_confirmation);
}

// =========================================================================
// Channel Filtering Tests (Task 2)
// =========================================================================

#[tokio::test]
async fn test_list_for_channel_all_visible() {
    let registry = ToolCatalog::new();
    registry.register_builtin_tools().await;

    let panel_tools = registry.list_for_channel(ChannelType::Panel).await;
    let telegram_tools = registry.list_for_channel(ChannelType::Telegram).await;

    assert_eq!(panel_tools.len(), telegram_tools.len());
    assert!(!panel_tools.is_empty());
}

#[tokio::test]
async fn test_list_for_channel_filtered() {
    let registry = ToolCatalog::new();

    let tool = UnifiedTool::new(
        "custom:panel-only",
        "panel-only",
        "Panel only tool",
        ToolSource::Custom { rule_index: 0 },
    )
    .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);

    registry.register_with_conflict_resolution(tool).await;

    let panel_tools = registry.list_for_channel(ChannelType::Panel).await;
    assert_eq!(panel_tools.len(), 1);

    let telegram_tools = registry.list_for_channel(ChannelType::Telegram).await;
    assert_eq!(telegram_tools.len(), 0);
}

// =========================================================================
// Command Resolution Tests (Task 3)
// =========================================================================

#[tokio::test]
async fn test_resolve_command_found() {
    let registry = ToolCatalog::new();
    let rules = vec![RoutingRuleConfig {
        regex: "^/search".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Search the web".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    let resolved = registry.resolve_command("/search rust async").await;
    assert!(resolved.is_some());
    let resolved = resolved.unwrap();
    assert_eq!(resolved.tool.name, "search");
    assert_eq!(resolved.arguments, Some("rust async".to_string()));
}

#[tokio::test]
async fn test_resolve_command_not_found() {
    let registry = ToolCatalog::new();
    let resolved = registry.resolve_command("/nonexistent").await;
    assert!(resolved.is_none());
}

#[tokio::test]
async fn test_resolve_command_not_slash() {
    let registry = ToolCatalog::new();
    let resolved = registry.resolve_command("hello world").await;
    assert!(resolved.is_none());
}

#[tokio::test]
async fn test_resolve_command_case_insensitive() {
    let registry = ToolCatalog::new();
    let rules = vec![RoutingRuleConfig {
        regex: "^/search".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Search".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    let resolved = registry.resolve_command("/SEARCH query").await;
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().tool.name, "search");
}

#[tokio::test]
async fn test_resolve_command_no_args() {
    let registry = ToolCatalog::new();
    let rules = vec![RoutingRuleConfig {
        regex: "^/help".to_string(),
        provider: None,
        system_prompt: Some("Help".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    let resolved = registry.resolve_command("/help").await;
    assert!(resolved.is_some());
    assert!(resolved.unwrap().arguments.is_none());
}

#[tokio::test]
async fn test_resolve_command_strips_bot_mention() {
    let registry = ToolCatalog::new();
    let rules = vec![RoutingRuleConfig {
        regex: "^/search".to_string(),
        provider: Some("openai".to_string()),
        system_prompt: Some("Search".to_string()),
        ..Default::default()
    }];
    registry.register_custom_commands(&rules).await;

    // Telegram group format: /command@botname args
    let resolved = registry.resolve_command("/search@alephbot weather").await;
    assert!(resolved.is_some());
    let resolved = resolved.unwrap();
    assert_eq!(resolved.tool.name, "search");
    assert_eq!(resolved.arguments, Some("weather".to_string()));

    // No args variant
    let resolved = registry.resolve_command("/search@alephbot").await;
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().tool.name, "search");

    // Without @botname should still work
    let resolved = registry.resolve_command("/search query").await;
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().tool.name, "search");
}

// =========================================================================
// Prefix Filtering Tests (Task 4)
// =========================================================================

#[tokio::test]
async fn test_filter_by_prefix() {
    let registry = ToolCatalog::new();
    let rules = vec![
        RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search".to_string()),
            ..Default::default()
        },
        RoutingRuleConfig {
            regex: "^/settings".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Settings".to_string()),
            ..Default::default()
        },
        RoutingRuleConfig {
            regex: "^/translate".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Translate".to_string()),
            ..Default::default()
        },
    ];
    registry.register_custom_commands(&rules).await;

    let results = registry.filter_by_prefix("se").await;
    assert_eq!(results.len(), 2);

    let results = registry.filter_by_prefix("SE").await;
    assert_eq!(results.len(), 2);

    let results = registry.filter_by_prefix("").await;
    assert_eq!(results.len(), 3);

    let results = registry.filter_by_prefix("xyz").await;
    assert_eq!(results.len(), 0);
}

#[tokio::test]
async fn test_high_risk_tool_channel_restriction() {
    let registry = ToolCatalog::new();

    let tool = UnifiedTool::new(
        "mcp:server:delete_all",
        "delete_all",
        "Delete everything",
        ToolSource::Mcp {
            server: "server".into(),
        },
    )
    .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);

    registry.register_with_conflict_resolution(tool).await;

    let telegram = registry.list_for_channel(ChannelType::Telegram).await;
    assert!(!telegram.iter().any(|t| t.name == "delete_all"));

    let panel = registry.list_for_channel(ChannelType::Panel).await;
    assert!(panel.iter().any(|t| t.name == "delete_all"));
}

// =========================================================================
// Hierarchical Command Resolution Tests (Task 3 - Rewrite)
// =========================================================================

/// Helper: register a tool with a dotted name directly
async fn register_tool(registry: &ToolCatalog, id: &str, name: &str) {
    let tool = UnifiedTool::new(id, name, format!("Tool {}", name), ToolSource::Builtin);
    registry.register_with_conflict_resolution(tool).await;
}

#[tokio::test]
async fn test_resolve_command_hierarchical_two_level() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;

    // "/session new my-topic" → session.new, args = "my-topic"
    let resolved = registry.resolve_command("/session new my-topic").await;
    assert!(resolved.is_some());
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "session_new");
    assert_eq!(r.arguments, Some("my-topic".to_string()));
}

#[tokio::test]
async fn test_resolve_command_hierarchical_no_args() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;

    // "/session new" → session.new, no args
    let resolved = registry.resolve_command("/session new").await;
    assert!(resolved.is_some());
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "session_new");
    assert!(r.arguments.is_none());
}

#[tokio::test]
async fn test_resolve_command_flat_with_args() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "custom:search", "search").await;

    // "/search weather" → search, args = "weather"
    let resolved = registry.resolve_command("/search weather").await;
    assert!(resolved.is_some());
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "search");
    assert_eq!(r.arguments, Some("weather".to_string()));
}

#[tokio::test]
async fn test_resolve_command_new_alias() {
    // `/new` is now a first-class alias of `session_new` (no phantom tool).
    let registry = ToolCatalog::new();
    let tool = UnifiedTool::new(
        "builtin:session_new",
        "session_new",
        "Start a new session",
        ToolSource::Builtin,
    )
    .with_aliases(["new"]);
    registry.register_with_conflict_resolution(tool).await;

    // "/new topic" → resolves to canonical session_new via alias, args preserved.
    let resolved = registry.resolve_command("/new my topic").await;
    assert!(resolved.is_some());
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "session_new");
    assert_eq!(r.arguments, Some("my topic".to_string()));
}

#[tokio::test]
async fn test_resolve_alias_never_shadows_canonical_name() {
    // A canonical-name hit must always beat another tool's alias hit.
    let registry = ToolCatalog::new();
    // Tool A aliases "go".
    let a = UnifiedTool::new("custom:goto", "goto", "Go somewhere", ToolSource::Builtin)
        .with_aliases(["go"]);
    // Tool B is literally named "go".
    let b = UnifiedTool::new("custom:go", "go", "The real go", ToolSource::Builtin);
    registry.register_with_conflict_resolution(a).await;
    registry.register_with_conflict_resolution(b).await;

    let resolved = registry.resolve_command("/go").await.unwrap();
    assert_eq!(
        resolved.tool.name, "go",
        "canonical name must win over alias"
    );
}

#[tokio::test]
async fn test_unregister_canonical_restores_existing_alias() {
    let registry = ToolCatalog::new();
    let a = UnifiedTool::new("custom:goto", "goto", "Go somewhere", ToolSource::Builtin)
        .with_aliases(["go"]);
    let a_id = registry.register_with_conflict_resolution(a).await;
    let b = UnifiedTool::new(
        "custom:go",
        "go",
        "Dynamic go",
        ToolSource::Custom { rule_index: 0 },
    );
    let b_id = registry.register_with_conflict_resolution(b).await;

    let resolved = registry.resolve_command("/go").await.unwrap();
    assert_eq!(
        resolved.tool.name, "go",
        "canonical 'go' must beat the alias hit while active"
    );

    let goto = registry.get_by_id(&a_id).await.unwrap();
    assert!(
        goto.aliases.iter().any(|a| a == "go"),
        "goto's 'go' alias must be preserved when canonical 'go' registers"
    );

    assert!(registry.set_tool_active(&b_id, false).await);
    let resolved_after = registry.resolve_command("/go").await.unwrap();
    assert_eq!(
        resolved_after.tool.name, "goto",
        "after canonical 'go' is deactivated, 'go' alias must resolve back to goto"
    );
}

/// The same two tools must land the same way whichever order they register in.
///
/// Registration-time renaming made this asymmetric: registering the alias
/// holder first renamed the newcomer's canonical name, and registering it
/// second renamed the alias holder's — so a tool's own name depended on
/// startup ordering it does not control. Lookup-time tier ordering has no such
/// dependence, and this pins that.
#[tokio::test]
async fn registration_order_does_not_decide_a_canonical_vs_alias_collision() {
    let alias_holder = || {
        UnifiedTool::new("custom:goto", "goto", "Go somewhere", ToolSource::Builtin)
            .with_aliases(["go"])
    };
    let namesake = || UnifiedTool::new("custom:go", "go", "The real go", ToolSource::Builtin);

    for (label, first, second) in [
        ("alias holder first", alias_holder(), namesake()),
        ("namesake first", namesake(), alias_holder()),
    ] {
        let registry = ToolCatalog::new();
        registry.register_with_conflict_resolution(first).await;
        registry.register_with_conflict_resolution(second).await;

        let names: Vec<String> = registry
            .list_all()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(
            names.contains(&"go".to_string()) && names.contains(&"goto".to_string()),
            "{label}: neither tool may be renamed over a nickname collision, got {names:?}"
        );
        assert_eq!(
            registry.resolve_command("/go").await.unwrap().tool.name,
            "go",
            "{label}: the canonical name must win"
        );
    }
}

/// Two tools may claim the same alias. Neither is renamed; the lookup tier
/// picks by source priority, and the loser's alias is a live fallback rather
/// than a casualty.
#[tokio::test]
async fn two_tools_may_share_an_alias_and_the_loser_keeps_it_as_a_fallback() {
    let registry = ToolCatalog::new();
    let low = UnifiedTool::new(
        "skill:notes",
        "take_notes",
        "Take notes",
        ToolSource::Skill {
            id: "notes".to_string(),
        },
    )
    .with_aliases(["n"]);
    let high = UnifiedTool::new(
        "builtin:navigate",
        "navigate",
        "Navigate",
        ToolSource::Builtin,
    )
    .with_aliases(["n"]);
    let low_id = registry.register_with_conflict_resolution(low).await;
    let high_id = registry.register_with_conflict_resolution(high).await;

    for id in [&low_id, &high_id] {
        let tool = registry.get_by_id(id).await.unwrap();
        assert!(
            !tool.was_renamed,
            "sharing an alias must not rename '{}'",
            tool.name
        );
        assert!(tool.aliases.iter().any(|a| a == "n"));
    }

    assert_eq!(
        registry.resolve_command("/n").await.unwrap().tool.name,
        "navigate",
        "the higher-priority source wins the shared alias"
    );

    assert!(registry.set_tool_active(&high_id, false).await);
    assert_eq!(
        registry.resolve_command("/n").await.unwrap().tool.name,
        "take_notes",
        "deactivating the winner must hand the alias to the other claimant"
    );
}

#[tokio::test]
async fn test_suggest_commands_scores_name_and_alias() {
    let registry = ToolCatalog::new();
    let tool = UnifiedTool::new(
        "builtin:session_new",
        "session_new",
        "Start a new session",
        ToolSource::Builtin,
    )
    .with_aliases(["new"]);
    registry.register_with_conflict_resolution(tool).await;
    register_tool(&registry, "custom:search", "search").await;

    // Typo of the alias "new" surfaces the canonical command.
    let by_alias = registry.suggest_commands("/nwe", 3).await;
    assert!(by_alias.contains(&"session_new".to_string()));

    // Typo of a canonical name surfaces it too.
    let by_name = registry.suggest_commands("serch", 3).await;
    assert_eq!(by_name.first().map(String::as_str), Some("search"));

    // An exact match is never suggested (caller already resolves it).
    let exact = registry.suggest_commands("search", 3).await;
    assert!(!exact.contains(&"search".to_string()));
}

#[tokio::test]
async fn test_resolve_command_three_level() {
    let registry = ToolCatalog::new();
    register_tool(
        &registry,
        "builtin:plugin_marketplace_install",
        "plugin_marketplace_install",
    )
    .await;

    // "/plugin marketplace install x" → plugin_marketplace_install, args = "x"
    let resolved = registry
        .resolve_command("/plugin marketplace install x")
        .await;
    assert!(resolved.is_some());
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "plugin_marketplace_install");
    assert_eq!(r.arguments, Some("x".to_string()));
}

#[tokio::test]
async fn test_resolve_command_nonexistent() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;

    let resolved = registry.resolve_command("/nonexistent").await;
    assert!(resolved.is_none());
}

#[tokio::test]
async fn test_resolve_command_greedy_longest_match() {
    let registry = ToolCatalog::new();
    // Register both a namespace parent and a child
    register_tool(&registry, "builtin:session", "session").await;
    register_tool(&registry, "builtin:session_new", "session_new").await;

    // "/session new topic" should match session.new (longer), not session with args "new topic"
    let resolved = registry.resolve_command("/session new topic").await;
    assert!(resolved.is_some());
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "session_new");
    assert_eq!(r.arguments, Some("topic".to_string()));

    // "/session unknown" should fall back to session with args "unknown"
    let resolved = registry.resolve_command("/session unknown").await;
    assert!(resolved.is_some());
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "session");
    assert_eq!(r.arguments, Some("unknown".to_string()));
}

#[tokio::test]
async fn test_resolve_command_dot_separator_matches_underscore() {
    // A dotted single token (`/session.new`) must resolve to the same command
    // the execution layer (`LoopToolRegistry::resolve`) would run for the
    // underscore form. Without the separator-tolerant tier this silently fell
    // through to plain chat.
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;

    let resolved = registry.resolve_command("/session.new topic").await;
    assert!(resolved.is_some(), "dotted command should resolve");
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "session_new");
    assert_eq!(r.arguments, Some("topic".to_string()));
}

#[tokio::test]
async fn test_resolve_command_dot_separator_prefers_specific_over_namespace() {
    // With both `session` and `session_new` registered, `/session.new` must
    // pick the specific `session_new` (the normalized token), never the
    // namespace parent `session`.
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session", "session").await;
    register_tool(&registry, "builtin:session_new", "session_new").await;

    let resolved = registry.resolve_command("/session.new").await.unwrap();
    assert_eq!(resolved.tool.name, "session_new");
    assert!(resolved.arguments.is_none());
}

#[tokio::test]
async fn test_resolve_command_dot_separator_unknown_is_none() {
    // A dotted token that normalizes to no registered command must stay
    // unresolved (returned to the caller as plain input), not match a partial.
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;

    assert!(registry.resolve_command("/session.unknown").await.is_none());
}

#[tokio::test]
async fn test_resolve_command_dot_separator_via_alias() {
    // The separator-tolerant tier also covers aliases (symmetry with Tier 2).
    let registry = ToolCatalog::new();
    let tool = UnifiedTool::new(
        "builtin:agent_switch",
        "agent_switch",
        "Switch the active agent",
        ToolSource::Builtin,
    )
    .with_aliases(["go_agent"]);
    registry.register_with_conflict_resolution(tool).await;

    let resolved = registry.resolve_command("/go.agent now").await.unwrap();
    assert_eq!(resolved.tool.name, "agent_switch");
    assert_eq!(resolved.arguments, Some("now".to_string()));
}

#[tokio::test]
async fn test_resolve_command_bot_mention_hierarchical() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;

    // Telegram: "/session@mybot new topic"
    let resolved = registry.resolve_command("/session@mybot new topic").await;
    assert!(resolved.is_some());
    let r = resolved.unwrap();
    assert_eq!(r.tool.name, "session_new");
    assert_eq!(r.arguments, Some("topic".to_string()));
}

// =========================================================================
// Namespace Query Tests
// =========================================================================

#[tokio::test]
async fn test_is_namespace_true() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;
    register_tool(&registry, "builtin:session_list", "session_list").await;

    assert!(registry.is_namespace("session").await);
}

#[tokio::test]
async fn test_is_namespace_false() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "custom:search", "search").await;

    assert!(!registry.is_namespace("search").await);
}

#[tokio::test]
async fn test_is_namespace_case_insensitive() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;

    assert!(registry.is_namespace("Session").await);
    assert!(registry.is_namespace("SESSION").await);
}

#[tokio::test]
async fn test_list_namespace_children() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "builtin:session_new", "session_new").await;
    register_tool(&registry, "builtin:session_list", "session_list").await;
    register_tool(&registry, "builtin:session.topic.set", "session.topic.set").await; // deeper, should be excluded
    register_tool(&registry, "custom:search", "search").await; // unrelated

    let children = registry.list_namespace_children("session").await;
    assert_eq!(children.len(), 2);

    let names: Vec<&str> = children.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"session_new"));
    assert!(names.contains(&"session_list"));
    assert!(!names.contains(&"session.topic.set"));
}

#[tokio::test]
async fn test_list_namespace_children_empty() {
    let registry = ToolCatalog::new();
    register_tool(&registry, "custom:search", "search").await;

    let children = registry.list_namespace_children("search").await;
    assert!(children.is_empty());
}

// ── Slash-command surfacing (round: reference-driven /help + friendly aliases) ──

/// The curated builtins expose the cross-tool-standard shortcuts every
/// reference CLI (codex/openclaw/hermes/kimi) ships: `/skills`, `/new`,
/// `/clear`, plus the newly-surfaced `/goal` and `/help`. Each must resolve to
/// its canonical tool via the alias tier of `find_best_match`.
#[tokio::test]
async fn curated_builtins_expose_friendly_slash_aliases() {
    let registry = ToolCatalog::new();
    registry.register_builtin_tools().await;

    for (typed, expected_canonical) in [
        ("/skills", "skill_list"),
        ("/new", "session_new"),
        ("/clear", "session_new"),
        ("/goal", "goal"),
        ("/help", "help"),
    ] {
        let resolved = registry
            .resolve_command(typed)
            .await
            .unwrap_or_else(|| panic!("{typed} should resolve to a builtin command"));
        assert_eq!(
            resolved.tool.name, expected_canonical,
            "{typed} must resolve to {expected_canonical}"
        );
    }
}

/// The `ToolCatalog` builder seeds a bare executor tool's discoverable aliases
/// from the single `tool_metadata::aliases` source (this simulates the
/// `tool_catalog_init.rs` definitions loop). A shortcut seeded that way must be
/// both resolvable (`/model` → select_model) and surfaced as an alias so it
/// appears in completion menus and "did you mean?" suggestions.
#[tokio::test]
async fn seeded_shorthand_alias_resolves_and_is_discoverable() {
    use crate::tool_metadata::shorthand_aliases_for;

    let registry = ToolCatalog::new();
    let aliases = shorthand_aliases_for("select_model");
    assert_eq!(
        aliases,
        vec!["model"],
        "single source must map /model → select_model"
    );

    let tool = UnifiedTool::new(
        "builtin:select_model",
        "select_model",
        "Switch the active model for this session",
        ToolSource::Builtin,
    )
    .with_aliases(aliases);
    registry.register_with_conflict_resolution(tool).await;

    let resolved = registry
        .resolve_command("/model gpt-5")
        .await
        .expect("/model should resolve to select_model");
    assert_eq!(resolved.tool.name, "select_model");
    assert_eq!(resolved.arguments, Some("gpt-5".to_string()));

    // Discoverable: `/model` is scored/suggested from the alias, not the canonical.
    let suggestions = registry.suggest_commands("modl", 3).await;
    assert!(
        suggestions.iter().any(|s| s == "select_model"),
        "a near-miss of the alias must suggest the canonical command, got {suggestions:?}"
    );
}

/// Drift guard: every execution shorthand alias must point at a canonical name
/// that the catalog can resolve when a tool of that name is registered — i.e.
/// the alias table and the resolution layer agree. Catches a future edit that
/// adds a SHORTHAND row whose target no longer matches a real tool name.
#[tokio::test]
async fn shorthand_alias_execution_and_discovery_agree() {
    use crate::tool_metadata::{resolve_shorthand, SHORTHAND_ALIASES};

    for (alias, canonical) in SHORTHAND_ALIASES {
        // Execution layer maps the alias to this canonical name.
        assert_eq!(resolve_shorthand(alias), Some(*canonical));

        // Discovery layer: registering a tool named `canonical` with the alias
        // seeded must make the typed alias resolve back to it.
        let registry = ToolCatalog::new();
        let tool = UnifiedTool::new(
            format!("builtin:{canonical}"),
            *canonical,
            "probe",
            ToolSource::Builtin,
        )
        .with_aliases([*alias]);
        registry.register_with_conflict_resolution(tool).await;

        let resolved = registry
            .resolve_command(&format!("/{alias}"))
            .await
            .unwrap_or_else(|| panic!("/{alias} should resolve to {canonical}"));
        assert_eq!(resolved.tool.name, *canonical, "/{alias} → {canonical}");
    }
}
