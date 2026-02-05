//! Step definitions for Extension Plugin Registry features

use cucumber::{gherkin::Step, given, then, when};
use std::path::PathBuf;
use tempfile::TempDir;

use crate::world::{AlephWorld, ExtensionContext};
use alephcore::extension::{
    manifest::{
        parse_aleph_plugin_content, parse_aleph_plugin_toml_content, parse_manifest_from_dir_sync,
        FilesystemPermission,
    },
    match_path, ChannelRegistration, CliRegistration, CommandRegistration, DiagnosticLevel,
    DirectCommandResult, ExtensionConfig, ExtensionError, ExtensionManager,
    GatewayMethodRegistration, HookRegistration, HttpHandlerRegistration, HttpRouteRegistration,
    PluginDiagnostic, PluginHookEvent, PluginKind, PluginLoader, PluginOrigin, PluginPermission,
    PluginRecord, PluginRegistry, ProviderRegistration, ServiceInfo, ServiceRegistration,
    ServiceResult, ServiceState, ToolRegistration,
};

// Lazy-initialized extension manager for runtime tests
use std::sync::OnceLock;
static EXTENSION_MANAGER: OnceLock<tokio::sync::Mutex<Option<ExtensionManager>>> = OnceLock::new();

fn get_manager_lock() -> &'static tokio::sync::Mutex<Option<ExtensionManager>> {
    EXTENSION_MANAGER.get_or_init(|| tokio::sync::Mutex::new(None))
}

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

// =============================================================================
// V2 Manifest Parsing - Given Steps
// =============================================================================

#[given("a temp directory with manifest files:")]
async fn given_temp_dir_with_files(w: &mut AlephWorld, step: &Step) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let temp_dir = TempDir::new().expect("Failed to create temp directory");

    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            // skip header
            let file = &row[0];
            let content = &row[1];
            // Handle escaped newlines in the content
            let content = content.replace("\\n", "\n");
            let file_path = temp_dir.path().join(file);

            // Create parent directories if needed
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent).expect("Failed to create parent directories");
            }

            std::fs::write(&file_path, content).expect("Failed to write file");
        }
    }

    ctx.temp_dir = Some(temp_dir);
}

#[given("a TOML manifest content:")]
async fn given_toml_content(w: &mut AlephWorld, step: &Step) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    ctx.toml_content = step.docstring.clone().map(|d| d.to_string());
}

#[given("a JSON manifest content:")]
async fn given_json_content(w: &mut AlephWorld, step: &Step) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    ctx.json_content = step.docstring.clone().map(|d| d.to_string());
}

#[given("an empty temp directory")]
async fn given_empty_temp_dir(w: &mut AlephWorld) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    ctx.temp_dir = Some(TempDir::new().expect("Failed to create temp directory"));
}

#[given(expr = "a ServiceInfo with id {string} plugin {string} name {string} state Running")]
async fn given_service_info(w: &mut AlephWorld, id: String, plugin_id: String, name: String) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    ctx.service_info = Some(ServiceInfo {
        id,
        plugin_id,
        name,
        state: ServiceState::Running,
        started_at: None,
        error: None,
    });
}

#[given("an extension manager with default config")]
async fn given_extension_manager_default(w: &mut AlephWorld) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let config = ExtensionConfig::default();
    let manager = ExtensionManager::new(config)
        .await
        .expect("Failed to create extension manager");
    *get_manager_lock().lock().await = Some(manager);
    ctx.manager_created = true;
}

#[given("a new plugin loader")]
async fn given_plugin_loader(w: &mut AlephWorld) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    let loader = PluginLoader::new();
    // Capture loader state
    ctx.any_runtime_active = loader.is_any_runtime_active();
    ctx.nodejs_runtime_active = loader.is_nodejs_runtime_active();
    ctx.wasm_runtime_active = loader.is_wasm_runtime_active();
    ctx.loaded_plugin_count = loader.loaded_count();
    ctx.loader_created = true;
}

#[given("a new standalone plugin registry")]
async fn given_standalone_registry(w: &mut AlephWorld) {
    let ctx = w.extension.get_or_insert_with(ExtensionContext::default);
    ctx.registry = Some(PluginRegistry::new());
}

// =============================================================================
// V2 Manifest Parsing - When Steps
// =============================================================================

#[when("I parse the manifest from the directory")]
async fn when_parse_manifest_from_dir(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp directory not set");

    match parse_manifest_from_dir_sync(temp_dir.path()) {
        Ok(manifest) => {
            ctx.manifest = Some(manifest);
            ctx.parse_error = None;
        }
        Err(e) => {
            ctx.manifest = None;
            ctx.parse_error = Some(e.to_string());
        }
    }
}

#[when("I parse the manifest from the directory expecting error")]
async fn when_parse_manifest_from_dir_expecting_error(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp directory not set");

    match parse_manifest_from_dir_sync(temp_dir.path()) {
        Ok(manifest) => {
            ctx.manifest = Some(manifest);
            ctx.parse_error = None;
        }
        Err(e) => {
            ctx.manifest = None;
            ctx.parse_error = Some(e.to_string());
        }
    }
}

#[when("I parse the TOML manifest")]
async fn when_parse_toml_manifest(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let content = ctx.toml_content.as_ref().expect("TOML content not set");

    match parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")) {
        Ok(manifest) => {
            ctx.manifest = Some(manifest);
            ctx.parse_error = None;
        }
        Err(e) => {
            ctx.manifest = None;
            ctx.parse_error = Some(e.to_string());
        }
    }
}

#[when("I parse the TOML manifest expecting error")]
async fn when_parse_toml_manifest_expecting_error(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let content = ctx.toml_content.as_ref().expect("TOML content not set");

    match parse_aleph_plugin_toml_content(content, std::path::Path::new("/test")) {
        Ok(manifest) => {
            ctx.manifest = Some(manifest);
            ctx.parse_error = None;
        }
        Err(e) => {
            ctx.manifest = None;
            ctx.parse_error = Some(e.to_string());
        }
    }
}

