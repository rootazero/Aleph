//! Tests for PluginRegistry

use super::*;
use crate::extension::registry::types::DiagnosticLevel;
use crate::extension::types::{PluginKind, PluginOrigin};
use std::path::PathBuf;

fn create_test_plugin(id: &str) -> PluginRecord {
    PluginRecord::new(
        id.to_string(),
        format!("Test Plugin {}", id),
        PluginKind::Static,
        PluginOrigin::Global,
    )
    .with_root_dir(PathBuf::from(format!("/plugins/{}", id)))
}

#[test]
fn test_registry_register_plugin() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("test-plugin");

    registry.register_plugin(plugin);

    assert!(registry.get_plugin("test-plugin").is_some());
    assert_eq!(registry.list_plugins().len(), 1);
}

#[test]
fn test_registry_disable_enable_plugin() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("test-plugin");

    registry.register_plugin(plugin);

    // Should be active initially
    assert_eq!(registry.list_active_plugins().len(), 1);

    // Disable
    assert!(registry.disable_plugin("test-plugin"));
    assert_eq!(registry.list_active_plugins().len(), 0);

    // Plugin still exists
    assert!(registry.get_plugin("test-plugin").is_some());

    // Enable
    assert!(registry.enable_plugin("test-plugin"));
    assert_eq!(registry.list_active_plugins().len(), 1);

    // Non-existent plugin
    assert!(!registry.disable_plugin("non-existent"));
    assert!(!registry.enable_plugin("non-existent"));
}

#[test]
fn test_registry_register_tool() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("tool-plugin");
    registry.register_plugin(plugin);

    let tool = ToolRegistration {
        name: "my_tool".to_string(),
        description: "A test tool".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        handler: "handle_my_tool".to_string(),
        plugin_id: "tool-plugin".to_string(),
    };

    registry.register_tool(tool);

    assert!(registry.get_tool("my_tool").is_some());
    assert_eq!(registry.list_tools().len(), 1);

    // Plugin record should be updated
    let plugin = registry.get_plugin("tool-plugin").unwrap();
    assert!(plugin.tool_names.contains(&"my_tool".to_string()));
}

#[test]
fn test_registry_hooks_sorted_by_priority() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("hook-plugin");
    registry.register_plugin(plugin);

    // Register hooks with different priorities
    let hook1 = HookRegistration {
        event: PluginHookEvent::BeforeToolCall,
        priority: 100,
        handler: "handler_low".to_string(),
        name: Some("Low Priority".to_string()),
        description: None,
        plugin_id: "hook-plugin".to_string(),
    };

    let hook2 = HookRegistration {
        event: PluginHookEvent::BeforeToolCall,
        priority: -50,
        handler: "handler_high".to_string(),
        name: Some("High Priority".to_string()),
        description: None,
        plugin_id: "hook-plugin".to_string(),
    };

    let hook3 = HookRegistration {
        event: PluginHookEvent::BeforeToolCall,
        priority: 0,
        handler: "handler_normal".to_string(),
        name: Some("Normal Priority".to_string()),
        description: None,
        plugin_id: "hook-plugin".to_string(),
    };

    // Register in non-priority order
    registry.register_hook(hook1);
    registry.register_hook(hook2);
    registry.register_hook(hook3);

    // Should be sorted by priority
    let hooks = registry.get_hooks_for_event(PluginHookEvent::BeforeToolCall);
    assert_eq!(hooks.len(), 3);
    assert_eq!(hooks[0].priority, -50); // High priority first
    assert_eq!(hooks[1].priority, 0);
    assert_eq!(hooks[2].priority, 100);

    // Plugin record should be updated
    let plugin = registry.get_plugin("hook-plugin").unwrap();
    assert_eq!(plugin.hook_count, 3);
}

