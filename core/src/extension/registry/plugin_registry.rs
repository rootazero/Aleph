//! Plugin Registry Implementation
//!
//! Central storage for all plugin registrations. The PluginRegistry maintains
//! a comprehensive registry of all plugins and their registered components:
//! tools, hooks, channels, providers, gateway methods, HTTP routes/handlers,
//! CLI commands, services, and in-chat commands.

use std::collections::HashMap;

use super::types::{
    ChannelRegistration, CliRegistration, CommandRegistration, GatewayMethodRegistration,
    HookRegistration, HttpHandlerRegistration, HttpRouteRegistration, PluginDiagnostic,
    PluginHookEvent, ProviderRegistration, ServiceRegistration, ToolRegistration,
};
use crate::extension::types::{PluginRecord, PluginStatus};

/// Central registry for all plugin registrations.
///
/// The PluginRegistry provides:
/// - Plugin lifecycle management (register, enable, disable, unregister)
/// - Component registration (tools, hooks, channels, providers, etc.)
/// - Query methods for accessing registered components
/// - Automatic tracking of which plugin registered each component
/// - Priority-ordered hook execution
#[derive(Debug, Default)]
pub struct PluginRegistry {
    /// Registered plugins by ID
    plugins: HashMap<String, PluginRecord>,

    /// Registered tools by name
    tools: HashMap<String, ToolRegistration>,

    /// Registered hooks (sorted by priority)
    hooks: Vec<HookRegistration>,

    /// Registered channels by ID
    channels: HashMap<String, ChannelRegistration>,

    /// Registered providers by ID
    providers: HashMap<String, ProviderRegistration>,

    /// Registered gateway RPC methods by method name
    gateway_methods: HashMap<String, GatewayMethodRegistration>,

    /// Registered HTTP routes
    http_routes: Vec<HttpRouteRegistration>,

    /// Registered HTTP handlers/middleware (sorted by priority)
    http_handlers: Vec<HttpHandlerRegistration>,

    /// Registered CLI commands by name
    cli_commands: HashMap<String, CliRegistration>,

    /// Registered background services by ID
    services: HashMap<String, ServiceRegistration>,

    /// Registered in-chat commands by name
    commands: HashMap<String, CommandRegistration>,

    /// Accumulated diagnostics from plugins
    diagnostics: Vec<PluginDiagnostic>,
}

impl PluginRegistry {
    /// Create a new empty plugin registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all registrations from the registry.
    ///
    /// This removes all plugins and their associated components.
    pub fn clear(&mut self) {
        self.plugins.clear();
        self.tools.clear();
        self.hooks.clear();
        self.channels.clear();
        self.providers.clear();
        self.gateway_methods.clear();
        self.http_routes.clear();
        self.http_handlers.clear();
        self.cli_commands.clear();
        self.services.clear();
        self.commands.clear();
        self.diagnostics.clear();
    }

    // =========================================================================
    // Plugin Management
    // =========================================================================

    /// Register a plugin in the registry.
    ///
    /// If a plugin with the same ID already exists, it will be replaced.
    pub fn register_plugin(&mut self, record: PluginRecord) {
        self.plugins.insert(record.id.clone(), record);
    }

    /// Get a plugin by ID.
    pub fn get_plugin(&self, id: &str) -> Option<&PluginRecord> {
        self.plugins.get(id)
    }

    /// Get a mutable reference to a plugin by ID.
    pub fn get_plugin_mut(&mut self, id: &str) -> Option<&mut PluginRecord> {
        self.plugins.get_mut(id)
    }

    /// List all registered plugins.
    pub fn list_plugins(&self) -> Vec<&PluginRecord> {
        self.plugins.values().collect()
    }

    /// List all active (loaded) plugins.
    pub fn list_active_plugins(&self) -> Vec<&PluginRecord> {
        self.plugins
            .values()
            .filter(|p| p.status.is_active())
            .collect()
    }