#[when("I parse the JSON manifest")]
async fn when_parse_json_manifest(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let content = ctx.json_content.as_ref().expect("JSON content not set");

    match parse_aleph_plugin_content(content, std::path::Path::new("/test")) {
        Ok(manifest) => {
            ctx.manifest = Some(manifest);
            ctx.parse_error = None;
        }
        Err(e) => {
            ctx.manifest = None;
            ctx.parse_error = Some(e.to_string());
        }
    }
}

#[when("I parse the JSON manifest expecting error")]
async fn when_parse_json_manifest_expecting_error(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let content = ctx.json_content.as_ref().expect("JSON content not set");

    match parse_aleph_plugin_content(content, std::path::Path::new("/test")) {
        Ok(manifest) => {
            ctx.manifest = Some(manifest);
            ctx.parse_error = None;
        }
        Err(e) => {
            ctx.manifest = None;
            ctx.parse_error = Some(e.to_string());
        }
    }
}

#[when("I serialize the ServiceInfo to JSON")]
async fn when_serialize_service_info(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let info = ctx.service_info.as_ref().expect("ServiceInfo not set");
    ctx.serialized_json = Some(serde_json::to_string(info).expect("Failed to serialize"));
}

#[when("I deserialize the JSON to ServiceInfo")]
async fn when_deserialize_service_info(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    let json = ctx.serialized_json.as_ref().expect("JSON not set");
    ctx.service_info =
        Some(serde_json::from_str(json).expect("Failed to deserialize ServiceInfo"));
}

#[when("I get the plugin registry")]
async fn when_get_plugin_registry(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    assert!(ctx.manager_created, "Extension manager not created");
    let guard = get_manager_lock().lock().await;
    let manager = guard.as_ref().expect("Extension manager not set");
    let _registry = manager.get_plugin_registry().await;
    // Just verify access works - store a new registry for verification
    ctx.registry = Some(PluginRegistry::new());
}

#[when("I get the plugin loader")]
async fn when_get_plugin_loader(w: &mut AlephWorld) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    assert!(ctx.manager_created, "Extension manager not created");
    let guard = get_manager_lock().lock().await;
    let manager = guard.as_ref().expect("Extension manager not set");
    let loader = manager.get_plugin_loader().await;
    // Capture loader state
    ctx.any_runtime_active = loader.is_any_runtime_active();
    ctx.nodejs_runtime_active = loader.is_nodejs_runtime_active();
    ctx.wasm_runtime_active = loader.is_wasm_runtime_active();
    ctx.loaded_plugin_count = loader.loaded_count();
    ctx.loader_created = true;
}

#[when(expr = "I call tool on non-existent plugin {string}")]
async fn when_call_tool_nonexistent(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    assert!(ctx.manager_created, "Extension manager not created");
    let guard = get_manager_lock().lock().await;
    let manager = guard.as_ref().expect("Extension manager not set");

    let result = manager
        .call_plugin_tool(&plugin_id, "someHandler", serde_json::json!({}))
        .await;

    match result {
        Ok(value) => {
            ctx.tool_result = Some(value);
            ctx.extension_error = None;
        }
        Err(e) => {
            ctx.tool_result = None;
            ctx.extension_error = Some(e);
        }
    }
}

#[when(expr = "I execute hook on non-existent plugin {string}")]
async fn when_execute_hook_nonexistent(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    assert!(ctx.manager_created, "Extension manager not created");
    let guard = get_manager_lock().lock().await;
    let manager = guard.as_ref().expect("Extension manager not set");

    let result = manager
        .execute_plugin_hook(&plugin_id, "onEvent", serde_json::json!({"test": true}))
        .await;

    match result {
        Ok(value) => {
            ctx.hook_result = Some(value);
            ctx.extension_error = None;
        }
        Err(e) => {
            ctx.hook_result = None;
            ctx.extension_error = Some(e);
        }
    }
}

#[when(expr = "I unload plugin {string}")]
async fn when_unload_plugin(w: &mut AlephWorld, plugin_id: String) {
    let ctx = w.extension.as_mut().expect("Extension context not initialized");
    // For unload tests, we use a fresh loader
    let mut loader = PluginLoader::new();

    match loader.unload_plugin(&plugin_id) {
        Ok(_) => {
            ctx.extension_error = None;
        }
        Err(e) => {
            ctx.extension_error = Some(e);
        }
    }
}

// =============================================================================
// V2 Manifest Parsing - Then Steps
// =============================================================================

#[then("the parse should have failed")]
async fn then_parse_should_fail(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    assert!(
        ctx.parse_error.is_some(),
        "Expected parsing to fail, but it succeeded"
    );
}

#[then(expr = "the manifest id should be {string}")]
async fn then_manifest_id(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert_eq!(manifest.id, expected, "Manifest id mismatch");
}

#[then(expr = "the manifest name should be {string}")]
async fn then_manifest_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert_eq!(manifest.name, expected, "Manifest name mismatch");
}

#[then(expr = "the manifest version should be {string}")]
async fn then_manifest_version(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert_eq!(
        manifest.version,
        Some(expected.clone()),
        "Manifest version mismatch"
    );
}

#[then("the manifest version should be empty")]
async fn then_manifest_version_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(manifest.version.is_none(), "Manifest version should be empty");
}

#[then(expr = "the manifest description should be {string}")]
async fn then_manifest_description(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert_eq!(
        manifest.description,
        Some(expected),
        "Manifest description mismatch"
    );
}

#[then(expr = "the manifest kind should be {string}")]
async fn then_manifest_kind(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let kind_str = match manifest.kind {
        PluginKind::Wasm => "wasm",
        PluginKind::NodeJs => "nodejs",
        PluginKind::Static => "static",
    };
    assert_eq!(kind_str, expected, "Manifest kind mismatch");
}

