//! Step definitions for Extension Plugin Registry features

use cucumber::{given, then, when};
use std::path::PathBuf;

use crate::world::{AlephWorld, ExtensionContext};
use alephcore::extension::{
    ChannelRegistration, CliRegistration, CommandRegistration, DiagnosticLevel,
    GatewayMethodRegistration, HookRegistration, HttpHandlerRegistration, HttpRouteRegistration,
    PluginDiagnostic, PluginHookEvent, PluginKind, PluginOrigin, PluginRecord, PluginRegistry,
    ProviderRegistration, ServiceRegistration, ToolRegistration,
};

// =============================================================================
// Helper Functions
// =============================================================================

/// Create a test plugin with standard settings
fn create_test_plugin(id: &str) -> PluginRecord {
    PluginRecord::new(
        id.to_string(),
        format!("Test Plugin {}", id),
        PluginKind::Static,
        PluginOrigin::Global,
    )
    .with_root_dir(PathBuf::from(format!("/plugins/{}", id)))
}

/// Parse hook event from string
fn parse_hook_event(event: &str) -> PluginHookEvent {
    match event {
        "BeforeAgentStart" => PluginHookEvent::BeforeAgentStart,
        "AgentEnd" => PluginHookEvent::AgentEnd,
        "BeforeToolCall" => PluginHookEvent::BeforeToolCall,
        "AfterToolCall" => PluginHookEvent::AfterToolCall,
        "ToolResultPersist" => PluginHookEvent::ToolResultPersist,
        "MessageReceived" => PluginHookEvent::MessageReceived,
        "MessageSending" => PluginHookEvent::MessageSending,
        "MessageSent" => PluginHookEvent::MessageSent,
        "SessionStart" => PluginHookEvent::SessionStart,
        "SessionEnd" => PluginHookEvent::SessionEnd,
        "BeforeCompaction" => PluginHookEvent::BeforeCompaction,
        "AfterCompaction" => PluginHookEvent::AfterCompaction,
        "GatewayStart" => PluginHookEvent::GatewayStart,
        "GatewayStop" => PluginHookEvent::GatewayStop,
        _ => panic!("Unknown hook event: {}", event),
    }
}

/// Ensure registry exists
fn ensure_registry(ctx: &mut ExtensionContext) -> &mut PluginRegistry {
    if ctx.registry.is_none() {
        ctx.registry = Some(PluginRegistry::new());
    }
    ctx.registry.as_mut().unwrap()
}

// =============================================================================
// Given Steps - Registry Setup
// =============================================================================

#[given("a new plugin registry")]
async fn given_new_plugin_registry(w: &mut AlephWorld) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    ctx.registry = Some(PluginRegistry::new());
    ctx.last_op_success = true;
}

// Compound step: "a test plugin X" + "I register the plugin"
#[given(expr = "a test plugin {string}")]
async fn given_test_plugin(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let plugin = create_test_plugin(&plugin_id);
    registry.register_plugin(plugin);
}

#[given("I register the plugin")]
async fn given_register_plugin(_w: &mut AlephWorld) {
    // No-op: plugin already registered in the "a test plugin" step
}

// Compound step: "a tool X for plugin Y" + "I register the tool"
#[given(expr = "a tool {string} for plugin {string}")]
async fn given_tool_for_plugin(w: &mut AlephWorld, tool_name: String, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let tool = ToolRegistration {
        name: tool_name,
        description: "Test tool".to_string(),
        parameters: serde_json::json!({}),
        handler: "handle".to_string(),
        plugin_id,
    };
    registry.register_tool(tool);
}

#[given("I register the tool")]
async fn given_register_tool(_w: &mut AlephWorld) {
    // No-op: tool already registered
}

// Compound step: "a hook X with priority Y for plugin Z" + "I register the hook"
#[given(expr = "a hook {string} with priority {int} for plugin {string}")]
async fn given_hook_for_plugin(w: &mut AlephWorld, event: String, priority: i32, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let hook = HookRegistration {
        event: parse_hook_event(&event),
        priority,
        handler: format!("handler_{}", priority),
        name: Some(format!("Hook {}", priority)),
        description: None,
        plugin_id,
    };
    registry.register_hook(hook);
}