#[test]
fn test_registry_hooks_filter_by_event() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("hook-plugin");
    registry.register_plugin(plugin);

    let hook1 = HookRegistration {
        event: PluginHookEvent::BeforeToolCall,
        priority: 0,
        handler: "before_tool".to_string(),
        name: None,
        description: None,
        plugin_id: "hook-plugin".to_string(),
    };

    let hook2 = HookRegistration {
        event: PluginHookEvent::AfterToolCall,
        priority: 0,
        handler: "after_tool".to_string(),
        name: None,
        description: None,
        plugin_id: "hook-plugin".to_string(),
    };

    registry.register_hook(hook1);
    registry.register_hook(hook2);

    assert_eq!(
        registry
            .get_hooks_for_event(PluginHookEvent::BeforeToolCall)
            .len(),
        1
    );
    assert_eq!(
        registry
            .get_hooks_for_event(PluginHookEvent::AfterToolCall)
            .len(),
        1
    );
    assert_eq!(
        registry
            .get_hooks_for_event(PluginHookEvent::SessionStart)
            .len(),
        0
    );
}

#[test]
fn test_registry_channel_registration() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("channel-plugin");
    registry.register_plugin(plugin);

    let channel = ChannelRegistration {
        id: "telegram".to_string(),
        label: "Telegram".to_string(),
        docs_path: Some("/docs/telegram.md".to_string()),
        blurb: Some("Telegram Bot".to_string()),
        system_image: None,
        aliases: vec!["tg".to_string()],
        order: 1,
        plugin_id: "channel-plugin".to_string(),
    };

    registry.register_channel(channel);

    assert!(registry.get_channel("telegram").is_some());
    assert_eq!(registry.list_channels().len(), 1);

    let plugin = registry.get_plugin("channel-plugin").unwrap();
    assert!(plugin.channel_ids.contains(&"telegram".to_string()));
}

#[test]
fn test_registry_provider_registration() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("provider-plugin");
    registry.register_plugin(plugin);

    let provider = ProviderRegistration {
        id: "anthropic".to_string(),
        name: "Anthropic".to_string(),
        models: vec!["claude-opus-4-5".to_string()],
        plugin_id: "provider-plugin".to_string(),
    };

    registry.register_provider(provider);

    assert!(registry.get_provider("anthropic").is_some());
    assert_eq!(registry.list_providers().len(), 1);

    let plugin = registry.get_plugin("provider-plugin").unwrap();
    assert!(plugin.provider_ids.contains(&"anthropic".to_string()));
}

#[test]
fn test_registry_gateway_method_registration() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("gateway-plugin");
    registry.register_plugin(plugin);

    let method = GatewayMethodRegistration {
        method: "myplugin.execute".to_string(),
        description: Some("Execute an action".to_string()),
        handler: "handle_execute".to_string(),
        plugin_id: "gateway-plugin".to_string(),
    };

    registry.register_gateway_method(method);

    assert!(registry.get_gateway_method("myplugin.execute").is_some());
    assert_eq!(registry.list_gateway_methods().len(), 1);

    let plugin = registry.get_plugin("gateway-plugin").unwrap();
    assert!(plugin.gateway_methods.contains(&"myplugin.execute".to_string()));
}

#[test]
fn test_registry_http_routes_and_handlers() {
    let mut registry = PluginRegistry::new();

    let route = HttpRouteRegistration {
        path: "/api/v1/webhook".to_string(),
        methods: vec!["POST".to_string()],
        handler: "handle_webhook".to_string(),
        plugin_id: "http-plugin".to_string(),
    };

    let handler1 = HttpHandlerRegistration {
        handler: "auth_middleware".to_string(),
        priority: -100,
        plugin_id: "http-plugin".to_string(),
    };

    let handler2 = HttpHandlerRegistration {
        handler: "logging_middleware".to_string(),
        priority: 100,
        plugin_id: "http-plugin".to_string(),
    };

    registry.register_http_route(route);
    registry.register_http_handler(handler2);
    registry.register_http_handler(handler1);

    assert_eq!(registry.list_http_routes().len(), 1);
    assert_eq!(registry.find_http_routes("/api/v1/webhook").len(), 1);

    let handlers = registry.list_http_handlers();
    assert_eq!(handlers.len(), 2);
    assert_eq!(handlers[0].priority, -100); // Auth first
    assert_eq!(handlers[1].priority, 100); // Logging second
}

#[test]
fn test_registry_cli_commands() {
    let mut registry = PluginRegistry::new();

    let cli = CliRegistration {
        name: "sync".to_string(),
        description: "Sync data".to_string(),
        handler: "handle_sync".to_string(),
        plugin_id: "cli-plugin".to_string(),
    };

    registry.register_cli_command(cli);

    assert!(registry.get_cli_command("sync").is_some());
    assert_eq!(registry.list_cli_commands().len(), 1);
}