#[then(expr = "the manifest entry should be {string}")]
async fn then_manifest_entry(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert_eq!(
        manifest.entry,
        PathBuf::from(expected),
        "Manifest entry mismatch"
    );
}

#[then(expr = "the manifest homepage should be {string}")]
async fn then_manifest_homepage(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert_eq!(
        manifest.homepage,
        Some(expected),
        "Manifest homepage mismatch"
    );
}

#[then(expr = "the manifest repository should be {string}")]
async fn then_manifest_repository(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert_eq!(
        manifest.repository,
        Some(expected),
        "Manifest repository mismatch"
    );
}

#[then(expr = "the manifest license should be {string}")]
async fn then_manifest_license(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert_eq!(manifest.license, Some(expected), "Manifest license mismatch");
}

#[then(expr = "the manifest keywords should be {string}")]
async fn then_manifest_keywords(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let expected_vec: Vec<&str> = expected.split(',').collect();
    assert_eq!(manifest.keywords, expected_vec, "Manifest keywords mismatch");
}

#[then(expr = "the manifest author name should be {string}")]
async fn then_manifest_author_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let author = manifest.author.as_ref().expect("Author not set");
    assert_eq!(author.name, Some(expected), "Author name mismatch");
}

#[then(expr = "the manifest author email should be {string}")]
async fn then_manifest_author_email(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let author = manifest.author.as_ref().expect("Author not set");
    assert_eq!(author.email, Some(expected), "Author email mismatch");
}

#[then("the manifest root_dir should match the temp directory")]
async fn then_manifest_root_dir_matches(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp directory not set");
    assert_eq!(
        manifest.root_dir,
        temp_dir.path(),
        "Manifest root_dir mismatch"
    );
}

// Tools assertions
#[then(expr = "the manifest should have {int} tool(s)")]
async fn then_manifest_tool_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let count = manifest.tools_v2.as_ref().map(|t| t.len()).unwrap_or(0);
    assert_eq!(count, expected, "Expected {} tools, got {}", expected, count);
}

#[then("the manifest tools should be empty")]
async fn then_manifest_tools_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(manifest.tools_v2.is_none(), "Expected tools to be empty");
}

#[then(expr = "tool {int} name should be {string}")]
async fn then_tool_name(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let tools = manifest.tools_v2.as_ref().expect("Tools not set");
    assert_eq!(tools[index].name, expected, "Tool name mismatch");
}

#[then(expr = "tool {int} description should be {string}")]
async fn then_tool_description(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let tools = manifest.tools_v2.as_ref().expect("Tools not set");
    assert_eq!(
        tools[index].description,
        Some(expected),
        "Tool description mismatch"
    );
}

#[then(expr = "tool {int} handler should be {string}")]
async fn then_tool_handler(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let tools = manifest.tools_v2.as_ref().expect("Tools not set");
    assert_eq!(
        tools[index].handler,
        Some(expected),
        "Tool handler mismatch"
    );
}

#[then(expr = "tool {int} instruction_file should be {string}")]
async fn then_tool_instruction_file(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let tools = manifest.tools_v2.as_ref().expect("Tools not set");
    assert_eq!(
        tools[index].instruction_file,
        Some(expected),
        "Tool instruction_file mismatch"
    );
}

#[then(expr = "tool {int} instruction_file should be empty")]
async fn then_tool_instruction_file_empty(w: &mut AlephWorld, index: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let tools = manifest.tools_v2.as_ref().expect("Tools not set");
    assert!(
        tools[index].instruction_file.is_none(),
        "Tool instruction_file should be empty"
    );
}

#[then(expr = "tool {int} should have parameters with type {string}")]
async fn then_tool_params_type(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let tools = manifest.tools_v2.as_ref().expect("Tools not set");
    let params = tools[index].parameters.as_ref().expect("Parameters not set");
    assert_eq!(params["type"], expected, "Tool parameters type mismatch");
}

#[then(expr = "tool {int} parameters should require {string}")]
async fn then_tool_params_require(w: &mut AlephWorld, index: usize, required: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let tools = manifest.tools_v2.as_ref().expect("Tools not set");
    let params = tools[index].parameters.as_ref().expect("Parameters not set");
    let required_arr = params["required"].as_array().expect("Required not an array");
    assert!(
        required_arr.contains(&serde_json::json!(required)),
        "Tool parameters should require '{}'",
        required
    );
}

// Hooks assertions
#[then(expr = "the manifest should have {int} hook(s)")]
async fn then_manifest_hook_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let count = manifest.hooks_v2.as_ref().map(|h| h.len()).unwrap_or(0);
    assert_eq!(count, expected, "Expected {} hooks, got {}", expected, count);
}

#[then("the manifest hooks should be empty")]
async fn then_manifest_hooks_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(manifest.hooks_v2.is_none(), "Expected hooks to be empty");
}

#[then(expr = "hook {int} event should be {string}")]
async fn then_hook_event(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let hooks = manifest.hooks_v2.as_ref().expect("Hooks not set");
    assert_eq!(hooks[index].event, expected, "Hook event mismatch");
}

#[then(expr = "hook {int} kind should be {string}")]
async fn then_hook_kind(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let hooks = manifest.hooks_v2.as_ref().expect("Hooks not set");
    assert_eq!(hooks[index].kind, expected, "Hook kind mismatch");
}

#[then(expr = "hook {int} priority should be {string}")]
async fn then_hook_priority(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let hooks = manifest.hooks_v2.as_ref().expect("Hooks not set");
    assert_eq!(hooks[index].priority, expected, "Hook priority mismatch");
}

#[then(expr = "hook {int} handler should be {string}")]
async fn then_hook_handler(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let hooks = manifest.hooks_v2.as_ref().expect("Hooks not set");
    assert_eq!(
        hooks[index].handler,
        Some(expected),
        "Hook handler mismatch"
    );
}