#[given("I register the hook")]
async fn given_register_hook(_w: &mut AlephWorld) {
    // No-op: hook already registered
}

// Compound step: "a channel X for plugin Y" + "I register the channel"
#[given(expr = "a channel {string} for plugin {string}")]
async fn given_channel_for_plugin(w: &mut AlephWorld, channel_id: String, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let channel = ChannelRegistration {
        id: channel_id,
        label: "Test".to_string(),
        docs_path: None,
        blurb: None,
        system_image: None,
        aliases: vec![],
        order: 1,
        plugin_id,
    };
    registry.register_channel(channel);
}

#[given(expr = "a channel {string} with alias {string} for plugin {string}")]
async fn given_channel_with_alias(
    w: &mut AlephWorld,
    channel_id: String,
    alias: String,
    plugin_id: String,
) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let channel = ChannelRegistration {
        id: channel_id.clone(),
        label: channel_id,
        docs_path: Some("/docs/channel.md".to_string()),
        blurb: Some("Test channel".to_string()),
        system_image: None,
        aliases: vec![alias],
        order: 1,
        plugin_id,
    };
    registry.register_channel(channel);
}

#[given(expr = "a channel {string} with order {int} for plugin {string}")]
async fn given_channel_with_order(
    w: &mut AlephWorld,
    channel_id: String,
    order: i32,
    plugin_id: String,
) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let channel = ChannelRegistration {
        id: channel_id.clone(),
        label: channel_id,
        docs_path: None,
        blurb: None,
        system_image: None,
        aliases: vec![],
        order,
        plugin_id,
    };
    registry.register_channel(channel);
}

#[given("I register the channel")]
async fn given_register_channel(_w: &mut AlephWorld) {
    // No-op: channel already registered
}

// Compound step: "a diagnostic with level X for plugin Y" + "I add the diagnostic"
#[given(expr = "a diagnostic with level {string} for plugin {string}")]
async fn given_diagnostic_for_plugin(w: &mut AlephWorld, level: String, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let diagnostic = PluginDiagnostic {
        level: match level.as_str() {
            "warn" => DiagnosticLevel::Warn,
            "error" => DiagnosticLevel::Error,
            _ => panic!("Unknown diagnostic level: {}", level),
        },
        message: format!("{} message", level),
        plugin_id: Some(plugin_id),
        source: None,
    };
    registry.add_diagnostic(diagnostic);
}

#[given("I add the diagnostic")]
async fn given_add_diagnostic(_w: &mut AlephWorld) {
    // No-op: diagnostic already added
}

// Compound step: "a provider X with model Y for plugin Z" + "I register the provider"
#[given(expr = "a provider {string} with model {string} for plugin {string}")]
async fn given_provider_for_plugin(
    w: &mut AlephWorld,
    provider_id: String,
    model: String,
    plugin_id: String,
) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let provider = ProviderRegistration {
        id: provider_id.clone(),
        name: provider_id,
        models: vec![model],
        plugin_id,
    };
    registry.register_provider(provider);
}

#[given("I register the provider")]
async fn given_register_provider(_w: &mut AlephWorld) {
    // No-op: provider already registered
}

// Compound step: "a gateway method X for plugin Y" + "I register the gateway method"
#[given(expr = "a gateway method {string} for plugin {string}")]
async fn given_gateway_method(w: &mut AlephWorld, method: String, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let gw_method = GatewayMethodRegistration {
        method,
        description: Some("Test method".to_string()),
        handler: "handle_method".to_string(),
        plugin_id,
    };
    registry.register_gateway_method(gw_method);
}

#[given("I register the gateway method")]
async fn given_register_gateway_method(_w: &mut AlephWorld) {
    // No-op: method already registered
}