    /// Disable a plugin by ID.
    ///
    /// Returns `true` if the plugin was found and disabled, `false` otherwise.
    pub fn disable_plugin(&mut self, id: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.status = PluginStatus::Disabled;
            true
        } else {
            false
        }
    }

    /// Enable a previously disabled plugin by ID.
    ///
    /// Returns `true` if the plugin was found and enabled, `false` otherwise.
    /// Note: This does not re-run plugin initialization; it only changes the status.
    pub fn enable_plugin(&mut self, id: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(id) {
            if matches!(plugin.status, PluginStatus::Disabled) {
                plugin.status = PluginStatus::Loaded;
                return true;
            }
        }
        false
    }

    // =========================================================================
    // Tool Registration
    // =========================================================================

    /// Register a tool.
    ///
    /// The tool is stored by name, and the owning plugin's record is updated.
    pub fn register_tool(&mut self, tool: ToolRegistration) {
        let plugin_id = tool.plugin_id.clone();
        let tool_name = tool.name.clone();

        self.tools.insert(tool_name.clone(), tool);

        // Update plugin record
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            if !plugin.tool_names.contains(&tool_name) {
                plugin.tool_names.push(tool_name);
            }
        }
    }

    /// Get a tool by name.
    pub fn get_tool(&self, name: &str) -> Option<&ToolRegistration> {
        self.tools.get(name)
    }

    /// List all registered tools.
    pub fn list_tools(&self) -> Vec<&ToolRegistration> {
        self.tools.values().collect()
    }

    /// List tools from a specific plugin.
    pub fn list_tools_for_plugin(&self, plugin_id: &str) -> Vec<&ToolRegistration> {
        self.tools
            .values()
            .filter(|t| t.plugin_id == plugin_id)
            .collect()
    }

    // =========================================================================
    // Hook Registration
    // =========================================================================

    /// Register a hook.
    ///
    /// Hooks are maintained in priority order (lower priority value = earlier execution).
    pub fn register_hook(&mut self, hook: HookRegistration) {
        let plugin_id = hook.plugin_id.clone();

        self.hooks.push(hook);
        // Sort by priority (stable sort to preserve insertion order for equal priorities)
        self.hooks.sort_by_key(|h| h.priority);

        // Update plugin record
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            plugin.hook_count += 1;
        }
    }

    /// Get all hooks registered for a specific event.
    ///
    /// Returns hooks in priority order (lower priority = earlier in list).
    pub fn get_hooks_for_event(&self, event: PluginHookEvent) -> Vec<&HookRegistration> {
        self.hooks.iter().filter(|h| h.event == event).collect()
    }

    /// List all registered hooks.
    pub fn list_hooks(&self) -> Vec<&HookRegistration> {
        self.hooks.iter().collect()
    }

    // =========================================================================
    // Channel Registration
    // =========================================================================

    /// Register a channel.
    pub fn register_channel(&mut self, channel: ChannelRegistration) {
        let plugin_id = channel.plugin_id.clone();
        let channel_id = channel.id.clone();

        self.channels.insert(channel_id.clone(), channel);

        // Update plugin record
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            if !plugin.channel_ids.contains(&channel_id) {
                plugin.channel_ids.push(channel_id);
            }
        }
    }

    /// Get a channel by ID.
    pub fn get_channel(&self, id: &str) -> Option<&ChannelRegistration> {
        self.channels.get(id)
    }

    /// List all registered channels.
    pub fn list_channels(&self) -> Vec<&ChannelRegistration> {
        let mut channels: Vec<_> = self.channels.values().collect();
        // Sort by display order
        channels.sort_by_key(|c| c.order);
        channels
    }

    // =========================================================================
    // Provider Registration
    // =========================================================================

    /// Register a provider.
    pub fn register_provider(&mut self, provider: ProviderRegistration) {
        let plugin_id = provider.plugin_id.clone();
        let provider_id = provider.id.clone();

        self.providers.insert(provider_id.clone(), provider);

        // Update plugin record
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            if !plugin.provider_ids.contains(&provider_id) {
                plugin.provider_ids.push(provider_id);
            }
        }
    }

    /// Get a provider by ID.
    pub fn get_provider(&self, id: &str) -> Option<&ProviderRegistration> {
        self.providers.get(id)
    }

    /// List all registered providers.
    pub fn list_providers(&self) -> Vec<&ProviderRegistration> {
        self.providers.values().collect()
    }

    // =========================================================================
    // Gateway Method Registration
    // =========================================================================

    /// Register a gateway RPC method.
    pub fn register_gateway_method(&mut self, method: GatewayMethodRegistration) {
        let plugin_id = method.plugin_id.clone();
        let method_name = method.method.clone();

        self.gateway_methods.insert(method_name.clone(), method);

        // Update plugin record
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            if !plugin.gateway_methods.contains(&method_name) {
                plugin.gateway_methods.push(method_name);
            }
        }
    }

    /// Get a gateway method by name.
    pub fn get_gateway_method(&self, method: &str) -> Option<&GatewayMethodRegistration> {
        self.gateway_methods.get(method)
    }

    /// List all registered gateway methods.
    pub fn list_gateway_methods(&self) -> Vec<&GatewayMethodRegistration> {
        self.gateway_methods.values().collect()
    }

    // =========================================================================
    // HTTP Route Registration
    // =========================================================================

    /// Register an HTTP route.
    pub fn register_http_route(&mut self, route: HttpRouteRegistration) {
        self.http_routes.push(route);
    }

    /// List all registered HTTP routes.
    pub fn list_http_routes(&self) -> Vec<&HttpRouteRegistration> {
        self.http_routes.iter().collect()
    }

    /// Find HTTP routes matching a path pattern.
    pub fn find_http_routes(&self, path: &str) -> Vec<&HttpRouteRegistration> {
        self.http_routes.iter().filter(|r| r.path == path).collect()
    }

    // =========================================================================
    // HTTP Handler Registration
    // =========================================================================

    /// Register an HTTP handler/middleware.
    ///
    /// Handlers are maintained in priority order (lower priority = earlier execution).
    pub fn register_http_handler(&mut self, handler: HttpHandlerRegistration) {
        self.http_handlers.push(handler);
        // Sort by priority
        self.http_handlers.sort_by_key(|h| h.priority);
    }

    /// List all registered HTTP handlers in priority order.
    pub fn list_http_handlers(&self) -> Vec<&HttpHandlerRegistration> {
        self.http_handlers.iter().collect()
    }

    // =========================================================================
    // CLI Command Registration
    // =========================================================================

    /// Register a CLI command.
    pub fn register_cli_command(&mut self, cli: CliRegistration) {
        self.cli_commands.insert(cli.name.clone(), cli);
    }

    /// Get a CLI command by name.
    pub fn get_cli_command(&self, name: &str) -> Option<&CliRegistration> {
        self.cli_commands.get(name)
    }

    /// List all registered CLI commands.
    pub fn list_cli_commands(&self) -> Vec<&CliRegistration> {
        self.cli_commands.values().collect()
    }

    // =========================================================================
    // Service Registration
    // =========================================================================

    /// Register a background service.
    pub fn register_service(&mut self, service: ServiceRegistration) {
        let plugin_id = service.plugin_id.clone();
        let service_id = service.id.clone();

        self.services.insert(service_id.clone(), service);

        // Update plugin record
        if let Some(plugin) = self.plugins.get_mut(&plugin_id) {
            if !plugin.service_ids.contains(&service_id) {
                plugin.service_ids.push(service_id);
            }
        }
    }

    /// Get a service by ID.
    pub fn get_service(&self, id: &str) -> Option<&ServiceRegistration> {
        self.services.get(id)
    }

    /// List all registered services.
    pub fn list_services(&self) -> Vec<&ServiceRegistration> {
        self.services.values().collect()
    }

    // =========================================================================
    // In-Chat Command Registration
    // =========================================================================

    /// Register an in-chat command (e.g., /mycommand).
    pub fn register_command(&mut self, command: CommandRegistration) {
        self.commands.insert(command.name.clone(), command);
    }

    /// Get an in-chat command by name.
    pub fn get_command(&self, name: &str) -> Option<&CommandRegistration> {
        self.commands.get(name)
    }

    /// List all registered in-chat commands.
    pub fn list_commands(&self) -> Vec<&CommandRegistration> {
        self.commands.values().collect()
    }

    // =========================================================================
    // Diagnostics
    // =========================================================================

    /// Add a diagnostic message.
    pub fn add_diagnostic(&mut self, diagnostic: PluginDiagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Get all diagnostic messages.
    pub fn diagnostics(&self) -> &[PluginDiagnostic] {
        &self.diagnostics
    }

    /// Clear all diagnostic messages.
    pub fn clear_diagnostics(&mut self) {
        self.diagnostics.clear();
    }

    // =========================================================================
    // Unregistration
    // =========================================================================

    /// Unregister a plugin and all its associated components.
    ///
    /// This removes:
    /// - The plugin record
    /// - All tools registered by this plugin
    /// - All hooks registered by this plugin
    /// - All channels registered by this plugin
    /// - All providers registered by this plugin
    /// - All gateway methods registered by this plugin
    /// - All HTTP routes registered by this plugin
    /// - All HTTP handlers registered by this plugin
    /// - All CLI commands registered by this plugin
    /// - All services registered by this plugin
    /// - All in-chat commands registered by this plugin
    /// - All diagnostics from this plugin
    pub fn unregister_plugin(&mut self, plugin_id: &str) {
        // Remove the plugin record
        self.plugins.remove(plugin_id);

        // Remove all tools from this plugin
        self.tools.retain(|_, t| t.plugin_id != plugin_id);

        // Remove all hooks from this plugin
        self.hooks.retain(|h| h.plugin_id != plugin_id);

        // Remove all channels from this plugin
        self.channels.retain(|_, c| c.plugin_id != plugin_id);

        // Remove all providers from this plugin
        self.providers.retain(|_, p| p.plugin_id != plugin_id);

        // Remove all gateway methods from this plugin
        self.gateway_methods.retain(|_, m| m.plugin_id != plugin_id);

        // Remove all HTTP routes from this plugin
        self.http_routes.retain(|r| r.plugin_id != plugin_id);

        // Remove all HTTP handlers from this plugin
        self.http_handlers.retain(|h| h.plugin_id != plugin_id);

        // Remove all CLI commands from this plugin
        self.cli_commands.retain(|_, c| c.plugin_id != plugin_id);

        // Remove all services from this plugin
        self.services.retain(|_, s| s.plugin_id != plugin_id);

        // Remove all in-chat commands from this plugin
        self.commands.retain(|_, c| c.plugin_id != plugin_id);

        // Remove all diagnostics from this plugin
        self.diagnostics
            .retain(|d| d.plugin_id.as_deref() != Some(plugin_id));
    }

    // =========================================================================
    // Statistics
    // =========================================================================

    /// Get registry statistics.
    pub fn stats(&self) -> RegistryStats {
        RegistryStats {
            plugins: self.plugins.len(),
            active_plugins: self.list_active_plugins().len(),
            tools: self.tools.len(),
            hooks: self.hooks.len(),
            channels: self.channels.len(),
            providers: self.providers.len(),
            gateway_methods: self.gateway_methods.len(),
            http_routes: self.http_routes.len(),
            http_handlers: self.http_handlers.len(),
            cli_commands: self.cli_commands.len(),
            services: self.services.len(),
            commands: self.commands.len(),
            diagnostics: self.diagnostics.len(),
        }
    }
}

/// Registry statistics.
#[derive(Debug, Clone, Default)]
pub struct RegistryStats {
    pub plugins: usize,
    pub active_plugins: usize,
    pub tools: usize,
    pub hooks: usize,
    pub channels: usize,
    pub providers: usize,
    pub gateway_methods: usize,
    pub http_routes: usize,
    pub http_handlers: usize,
    pub cli_commands: usize,
    pub services: usize,
    pub commands: usize,
    pub diagnostics: usize,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
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
}