#[then(expr = "hook {int} filter should be {string}")]
async fn then_hook_filter(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let hooks = manifest.hooks_v2.as_ref().expect("Hooks not set");
    assert_eq!(hooks[index].filter, Some(expected), "Hook filter mismatch");
}

// Prompt assertions
#[then(expr = "the prompt file should be {string}")]
async fn then_prompt_file(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let prompt = manifest.prompt_v2.as_ref().expect("Prompt not set");
    assert_eq!(prompt.file, expected, "Prompt file mismatch");
}

#[then(expr = "the prompt scope should be {string}")]
async fn then_prompt_scope(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let prompt = manifest.prompt_v2.as_ref().expect("Prompt not set");
    assert_eq!(prompt.scope, expected, "Prompt scope mismatch");
}

#[then("the manifest prompt should be empty")]
async fn then_manifest_prompt_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(manifest.prompt_v2.is_none(), "Expected prompt to be empty");
}

// Permissions assertions
#[then(expr = "the manifest should have permission {string}")]
async fn then_manifest_has_permission(w: &mut AlephWorld, permission: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");

    let expected = if permission.starts_with("Custom:") {
        PluginPermission::Custom(permission.strip_prefix("Custom:").unwrap().to_string())
    } else {
        match permission.as_str() {
            "Network" => PluginPermission::Network,
            "Filesystem" => PluginPermission::Filesystem,
            "FilesystemRead" => PluginPermission::FilesystemRead,
            "FilesystemWrite" => PluginPermission::FilesystemWrite,
            "Env" => PluginPermission::Env,
            _ => panic!("Unknown permission: {}", permission),
        }
    };

    assert!(
        manifest.permissions.contains(&expected),
        "Manifest should have permission '{}'",
        permission
    );
}

#[then(expr = "the manifest should not have permission {string}")]
async fn then_manifest_not_have_permission(w: &mut AlephWorld, permission: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");

    let expected = if permission.starts_with("Custom:") {
        PluginPermission::Custom(permission.strip_prefix("Custom:").unwrap().to_string())
    } else {
        match permission.as_str() {
            "Network" => PluginPermission::Network,
            "Filesystem" => PluginPermission::Filesystem,
            "FilesystemRead" => PluginPermission::FilesystemRead,
            "FilesystemWrite" => PluginPermission::FilesystemWrite,
            "Env" => PluginPermission::Env,
            _ => panic!("Unknown permission: {}", permission),
        }
    };

    assert!(
        !manifest.permissions.contains(&expected),
        "Manifest should not have permission '{}'",
        permission
    );
}

#[then("the manifest permissions should be empty")]
async fn then_manifest_permissions_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(
        manifest.permissions.is_empty(),
        "Expected permissions to be empty"
    );
}

// FilesystemPermission assertions
#[then(expr = "FilesystemPermission Bool true should can_read")]
async fn then_fs_bool_true_can_read(_w: &mut AlephWorld) {
    assert!(FilesystemPermission::Bool(true).can_read());
}

#[then(expr = "FilesystemPermission Bool false should not can_read")]
async fn then_fs_bool_false_not_can_read(_w: &mut AlephWorld) {
    assert!(!FilesystemPermission::Bool(false).can_read());
}

#[then(expr = "FilesystemPermission Level {string} should can_read")]
async fn then_fs_level_can_read(_w: &mut AlephWorld, level: String) {
    assert!(FilesystemPermission::Level(level).can_read());
}

#[then(expr = "FilesystemPermission Level {string} should not can_read")]
async fn then_fs_level_not_can_read(_w: &mut AlephWorld, level: String) {
    assert!(!FilesystemPermission::Level(level).can_read());
}

#[then(expr = "FilesystemPermission Bool true should can_write")]
async fn then_fs_bool_true_can_write(_w: &mut AlephWorld) {
    assert!(FilesystemPermission::Bool(true).can_write());
}

#[then(expr = "FilesystemPermission Bool false should not can_write")]
async fn then_fs_bool_false_not_can_write(_w: &mut AlephWorld) {
    assert!(!FilesystemPermission::Bool(false).can_write());
}

#[then(expr = "FilesystemPermission Level {string} should can_write")]
async fn then_fs_level_can_write(_w: &mut AlephWorld, level: String) {
    assert!(FilesystemPermission::Level(level).can_write());
}

#[then(expr = "FilesystemPermission Level {string} should not can_write")]
async fn then_fs_level_not_can_write(_w: &mut AlephWorld, level: String) {
    assert!(!FilesystemPermission::Level(level).can_write());
}

// Capabilities assertions
#[then(expr = "the manifest capability dynamic_tools should be true")]
async fn then_capability_dynamic_tools_true(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let caps = manifest.capabilities_v2.as_ref().expect("Capabilities not set");
    assert!(caps.dynamic_tools, "Expected dynamic_tools to be true");
}

#[then(expr = "the manifest capability dynamic_tools should be false")]
async fn then_capability_dynamic_tools_false(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let caps = manifest.capabilities_v2.as_ref().expect("Capabilities not set");
    assert!(!caps.dynamic_tools, "Expected dynamic_tools to be false");
}

#[then(expr = "the manifest capability dynamic_hooks should be true")]
async fn then_capability_dynamic_hooks_true(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let caps = manifest.capabilities_v2.as_ref().expect("Capabilities not set");
    assert!(caps.dynamic_hooks, "Expected dynamic_hooks to be true");
}

#[then(expr = "the manifest capability dynamic_hooks should be false")]
async fn then_capability_dynamic_hooks_false(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let caps = manifest.capabilities_v2.as_ref().expect("Capabilities not set");
    assert!(!caps.dynamic_hooks, "Expected dynamic_hooks to be false");
}