#[test]
fn test_registry_services() {
    let mut registry = PluginRegistry::new();
    let plugin = create_test_plugin("service-plugin");
    registry.register_plugin(plugin);

    let service = ServiceRegistration {
        id: "background-worker".to_string(),
        name: "Background Worker".to_string(),
        start_handler: "start_worker".to_string(),
        stop_handler: "stop_worker".to_string(),
        plugin_id: "service-plugin".to_string(),
    };

    registry.register_service(service);

    assert!(registry.get_service("background-worker").is_some());
    assert_eq!(registry.list_services().len(), 1);

    let plugin = registry.get_plugin("service-plugin").unwrap();
    assert!(plugin.service_ids.contains(&"background-worker".to_string()));
}

#[test]
fn test_registry_commands() {
    let mut registry = PluginRegistry::new();

    let command = CommandRegistration {
        name: "remind".to_string(),
        description: "Set a reminder".to_string(),
        handler: "handle_remind".to_string(),
        plugin_id: "command-plugin".to_string(),
    };

    registry.register_command(command);

    assert!(registry.get_command("remind").is_some());
    assert_eq!(registry.list_commands().len(), 1);
}

#[test]
fn test_registry_diagnostics() {
    let mut registry = PluginRegistry::new();

    let diag1 = PluginDiagnostic {
        level: DiagnosticLevel::Warn,
        message: "Minor issue".to_string(),
        plugin_id: Some("plugin-a".to_string()),
        source: None,
    };

    let diag2 = PluginDiagnostic {
        level: DiagnosticLevel::Error,
        message: "Critical error".to_string(),
        plugin_id: Some("plugin-b".to_string()),
        source: Some("init".to_string()),
    };

    registry.add_diagnostic(diag1);
    registry.add_diagnostic(diag2);

    assert_eq!(registry.diagnostics().len(), 2);

    registry.clear_diagnostics();
    assert_eq!(registry.diagnostics().len(), 0);
}

#[test]
fn test_registry_unregister_plugin() {
    let mut registry = PluginRegistry::new();

    // Create and register a plugin with various components
    let plugin = create_test_plugin("full-plugin");
    registry.register_plugin(plugin);

    let tool = ToolRegistration {
        name: "plugin_tool".to_string(),
        description: "Tool".to_string(),
        parameters: serde_json::json!({}),
        handler: "handle".to_string(),
        plugin_id: "full-plugin".to_string(),
    };
    registry.register_tool(tool);

    let hook = HookRegistration {
        event: PluginHookEvent::BeforeAgentStart,
        priority: 0,
        handler: "hook".to_string(),
        name: None,
        description: None,
        plugin_id: "full-plugin".to_string(),
    };
    registry.register_hook(hook);

    let channel = ChannelRegistration {
        id: "test-channel".to_string(),
        label: "Test".to_string(),
        docs_path: None,
        blurb: None,
        system_image: None,
        aliases: vec![],
        order: 1,
        plugin_id: "full-plugin".to_string(),
    };
    registry.register_channel(channel);

    let diag = PluginDiagnostic {
        level: DiagnosticLevel::Warn,
        message: "Warning".to_string(),
        plugin_id: Some("full-plugin".to_string()),
        source: None,
    };
    registry.add_diagnostic(diag);

    // Verify components exist
    assert!(registry.get_plugin("full-plugin").is_some());
    assert!(registry.get_tool("plugin_tool").is_some());
    assert_eq!(
        registry
            .get_hooks_for_event(PluginHookEvent::BeforeAgentStart)
            .len(),
        1
    );
    assert!(registry.get_channel("test-channel").is_some());
    assert_eq!(registry.diagnostics().len(), 1);

    // Unregister the plugin
    registry.unregister_plugin("full-plugin");

    // Verify everything is removed
    assert!(registry.get_plugin("full-plugin").is_none());
    assert!(registry.get_tool("plugin_tool").is_none());
    assert_eq!(
        registry
            .get_hooks_for_event(PluginHookEvent::BeforeAgentStart)
            .len(),
        0
    );
    assert!(registry.get_channel("test-channel").is_none());
    assert_eq!(registry.diagnostics().len(), 0);
}

