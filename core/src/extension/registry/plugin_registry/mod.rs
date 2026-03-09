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
    ProviderRegistration, ServiceRegistration, ToolRegistration,
};
use crate::extension::types::{HookEvent, PluginRecord, PluginStatus};

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
    pub fn get_hooks_for_event(&self, event: HookEvent) -> Vec<&HookRegistration> {
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