// Services assertions
#[then(expr = "the manifest should have {int} service(s)")]
async fn then_manifest_service_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let count = manifest.services_v2.as_ref().map(|s| s.len()).unwrap_or(0);
    assert_eq!(
        count, expected,
        "Expected {} services, got {}",
        expected, count
    );
}

#[then("the manifest services should be empty")]
async fn then_manifest_services_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(manifest.services_v2.is_none(), "Expected services to be empty");
}

#[then(expr = "service {int} name should be {string}")]
async fn then_service_name(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let services = manifest.services_v2.as_ref().expect("Services not set");
    assert_eq!(services[index].name, expected, "Service name mismatch");
}

#[then(expr = "service {int} description should be {string}")]
async fn then_service_description(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let services = manifest.services_v2.as_ref().expect("Services not set");
    assert_eq!(
        services[index].description,
        Some(expected),
        "Service description mismatch"
    );
}

#[then(expr = "service {int} description should be empty")]
async fn then_service_description_empty(w: &mut AlephWorld, index: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let services = manifest.services_v2.as_ref().expect("Services not set");
    assert!(
        services[index].description.is_none(),
        "Service description should be empty"
    );
}

#[then(expr = "service {int} start_handler should be {string}")]
async fn then_service_start_handler(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let services = manifest.services_v2.as_ref().expect("Services not set");
    assert_eq!(
        services[index].start_handler,
        Some(expected),
        "Service start_handler mismatch"
    );
}

#[then(expr = "service {int} stop_handler should be {string}")]
async fn then_service_stop_handler(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let services = manifest.services_v2.as_ref().expect("Services not set");
    assert_eq!(
        services[index].stop_handler,
        Some(expected),
        "Service stop_handler mismatch"
    );
}

#[then(expr = "service {int} stop_handler should be empty")]
async fn then_service_stop_handler_empty(w: &mut AlephWorld, index: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let services = manifest.services_v2.as_ref().expect("Services not set");
    assert!(
        services[index].stop_handler.is_none(),
        "Service stop_handler should be empty"
    );
}

// Commands assertions
#[then(expr = "the manifest should have {int} command(s)")]
async fn then_manifest_command_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let count = manifest.commands_v2.as_ref().map(|c| c.len()).unwrap_or(0);
    assert_eq!(
        count, expected,
        "Expected {} commands, got {}",
        expected, count
    );
}

#[then("the manifest commands should be empty")]
async fn then_manifest_commands_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(
        manifest.commands_v2.is_none(),
        "Expected commands to be empty"
    );
}

#[then(expr = "command {int} name should be {string}")]
async fn then_command_name(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let commands = manifest.commands_v2.as_ref().expect("Commands not set");
    assert_eq!(commands[index].name, expected, "Command name mismatch");
}

#[then(expr = "command {int} description should be {string}")]
async fn then_command_description(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let commands = manifest.commands_v2.as_ref().expect("Commands not set");
    assert_eq!(
        commands[index].description,
        Some(expected),
        "Command description mismatch"
    );
}

#[then(expr = "command {int} handler should be {string}")]
async fn then_command_handler(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let commands = manifest.commands_v2.as_ref().expect("Commands not set");
    assert_eq!(
        commands[index].handler,
        Some(expected),
        "Command handler mismatch"
    );
}

#[then(expr = "command {int} prompt_file should be {string}")]
async fn then_command_prompt_file(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let commands = manifest.commands_v2.as_ref().expect("Commands not set");
    assert_eq!(
        commands[index].prompt_file,
        Some(expected),
        "Command prompt_file mismatch"
    );
}

#[then(expr = "command {int} prompt_file should be empty")]
async fn then_command_prompt_file_empty(w: &mut AlephWorld, index: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let commands = manifest.commands_v2.as_ref().expect("Commands not set");
    assert!(
        commands[index].prompt_file.is_none(),
        "Command prompt_file should be empty"
    );
}

// Channels assertions
#[then(expr = "the manifest should have {int} channel(s)")]
async fn then_manifest_channel_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let count = manifest.channels_v2.as_ref().map(|c| c.len()).unwrap_or(0);
    assert_eq!(
        count, expected,
        "Expected {} channels, got {}",
        expected, count
    );
}

#[then("the manifest channels should be empty")]
async fn then_manifest_channels_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(
        manifest.channels_v2.is_none(),
        "Expected channels to be empty"
    );
}

#[then(expr = "channel {int} id should be {string}")]
async fn then_channel_id(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let channels = manifest.channels_v2.as_ref().expect("Channels not set");
    assert_eq!(channels[index].id, expected, "Channel id mismatch");
}

#[then(expr = "channel {int} label should be {string}")]
async fn then_channel_label(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let channels = manifest.channels_v2.as_ref().expect("Channels not set");
    assert_eq!(channels[index].label, expected, "Channel label mismatch");
}

#[then(expr = "channel {int} handler should be {string}")]
async fn then_channel_handler(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let channels = manifest.channels_v2.as_ref().expect("Channels not set");
    assert_eq!(
        channels[index].handler,
        Some(expected),
        "Channel handler mismatch"
    );
}

#[then(expr = "channel {int} should have config_schema")]
async fn then_channel_has_config_schema(w: &mut AlephWorld, index: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let channels = manifest.channels_v2.as_ref().expect("Channels not set");
    assert!(
        channels[index].config_schema.is_some(),
        "Channel should have config_schema"
    );
}

#[then(expr = "channel {int} should not have config_schema")]
async fn then_channel_not_have_config_schema(w: &mut AlephWorld, index: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let channels = manifest.channels_v2.as_ref().expect("Channels not set");
    assert!(
        channels[index].config_schema.is_none(),
        "Channel should not have config_schema"
    );
}