#[test]
fn test_registry_clear() {
    let mut registry = PluginRegistry::new();

    // Add various items
    registry.register_plugin(create_test_plugin("plugin-1"));
    registry.register_tool(ToolRegistration {
        name: "tool".to_string(),
        description: "Desc".to_string(),
        parameters: serde_json::json!({}),
        handler: "h".to_string(),
        plugin_id: "plugin-1".to_string(),
    });

    assert!(!registry.list_plugins().is_empty());

    registry.clear();

    assert!(registry.list_plugins().is_empty());
    assert!(registry.list_tools().is_empty());
    assert!(registry.list_hooks().is_empty());
    assert!(registry.list_channels().is_empty());
    assert!(registry.list_providers().is_empty());
    assert!(registry.list_gateway_methods().is_empty());
    assert!(registry.list_http_routes().is_empty());
    assert!(registry.list_http_handlers().is_empty());
    assert!(registry.list_cli_commands().is_empty());
    assert!(registry.list_services().is_empty());
    assert!(registry.list_commands().is_empty());
    assert!(registry.diagnostics().is_empty());
}

#[test]
fn test_registry_stats() {
    let mut registry = PluginRegistry::new();

    registry.register_plugin(create_test_plugin("plugin-1"));
    registry.register_tool(ToolRegistration {
        name: "tool".to_string(),
        description: "Desc".to_string(),
        parameters: serde_json::json!({}),
        handler: "h".to_string(),
        plugin_id: "plugin-1".to_string(),
    });

    let stats = registry.stats();

    assert_eq!(stats.plugins, 1);
    assert_eq!(stats.active_plugins, 1);
    assert_eq!(stats.tools, 1);
    assert_eq!(stats.hooks, 0);
}

#[test]
fn test_list_tools_for_plugin() {
    let mut registry = PluginRegistry::new();

    registry.register_plugin(create_test_plugin("plugin-a"));
    registry.register_plugin(create_test_plugin("plugin-b"));

    registry.register_tool(ToolRegistration {
        name: "tool_a1".to_string(),
        description: "Desc".to_string(),
        parameters: serde_json::json!({}),
        handler: "h".to_string(),
        plugin_id: "plugin-a".to_string(),
    });

    registry.register_tool(ToolRegistration {
        name: "tool_a2".to_string(),
        description: "Desc".to_string(),
        parameters: serde_json::json!({}),
        handler: "h".to_string(),
        plugin_id: "plugin-a".to_string(),
    });

    registry.register_tool(ToolRegistration {
        name: "tool_b1".to_string(),
        description: "Desc".to_string(),
        parameters: serde_json::json!({}),
        handler: "h".to_string(),
        plugin_id: "plugin-b".to_string(),
    });

    assert_eq!(registry.list_tools_for_plugin("plugin-a").len(), 2);
    assert_eq!(registry.list_tools_for_plugin("plugin-b").len(), 1);
    assert_eq!(registry.list_tools_for_plugin("plugin-c").len(), 0);
}

#[test]
fn test_channels_sorted_by_order() {
    let mut registry = PluginRegistry::new();

    let channel1 = ChannelRegistration {
        id: "channel-c".to_string(),
        label: "C".to_string(),
        docs_path: None,
        blurb: None,
        system_image: None,
        aliases: vec![],
        order: 30,
        plugin_id: "p".to_string(),
    };

    let channel2 = ChannelRegistration {
        id: "channel-a".to_string(),
        label: "A".to_string(),
        docs_path: None,
        blurb: None,
        system_image: None,
        aliases: vec![],
        order: 10,
        plugin_id: "p".to_string(),
    };

    let channel3 = ChannelRegistration {
        id: "channel-b".to_string(),
        label: "B".to_string(),
        docs_path: None,
        blurb: None,
        system_image: None,
        aliases: vec![],
        order: 20,
        plugin_id: "p".to_string(),
    };

    registry.register_channel(channel1);
    registry.register_channel(channel2);
    registry.register_channel(channel3);

    let channels = registry.list_channels();
    assert_eq!(channels[0].id, "channel-a");
    assert_eq!(channels[1].id, "channel-b");
    assert_eq!(channels[2].id, "channel-c");
}