// Compound step: "an http route X with methods Y for plugin Z" + "I register the http route"
#[given(expr = "an http route {string} with methods {string} for plugin {string}")]
async fn given_http_route(w: &mut AlephWorld, path: String, methods: String, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let route = HttpRouteRegistration {
        path,
        methods: methods.split(',').map(|s| s.trim().to_string()).collect(),
        handler: "handle_route".to_string(),
        plugin_id,
    };
    registry.register_http_route(route);
}

#[given("I register the http route")]
async fn given_register_http_route(_w: &mut AlephWorld) {
    // No-op: route already registered
}

// Compound step: "an http handler X with priority Y for plugin Z" + "I register the http handler"
#[given(expr = "an http handler {string} with priority {int} for plugin {string}")]
async fn given_http_handler(w: &mut AlephWorld, handler: String, priority: i32, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let http_handler = HttpHandlerRegistration {
        handler,
        priority,
        plugin_id,
    };
    registry.register_http_handler(http_handler);
}

#[given("I register the http handler")]
async fn given_register_http_handler(_w: &mut AlephWorld) {
    // No-op: handler already registered
}

// Compound step: "a cli command X for plugin Y" + "I register the cli command"
#[given(expr = "a cli command {string} for plugin {string}")]
async fn given_cli_command(w: &mut AlephWorld, name: String, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let cli = CliRegistration {
        name,
        description: "Test CLI command".to_string(),
        handler: "handle_cli".to_string(),
        plugin_id,
    };
    registry.register_cli_command(cli);
}

#[given("I register the cli command")]
async fn given_register_cli_command(_w: &mut AlephWorld) {
    // No-op: CLI command already registered
}

// Compound step: "a service X for plugin Y" + "I register the service"
#[given(expr = "a service {string} for plugin {string}")]
async fn given_service(w: &mut AlephWorld, service_id: String, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let service = ServiceRegistration {
        id: service_id.clone(),
        name: service_id,
        start_handler: "start_service".to_string(),
        stop_handler: "stop_service".to_string(),
        plugin_id,
    };
    registry.register_service(service);
}

#[given("I register the service")]
async fn given_register_service(_w: &mut AlephWorld) {
    // No-op: service already registered
}

// Compound step: "an in-chat command X for plugin Y" + "I register the in-chat command"
#[given(expr = "an in-chat command {string} for plugin {string}")]
async fn given_inchat_command(w: &mut AlephWorld, name: String, plugin_id: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let registry = ensure_registry(ctx);
    let command = CommandRegistration {
        name,
        description: "Test command".to_string(),
        handler: "handle_command".to_string(),
        plugin_id,
    };
    registry.register_command(command);
}

#[given("I register the in-chat command")]
async fn given_register_inchat_command(_w: &mut AlephWorld) {
    // No-op: command already registered
}

// =============================================================================
// When Steps - Registry Operations
// =============================================================================