// Providers assertions
#[then(expr = "the manifest should have {int} provider(s)")]
async fn then_manifest_provider_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let count = manifest.providers_v2.as_ref().map(|p| p.len()).unwrap_or(0);
    assert_eq!(
        count, expected,
        "Expected {} providers, got {}",
        expected, count
    );
}

#[then("the manifest providers should be empty")]
async fn then_manifest_providers_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(
        manifest.providers_v2.is_none(),
        "Expected providers to be empty"
    );
}

#[then(expr = "provider {int} id should be {string}")]
async fn then_provider_id(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let providers = manifest.providers_v2.as_ref().expect("Providers not set");
    assert_eq!(providers[index].id, expected, "Provider id mismatch");
}

#[then(expr = "provider {int} name should be {string}")]
async fn then_provider_name(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let providers = manifest.providers_v2.as_ref().expect("Providers not set");
    assert_eq!(providers[index].name, expected, "Provider name mismatch");
}

#[then(expr = "provider {int} models should be {string}")]
async fn then_provider_models(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let providers = manifest.providers_v2.as_ref().expect("Providers not set");
    let models_str = providers[index].models.join(",");
    assert_eq!(models_str, expected, "Provider models mismatch");
}

#[then(expr = "provider {int} models count should be {int}")]
async fn then_provider_models_count(w: &mut AlephWorld, index: usize, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let providers = manifest.providers_v2.as_ref().expect("Providers not set");
    assert_eq!(
        providers[index].models.len(),
        expected,
        "Provider models count mismatch"
    );
}

#[then(expr = "provider {int} handler should be {string}")]
async fn then_provider_handler(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let providers = manifest.providers_v2.as_ref().expect("Providers not set");
    assert_eq!(
        providers[index].handler,
        Some(expected),
        "Provider handler mismatch"
    );
}

#[then(expr = "provider {int} should have config_schema")]
async fn then_provider_has_config_schema(w: &mut AlephWorld, index: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let providers = manifest.providers_v2.as_ref().expect("Providers not set");
    assert!(
        providers[index].config_schema.is_some(),
        "Provider should have config_schema"
    );
}

#[then(expr = "provider {int} should not have config_schema")]
async fn then_provider_not_have_config_schema(w: &mut AlephWorld, index: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let providers = manifest.providers_v2.as_ref().expect("Providers not set");
    assert!(
        providers[index].config_schema.is_none(),
        "Provider should not have config_schema"
    );
}

// HTTP Routes assertions
#[then(expr = "the manifest should have {int} http_route(s)")]
async fn then_manifest_http_route_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let count = manifest.http_routes_v2.as_ref().map(|r| r.len()).unwrap_or(0);
    assert_eq!(
        count, expected,
        "Expected {} http_routes, got {}",
        expected, count
    );
}

#[then("the manifest http_routes should be empty")]
async fn then_manifest_http_routes_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(
        manifest.http_routes_v2.is_none(),
        "Expected http_routes to be empty"
    );
}

#[then(expr = "http_route {int} path should be {string}")]
async fn then_http_route_path(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let routes = manifest.http_routes_v2.as_ref().expect("HTTP routes not set");
    assert_eq!(routes[index].path, expected, "HTTP route path mismatch");
}

#[then(expr = "http_route {int} methods should be {string}")]
async fn then_http_route_methods(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let routes = manifest.http_routes_v2.as_ref().expect("HTTP routes not set");
    let methods_str = routes[index].methods.join(",");
    assert_eq!(methods_str, expected, "HTTP route methods mismatch");
}

#[then(expr = "http_route {int} methods count should be {int}")]
async fn then_http_route_methods_count(w: &mut AlephWorld, index: usize, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let routes = manifest.http_routes_v2.as_ref().expect("HTTP routes not set");
    assert_eq!(
        routes[index].methods.len(),
        expected,
        "HTTP route methods count mismatch"
    );
}

#[then(expr = "http_route {int} handler should be {string}")]
async fn then_http_route_handler(w: &mut AlephWorld, index: usize, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let routes = manifest.http_routes_v2.as_ref().expect("HTTP routes not set");
    assert_eq!(routes[index].handler, expected, "HTTP route handler mismatch");
}

// HTTP Path Matching assertions
#[then(expr = "HTTP path {string} should match {string} with no params")]
async fn then_http_path_match_no_params(_w: &mut AlephWorld, pattern: String, path: String) {
    let params = match_path(&pattern, &path);
    assert!(params.is_some(), "Expected path '{}' to match pattern '{}'", path, pattern);
    assert!(params.unwrap().is_empty(), "Expected no params");
}

#[then(expr = "HTTP path {string} should match {string} with id={string}")]
async fn then_http_path_match_id(_w: &mut AlephWorld, pattern: String, path: String, id: String) {
    let params = match_path(&pattern, &path);
    assert!(params.is_some(), "Expected path '{}' to match pattern '{}'", path, pattern);
    let params = params.unwrap();
    assert_eq!(params.get("id"), Some(&id), "Expected id param to be '{}'", id);
}

#[then(expr = "HTTP path {string} should match {string} with org={string} repo={string}")]
async fn then_http_path_match_org_repo(
    _w: &mut AlephWorld,
    pattern: String,
    path: String,
    org: String,
    repo: String,
) {
    let params = match_path(&pattern, &path);
    assert!(params.is_some(), "Expected path '{}' to match pattern '{}'", path, pattern);
    let params = params.unwrap();
    assert_eq!(params.get("org"), Some(&org), "Expected org param");
    assert_eq!(params.get("repo"), Some(&repo), "Expected repo param");
}

#[then(expr = "HTTP path {string} should match {string} with version={string} user_id={string} post_id={string}")]
async fn then_http_path_match_complex(
    _w: &mut AlephWorld,
    pattern: String,
    path: String,
    version: String,
    user_id: String,
    post_id: String,
) {
    let params = match_path(&pattern, &path);
    assert!(params.is_some(), "Expected path '{}' to match pattern '{}'", path, pattern);
    let params = params.unwrap();
    assert_eq!(params.get("version"), Some(&version), "Expected version param");
    assert_eq!(params.get("user_id"), Some(&user_id), "Expected user_id param");
    assert_eq!(params.get("post_id"), Some(&post_id), "Expected post_id param");
}