#[when("I register the plugin")]
async fn when_register_plugin(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the tool")]
async fn when_register_tool(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the hook")]
async fn when_register_hook(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the channel")]
async fn when_register_channel(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the provider")]
async fn when_register_provider(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the gateway method")]
async fn when_register_gateway_method(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the http route")]
async fn when_register_http_route(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the http handler")]
async fn when_register_http_handler(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the cli command")]
async fn when_register_cli_command(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the service")]
async fn when_register_service(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when("I register the in-chat command")]
async fn when_register_inchat_command(_w: &mut AlephWorld) {
    // No-op: handled by given step
}

#[when(expr = "I disable the plugin {string}")]
async fn when_disable_plugin(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    ctx.last_op_success = registry.disable_plugin(&plugin_id);
}

#[when(expr = "I enable the plugin {string}")]
async fn when_enable_plugin(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    ctx.last_op_success = registry.enable_plugin(&plugin_id);
}

#[when(expr = "I disable a non-existent plugin {string}")]
async fn when_disable_nonexistent_plugin(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    ctx.last_op_success = registry.disable_plugin(&plugin_id);
}

#[when(expr = "I enable a non-existent plugin {string}")]
async fn when_enable_nonexistent_plugin(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    ctx.last_op_success = registry.enable_plugin(&plugin_id);
}

#[when(expr = "I unregister the plugin {string}")]
async fn when_unregister_plugin(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    registry.unregister_plugin(&plugin_id);
}

#[when("I clear the registry")]
async fn when_clear_registry(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    registry.clear();
}

#[when("I clear the diagnostics")]
async fn when_clear_diagnostics(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let registry = ctx.registry.as_mut().expect("Registry not initialized");
    registry.clear_diagnostics();
}

#[when("I get the registry stats")]
async fn when_get_stats(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    ctx.stats = Some(registry.stats());
}

// =============================================================================
// Then Steps - Assertions
// =============================================================================

#[then(expr = "the plugin {string} should exist")]
async fn then_plugin_should_exist(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_plugin(&plugin_id).is_some(),
        "Plugin '{}' should exist",
        plugin_id
    );
}

#[then(expr = "the plugin {string} should not exist")]
async fn then_plugin_should_not_exist(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_plugin(&plugin_id).is_none(),
        "Plugin '{}' should not exist",
        plugin_id
    );
}

#[then(expr = "the plugin count should be {int}")]
async fn then_plugin_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_plugins().len();
    assert_eq!(count, expected, "Expected {} plugins, got {}", expected, count);
}

#[then(expr = "the active plugin count should be {int}")]
async fn then_active_plugin_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_active_plugins().len();
    assert_eq!(
        count, expected,
        "Expected {} active plugins, got {}",
        expected, count
    );
}

#[then("the last operation should have failed")]
async fn then_operation_should_fail(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    assert!(!ctx.last_op_success, "Expected operation to fail");
}

#[then(expr = "the tool {string} should exist")]
async fn then_tool_should_exist(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_tool(&tool_name).is_some(),
        "Tool '{}' should exist",
        tool_name
    );
}

#[then(expr = "the tool {string} should not exist")]
async fn then_tool_should_not_exist(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_tool(&tool_name).is_none(),
        "Tool '{}' should not exist",
        tool_name
    );
}

#[then(expr = "the tool count should be {int}")]
async fn then_tool_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_tools().len();
    assert_eq!(count, expected, "Expected {} tools, got {}", expected, count);
}

#[then(expr = "the plugin {string} should have tool {string}")]
async fn then_plugin_has_tool(w: &mut AlephWorld, plugin_id: String, tool_name: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let plugin = registry.get_plugin(&plugin_id).expect("Plugin not found");
    assert!(
        plugin.tool_names.contains(&tool_name),
        "Plugin '{}' should have tool '{}'",
        plugin_id,
        tool_name
    );
}

#[then(expr = "the tools for plugin {string} should be {int}")]
async fn then_tools_for_plugin(w: &mut AlephWorld, plugin_id: String, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_tools_for_plugin(&plugin_id).len();
    assert_eq!(
        count, expected,
        "Expected {} tools for plugin '{}', got {}",
        expected, plugin_id, count
    );
}

#[then(expr = "the hook count for event {string} should be {int}")]
async fn then_hook_count_for_event(w: &mut AlephWorld, event: String, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let event = parse_hook_event(&event);
    let count = registry.get_hooks_for_event(event).len();
    assert_eq!(
        count, expected,
        "Expected {} hooks for event, got {}",
        expected, count
    );
}

#[then(expr = "the hook count should be {int}")]
async fn then_hook_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_hooks().len();
    assert_eq!(count, expected, "Expected {} hooks, got {}", expected, count);
}

#[then(expr = "the hooks for event {string} should be sorted by priority as {string}")]
async fn then_hooks_sorted_by_priority(w: &mut AlephWorld, event: String, priorities: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let event = parse_hook_event(&event);
    let hooks = registry.get_hooks_for_event(event);
    let actual_priorities: Vec<i32> = hooks.iter().map(|h| h.priority).collect();
    let expected_priorities: Vec<i32> = priorities
        .split(',')
        .map(|s| s.parse().expect("Invalid priority"))
        .collect();
    assert_eq!(
        actual_priorities, expected_priorities,
        "Hook priorities mismatch"
    );
}

#[then(expr = "the plugin {string} hook count should be {int}")]
async fn then_plugin_hook_count(w: &mut AlephWorld, plugin_id: String, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let plugin = registry.get_plugin(&plugin_id).expect("Plugin not found");
    assert_eq!(
        plugin.hook_count, expected,
        "Expected {} hooks for plugin '{}', got {}",
        expected, plugin_id, plugin.hook_count
    );
}

#[then(expr = "the channel {string} should exist")]
async fn then_channel_should_exist(w: &mut AlephWorld, channel_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_channel(&channel_id).is_some(),
        "Channel '{}' should exist",
        channel_id
    );
}