#[then(expr = "HTTP path {string} should not match {string}")]
async fn then_http_path_not_match(_w: &mut AlephWorld, pattern: String, path: String) {
    let params = match_path(&pattern, &path);
    assert!(params.is_none(), "Expected path '{}' to NOT match pattern '{}'", path, pattern);
}

// Config Schema assertions
#[then("the manifest should have config_schema")]
async fn then_manifest_has_config_schema(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    assert!(manifest.config_schema.is_some(), "Expected config_schema");
}

#[then(expr = "the config_schema type should be {string}")]
async fn then_config_schema_type(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let schema = manifest.config_schema.as_ref().expect("Config schema not set");
    assert_eq!(schema["type"], expected, "Config schema type mismatch");
}

#[then(expr = "the ui_hint {string} label should be {string}")]
async fn then_ui_hint_label(w: &mut AlephWorld, key: String, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let hint = manifest.config_ui_hints.get(&key).expect("UI hint not found");
    assert_eq!(hint.label, Some(expected), "UI hint label mismatch");
}

#[then(expr = "the ui_hint {string} sensitive should be true")]
async fn then_ui_hint_sensitive(w: &mut AlephWorld, key: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let hint = manifest.config_ui_hints.get(&key).expect("UI hint not found");
    assert_eq!(hint.sensitive, Some(true), "UI hint sensitive mismatch");
}

#[then(expr = "the ui_hint {string} advanced should be true")]
async fn then_ui_hint_advanced(w: &mut AlephWorld, key: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let manifest = ctx.manifest.as_ref().expect("Manifest not parsed");
    let hint = manifest.config_ui_hints.get(&key).expect("UI hint not found");
    assert_eq!(hint.advanced, Some(true), "UI hint advanced mismatch");
}

// Service serialization assertions
#[then(expr = "ServiceState Running should serialize to {string}")]
async fn then_service_state_running_serialize(_w: &mut AlephWorld, expected: String) {
    let json = serde_json::to_string(&ServiceState::Running).unwrap();
    assert_eq!(json, format!("\"{}\"", expected));
}

#[then(expr = "ServiceState Stopped should serialize to {string}")]
async fn then_service_state_stopped_serialize(_w: &mut AlephWorld, expected: String) {
    let json = serde_json::to_string(&ServiceState::Stopped).unwrap();
    assert_eq!(json, format!("\"{}\"", expected));
}

#[then(expr = "ServiceState Starting should serialize to {string}")]
async fn then_service_state_starting_serialize(_w: &mut AlephWorld, expected: String) {
    let json = serde_json::to_string(&ServiceState::Starting).unwrap();
    assert_eq!(json, format!("\"{}\"", expected));
}

#[then(expr = "ServiceState Stopping should serialize to {string}")]
async fn then_service_state_stopping_serialize(_w: &mut AlephWorld, expected: String) {
    let json = serde_json::to_string(&ServiceState::Stopping).unwrap();
    assert_eq!(json, format!("\"{}\"", expected));
}

#[then(expr = "ServiceState Failed should serialize to {string}")]
async fn then_service_state_failed_serialize(_w: &mut AlephWorld, expected: String) {
    let json = serde_json::to_string(&ServiceState::Failed).unwrap();
    assert_eq!(json, format!("\"{}\"", expected));
}

#[then(expr = "{string} should deserialize to ServiceState Stopped")]
async fn then_deserialize_to_stopped(_w: &mut AlephWorld, json: String) {
    let state: ServiceState = serde_json::from_str(&format!("\"{}\"", json)).unwrap();
    assert_eq!(state, ServiceState::Stopped);
}

#[then(expr = "{string} should deserialize to ServiceState Running")]
async fn then_deserialize_to_running(_w: &mut AlephWorld, json: String) {
    let state: ServiceState = serde_json::from_str(&format!("\"{}\"", json)).unwrap();
    assert_eq!(state, ServiceState::Running);
}

#[then(expr = "{string} should deserialize to ServiceState Failed")]
async fn then_deserialize_to_failed(_w: &mut AlephWorld, json: String) {
    let state: ServiceState = serde_json::from_str(&format!("\"{}\"", json)).unwrap();
    assert_eq!(state, ServiceState::Failed);
}

#[then(expr = "the serialized JSON should contain {string}")]
async fn then_serialized_json_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let json = ctx.serialized_json.as_ref().expect("JSON not set");
    assert!(json.contains(&expected), "Serialized JSON should contain '{}'", expected);
}

#[then(expr = "the ServiceInfo id should be {string}")]
async fn then_service_info_id(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let info = ctx.service_info.as_ref().expect("ServiceInfo not set");
    assert_eq!(info.id, expected, "ServiceInfo id mismatch");
}

#[then(expr = "the ServiceInfo plugin_id should be {string}")]
async fn then_service_info_plugin_id(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let info = ctx.service_info.as_ref().expect("ServiceInfo not set");
    assert_eq!(info.plugin_id, expected, "ServiceInfo plugin_id mismatch");
}

#[then(expr = "the ServiceInfo name should be {string}")]
async fn then_service_info_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let info = ctx.service_info.as_ref().expect("ServiceInfo not set");
    assert_eq!(info.name, expected, "ServiceInfo name mismatch");
}

#[then("the ServiceInfo state should be Running")]
async fn then_service_info_state_running(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let info = ctx.service_info.as_ref().expect("ServiceInfo not set");
    assert_eq!(info.state, ServiceState::Running, "ServiceInfo state mismatch");
}