#[then(expr = "the channel {string} should not exist")]
async fn then_channel_should_not_exist(w: &mut AlephWorld, channel_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_channel(&channel_id).is_none(),
        "Channel '{}' should not exist",
        channel_id
    );
}

#[then(expr = "the channel count should be {int}")]
async fn then_channel_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_channels().len();
    assert_eq!(
        count, expected,
        "Expected {} channels, got {}",
        expected, count
    );
}

#[then(expr = "the plugin {string} should have channel {string}")]
async fn then_plugin_has_channel(w: &mut AlephWorld, plugin_id: String, channel_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let plugin = registry.get_plugin(&plugin_id).expect("Plugin not found");
    assert!(
        plugin.channel_ids.contains(&channel_id),
        "Plugin '{}' should have channel '{}'",
        plugin_id,
        channel_id
    );
}

#[then(expr = "the channels should be sorted by order as {string}")]
async fn then_channels_sorted_by_order(w: &mut AlephWorld, expected_order: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let channels = registry.list_channels();
    let actual_ids: Vec<&str> = channels.iter().map(|c| c.id.as_str()).collect();
    let expected_ids: Vec<&str> = expected_order.split(',').collect();
    assert_eq!(actual_ids, expected_ids, "Channel order mismatch");
}

#[then(expr = "the provider {string} should exist")]
async fn then_provider_should_exist(w: &mut AlephWorld, provider_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_provider(&provider_id).is_some(),
        "Provider '{}' should exist",
        provider_id
    );
}

#[then(expr = "the provider count should be {int}")]
async fn then_provider_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_providers().len();
    assert_eq!(
        count, expected,
        "Expected {} providers, got {}",
        expected, count
    );
}

#[then(expr = "the plugin {string} should have provider {string}")]
async fn then_plugin_has_provider(w: &mut AlephWorld, plugin_id: String, provider_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let plugin = registry.get_plugin(&plugin_id).expect("Plugin not found");
    assert!(
        plugin.provider_ids.contains(&provider_id),
        "Plugin '{}' should have provider '{}'",
        plugin_id,
        provider_id
    );
}

#[then(expr = "the gateway method {string} should exist")]
async fn then_gateway_method_should_exist(w: &mut AlephWorld, method: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_gateway_method(&method).is_some(),
        "Gateway method '{}' should exist",
        method
    );
}

#[then(expr = "the gateway method count should be {int}")]
async fn then_gateway_method_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_gateway_methods().len();
    assert_eq!(
        count, expected,
        "Expected {} gateway methods, got {}",
        expected, count
    );
}

#[then(expr = "the plugin {string} should have gateway method {string}")]
async fn then_plugin_has_gateway_method(w: &mut AlephWorld, plugin_id: String, method: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let plugin = registry.get_plugin(&plugin_id).expect("Plugin not found");
    assert!(
        plugin.gateway_methods.contains(&method),
        "Plugin '{}' should have gateway method '{}'",
        plugin_id,
        method
    );
}

#[then(expr = "the http route count should be {int}")]
async fn then_http_route_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_http_routes().len();
    assert_eq!(
        count, expected,
        "Expected {} http routes, got {}",
        expected, count
    );
}

#[then(expr = "the routes matching {string} should be {int}")]
async fn then_routes_matching(w: &mut AlephWorld, path: String, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.find_http_routes(&path).len();
    assert_eq!(
        count, expected,
        "Expected {} routes matching '{}', got {}",
        expected, path, count
    );
}

#[then(expr = "the http handler count should be {int}")]
async fn then_http_handler_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_http_handlers().len();
    assert_eq!(
        count, expected,
        "Expected {} http handlers, got {}",
        expected, count
    );
}

#[then(expr = "the http handlers should be sorted by priority as {string}")]
async fn then_http_handlers_sorted(w: &mut AlephWorld, priorities: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let handlers = registry.list_http_handlers();
    let actual_priorities: Vec<i32> = handlers.iter().map(|h| h.priority).collect();
    let expected_priorities: Vec<i32> = priorities
        .split(',')
        .map(|s| s.parse().expect("Invalid priority"))
        .collect();
    assert_eq!(
        actual_priorities, expected_priorities,
        "HTTP handler priorities mismatch"
    );
}

#[then(expr = "the cli command {string} should exist")]
async fn then_cli_command_should_exist(w: &mut AlephWorld, name: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_cli_command(&name).is_some(),
        "CLI command '{}' should exist",
        name
    );
}

#[then(expr = "the cli command count should be {int}")]
async fn then_cli_command_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_cli_commands().len();
    assert_eq!(
        count, expected,
        "Expected {} cli commands, got {}",
        expected, count
    );
}

#[then(expr = "the service {string} should exist")]
async fn then_service_should_exist(w: &mut AlephWorld, service_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_service(&service_id).is_some(),
        "Service '{}' should exist",
        service_id
    );
}

#[then(expr = "the service count should be {int}")]
async fn then_service_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_services().len();
    assert_eq!(
        count, expected,
        "Expected {} services, got {}",
        expected, count
    );
}

#[then(expr = "the plugin {string} should have service {string}")]
async fn then_plugin_has_service(w: &mut AlephWorld, plugin_id: String, service_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let plugin = registry.get_plugin(&plugin_id).expect("Plugin not found");
    assert!(
        plugin.service_ids.contains(&service_id),
        "Plugin '{}' should have service '{}'",
        plugin_id,
        service_id
    );
}

#[then(expr = "the in-chat command {string} should exist")]
async fn then_inchat_command_should_exist(w: &mut AlephWorld, name: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    assert!(
        registry.get_command(&name).is_some(),
        "In-chat command '{}' should exist",
        name
    );
}

#[then(expr = "the in-chat command count should be {int}")]
async fn then_inchat_command_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.list_commands().len();
    assert_eq!(
        count, expected,
        "Expected {} in-chat commands, got {}",
        expected, count
    );
}

#[then(expr = "the diagnostic count should be {int}")]
async fn then_diagnostic_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not initialized");
    let count = registry.diagnostics().len();
    assert_eq!(
        count, expected,
        "Expected {} diagnostics, got {}",
        expected, count
    );
}

#[then(expr = "the stats should show {int} plugins")]
async fn then_stats_show_plugins(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let stats = ctx.stats.as_ref().expect("Stats not captured");
    assert_eq!(
        stats.plugins, expected,
        "Expected stats.plugins = {}, got {}",
        expected, stats.plugins
    );
}

#[then(expr = "the stats should show {int} active plugins")]
async fn then_stats_show_active_plugins(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let stats = ctx.stats.as_ref().expect("Stats not captured");
    assert_eq!(
        stats.active_plugins, expected,
        "Expected stats.active_plugins = {}, got {}",
        expected, stats.active_plugins
    );
}

#[then(expr = "the stats should show {int} tools")]
async fn then_stats_show_tools(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let stats = ctx.stats.as_ref().expect("Stats not captured");
    assert_eq!(
        stats.tools, expected,
        "Expected stats.tools = {}, got {}",
        expected, stats.tools
    );
}

#[then(expr = "the stats should show {int} hooks")]
async fn then_stats_show_hooks(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let stats = ctx.stats.as_ref().expect("Stats not captured");
    assert_eq!(
        stats.hooks, expected,
        "Expected stats.hooks = {}, got {}",
        expected, stats.hooks
    );
}