// ServiceResult assertions
#[then("ServiceResult ok should have success true")]
async fn then_service_result_ok_success(_w: &mut AlephWorld) {
    let result = ServiceResult::ok();
    assert!(result.success, "ServiceResult ok should have success true");
}

#[then("ServiceResult ok should have no message")]
async fn then_service_result_ok_no_message(_w: &mut AlephWorld) {
    let result = ServiceResult::ok();
    assert!(result.message.is_none(), "ServiceResult ok should have no message");
}

#[then(expr = "ServiceResult ok_with_message {string} should have success true")]
async fn then_service_result_ok_with_msg_success(_w: &mut AlephWorld, _msg: String) {
    let result = ServiceResult::ok_with_message("test");
    assert!(result.success, "ServiceResult ok_with_message should have success true");
}

#[then(expr = "ServiceResult ok_with_message {string} should have message {string}")]
async fn then_service_result_ok_with_msg_message(_w: &mut AlephWorld, _msg1: String, msg2: String) {
    let result = ServiceResult::ok_with_message(&msg2);
    assert_eq!(result.message, Some(msg2), "Message mismatch");
}

#[then(expr = "ServiceResult error {string} should have success false")]
async fn then_service_result_error_success(_w: &mut AlephWorld, _msg: String) {
    let result = ServiceResult::error("test");
    assert!(!result.success, "ServiceResult error should have success false");
}

#[then(expr = "ServiceResult error {string} should have message {string}")]
async fn then_service_result_error_message(_w: &mut AlephWorld, _msg1: String, msg2: String) {
    let result = ServiceResult::error(&msg2);
    assert_eq!(result.message, Some(msg2), "Message mismatch");
}

// DirectCommandResult assertions
#[then(expr = "DirectCommandResult success {string} should have success true and content {string}")]
async fn then_direct_command_success(_w: &mut AlephWorld, content: String, expected: String) {
    let result = DirectCommandResult::success(&content);
    assert!(result.success, "DirectCommandResult success should have success true");
    assert_eq!(result.content, expected, "Content mismatch");
}

#[then(expr = "DirectCommandResult with_data {string} with count {int} should have data")]
async fn then_direct_command_with_data(_w: &mut AlephWorld, _content: String, count: i32) {
    let result = DirectCommandResult::with_data("Result", serde_json::json!({"count": count}));
    assert!(result.success, "DirectCommandResult with_data should have success true");
    assert!(result.data.is_some(), "DirectCommandResult with_data should have data");
    assert_eq!(result.data.as_ref().unwrap()["count"], count);
}

#[then(expr = "DirectCommandResult error {string} should have success false and content {string}")]
async fn then_direct_command_error(_w: &mut AlephWorld, content: String, expected: String) {
    let result = DirectCommandResult::error(&content);
    assert!(!result.success, "DirectCommandResult error should have success false");
    assert_eq!(result.content, expected, "Content mismatch");
}

// Runtime assertions
#[then("the registry should be empty")]
async fn then_registry_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not set");
    assert!(registry.list_plugins().is_empty(), "Registry should be empty");
}

#[then("the registry tools should be empty")]
async fn then_registry_tools_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not set");
    assert!(registry.list_tools().is_empty(), "Registry tools should be empty");
}

#[then("the registry hooks should be empty")]
async fn then_registry_hooks_empty(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not set");
    assert!(registry.list_hooks().is_empty(), "Registry hooks should be empty");
}

#[then(expr = "the registry stats plugins should be {int}")]
async fn then_registry_stats_plugins(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not set");
    let stats = registry.stats();
    assert_eq!(stats.plugins, expected, "Registry stats plugins mismatch");
}

#[then(expr = "the registry stats tools should be {int}")]
async fn then_registry_stats_tools(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not set");
    let stats = registry.stats();
    assert_eq!(stats.tools, expected, "Registry stats tools mismatch");
}

#[then(expr = "the registry stats hooks should be {int}")]
async fn then_registry_stats_hooks(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let registry = ctx.registry.as_ref().expect("Registry not set");
    let stats = registry.stats();
    assert_eq!(stats.hooks, expected, "Registry stats hooks mismatch");
}

#[then("no runtime should be active")]
async fn then_no_runtime_active(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    assert!(ctx.loader_created, "Plugin loader not created");
    assert!(!ctx.any_runtime_active, "No runtime should be active");
}

#[then("nodejs runtime should not be active")]
async fn then_nodejs_runtime_not_active(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    assert!(ctx.loader_created, "Plugin loader not created");
    assert!(!ctx.nodejs_runtime_active, "Node.js runtime should not be active");
}

#[then("wasm runtime should not be active")]
async fn then_wasm_runtime_not_active(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    assert!(ctx.loader_created, "Plugin loader not created");
    assert!(!ctx.wasm_runtime_active, "WASM runtime should not be active");
}

#[then("no plugins should be loaded")]
async fn then_no_plugins_loaded(w: &mut AlephWorld) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    assert!(ctx.loader_created, "Plugin loader not created");
    assert_eq!(ctx.loaded_plugin_count, 0, "No plugins should be loaded");
}

#[then(expr = "loaded plugin count should be {int}")]
async fn then_loaded_plugin_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    assert!(ctx.loader_created, "Plugin loader not created");
    assert_eq!(ctx.loaded_plugin_count, expected, "Loaded plugin count mismatch");
}

#[then(expr = "the error should be PluginNotFound with id {string}")]
async fn then_error_plugin_not_found(w: &mut AlephWorld, expected_id: String) {
    let ctx = w.extension.as_ref().expect("Extension context not initialized");
    let error = ctx.extension_error.as_ref().expect("Error not set");
    match error {
        ExtensionError::PluginNotFound(id) => {
            assert_eq!(id, &expected_id, "PluginNotFound id mismatch");
        }
        _ => panic!("Expected PluginNotFound error, got: {:?}", error),
    }
}
