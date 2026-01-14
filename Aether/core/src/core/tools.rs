//! Tool registry and dispatcher operations for AetherCore
//!
//! This module contains all tool registry methods:
//! - Tool listing and searching
//! - Tool registry refresh (full and scoped)
//! - Command completion
//! - Async confirmation handling
//! - Native tool execution (AgentTool)

use super::AetherCore;
use crate::error::Result;
use crate::mcp::{create_bridges, ExternalServerConfig};
use crate::tools::{
    create_clipboard_tools, create_filesystem_tools, create_git_tools, create_screen_tools,
    create_shell_tools, create_system_tools, create_web_tools, FilesystemConfig, GitConfig,
    ScreenConfig, ShellConfig, ToolResult, WebFetchConfig,
};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Scope for incremental tool registry refresh
///
/// Instead of clearing and rebuilding the entire registry, scoped refresh
/// only updates the affected tool sources, improving hot-reload performance.
#[derive(Debug, Clone)]
pub enum RefreshScope {
    /// Full refresh - clear and rebuild everything (used for initialization)
    Full,
    /// Refresh only skills (install/delete skill)
    SkillsOnly,
    /// Refresh only custom commands (update routing rules)
    CustomCommandsOnly,
    /// Refresh all MCP servers (add/delete external server)
    McpServersOnly,
    /// Refresh a specific MCP server (update server config)
    McpServer(String),
    /// Refresh only native tools (rarely needed)
    NativeToolsOnly,
}

impl AetherCore {
    // ========================================================================
    // Tool Registry Methods (implement-dispatcher-layer)
    // ========================================================================

    /// Refresh the unified tool registry (synchronous)
    ///
    /// This method aggregates tools from all sources:
    /// - Native AgentTools (filesystem, git, shell, system, clipboard, screen)
    /// - System tools (legacy MCP-style, from MCP client)
    /// - External MCP servers
    /// - Installed skills
    /// - Custom commands from config rules
    ///
    /// Call this method when:
    /// - Application starts (after initialization) - ensures tools available immediately
    ///
    /// For hot-reload scenarios (config changes, skill install/remove),
    /// use `refresh_tool_registry_background()` instead to avoid blocking.
    pub fn refresh_tool_registry(&self) {
        // Run refresh synchronously using block_on to ensure tools are available
        // immediately after AetherCore::new() returns. This is safe because
        // AetherCore initialization runs on a background thread.
        self.runtime.block_on(self.refresh_tool_registry_internal());
    }

    /// Refresh the unified tool registry in background (non-blocking)
    ///
    /// Use this method for hot-reload scenarios where blocking is undesirable:
    /// - Config file changes (hot-reload callback from ConfigWatcher)
    /// - MCP servers connect/disconnect
    /// - Skills are installed/removed
    ///
    /// This method returns immediately and spawns the refresh task on the
    /// tokio runtime. When complete, `on_tools_changed()` will be called
    /// to notify Swift.
    pub fn refresh_tool_registry_background(&self) {
        let registry = Arc::clone(&self.tool_registry);
        let native_registry = Arc::clone(&self.native_tool_registry);
        let config = self.lock_config().clone();
        let mcp_client = self.mcp_client.clone();
        let event_handler = Arc::clone(&self.event_handler);
        let intent_pipeline = self.intent_pipeline.clone();
        let unified_executor = Arc::clone(&self.unified_executor);

        // Spawn refresh task on the runtime - returns immediately
        self.runtime.spawn(async move {
            Self::refresh_tool_registry_impl(
                registry,
                native_registry,
                config,
                mcp_client,
                event_handler,
                intent_pipeline,
                unified_executor,
            )
            .await;
        });

        debug!("Tool registry background refresh initiated");
    }

    /// Scoped refresh - update only specific tool sources (non-blocking)
    ///
    /// This method provides incremental updates instead of full refresh,
    /// improving hot-reload performance by only affecting changed tools.
    ///
    /// # Arguments
    /// * `scope` - The scope of tools to refresh
    ///
    /// # Usage
    /// ```rust,ignore
    /// // After installing a skill
    /// core.refresh_tool_registry_scoped(RefreshScope::SkillsOnly);
    ///
    /// // After updating routing rules
    /// core.refresh_tool_registry_scoped(RefreshScope::CustomCommandsOnly);
    /// ```
    pub fn refresh_tool_registry_scoped(&self, scope: RefreshScope) {
        // For full refresh, delegate to existing method
        if matches!(scope, RefreshScope::Full) {
            self.refresh_tool_registry_background();
            return;
        }

        let registry = Arc::clone(&self.tool_registry);
        let native_registry = Arc::clone(&self.native_tool_registry);
        let config = self.lock_config().clone();
        let mcp_client = self.mcp_client.clone();
        let event_handler = Arc::clone(&self.event_handler);
        let intent_pipeline = self.intent_pipeline.clone();
        let unified_executor = Arc::clone(&self.unified_executor);

        // Spawn scoped refresh task on the runtime
        self.runtime.spawn(async move {
            Self::refresh_tool_registry_scoped_impl(
                scope,
                registry,
                native_registry,
                config,
                mcp_client,
                event_handler,
                intent_pipeline,
                unified_executor,
            )
            .await;
        });

        debug!("Tool registry scoped refresh initiated");
    }

    /// Scoped refresh implementation
    ///
    /// Performs incremental updates based on the specified scope.
    async fn refresh_tool_registry_scoped_impl(
        scope: RefreshScope,
        registry: Arc<crate::dispatcher::ToolRegistry>,
        native_registry: Arc<crate::tools::NativeToolRegistry>,
        config: crate::Config,
        mcp_client: Option<Arc<crate::mcp::McpClient>>,
        event_handler: Arc<dyn crate::event_handler::InternalEventHandler>,
        intent_pipeline: Option<Arc<crate::routing::IntentRoutingPipeline>>,
        unified_executor: Arc<super::tool_executor::UnifiedToolExecutor>,
    ) {
        match scope {
            RefreshScope::Full => {
                // Should not reach here, but handle gracefully
                Self::refresh_tool_registry_impl(
                    registry,
                    native_registry,
                    config,
                    mcp_client,
                    event_handler,
                    intent_pipeline,
                    unified_executor,
                )
                .await;
            }

            RefreshScope::SkillsOnly => {
                // 1. Remove existing skills from registry
                let removed = registry.remove_skills().await;
                debug!("Removed {} skill tools", removed);

                // 2. Re-register skills
                match crate::initialization::list_installed_skills() {
                    Ok(skills) => {
                        registry.register_skills(&skills).await;
                        debug!("Re-registered {} skills", skills.len());
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to list installed skills during scoped refresh");
                    }
                }

                // 3. Update intent pipeline and notify
                Self::finalize_scoped_refresh(&registry, &intent_pipeline, &event_handler, &unified_executor).await;
            }

            RefreshScope::CustomCommandsOnly => {
                // 1. Remove existing custom commands from registry
                let removed = registry.remove_custom_commands().await;
                debug!("Removed {} custom commands", removed);

                // 2. Re-register custom commands from config rules
                registry.register_custom_commands(&config.rules).await;
                debug!("Re-registered custom commands from {} rules", config.rules.len());

                // 3. Update intent pipeline and notify
                Self::finalize_scoped_refresh(&registry, &intent_pipeline, &event_handler, &unified_executor).await;
            }

            RefreshScope::McpServersOnly => {
                // 1. Remove all MCP tools from both registries
                let removed_dispatcher = registry.remove_all_mcp_tools().await;
                let removed_native = native_registry.remove_mcp_tools().await;
                debug!(
                    "Removed {} MCP tools from dispatcher, {} from native registry",
                    removed_dispatcher, removed_native
                );

                // 2. Restart all MCP servers and re-register tools
                if let Some(ref client) = mcp_client {
                    // Stop all existing servers first
                    let _ = client.stop_all().await;

                    // Convert config to ExternalServerConfig
                    let server_configs: Vec<ExternalServerConfig> = config
                        .mcp
                        .external_servers
                        .iter()
                        .map(|s| ExternalServerConfig {
                            name: s.name.clone(),
                            command: s.command.clone(),
                            args: s.args.clone(),
                            env: s.env.clone(),
                            cwd: s.cwd.as_ref().map(PathBuf::from),
                            requires_runtime: s.requires_runtime.clone(),
                            timeout_seconds: Some(s.timeout_seconds),
                        })
                        .collect();

                    if !server_configs.is_empty() {
                        // Start external servers concurrently
                        let startup_report = client.start_external_servers(server_configs).await;

                        // Log startup results
                        if !startup_report.failed.is_empty() {
                            for (server_name, error_msg) in &startup_report.failed {
                                warn!(
                                    server = %server_name,
                                    error = %error_msg,
                                    "Failed to start MCP server during scoped refresh"
                                );
                            }
                        }
                        info!(
                            succeeded = startup_report.succeeded.len(),
                            failed = startup_report.failed.len(),
                            "MCP servers restart complete"
                        );

                        // Notify Swift about MCP startup results
                        let ffi_report = crate::McpStartupReportFFI::from_internal(&startup_report);
                        event_handler.on_mcp_startup_complete(ffi_report);

                        // Create bridges and register tools
                        let mcp_bridges = create_bridges(client).await;
                        let mcp_tool_count = mcp_bridges.len();

                        if mcp_tool_count > 0 {
                            native_registry.register_all(mcp_bridges.clone()).await;

                            let mcp_groups = Self::group_mcp_tools_by_server(&mcp_bridges);
                            for (server_name, tools) in mcp_groups {
                                registry.register_agent_tools(&tools, &server_name).await;
                            }

                            debug!("Re-registered {} MCP tools", mcp_tool_count);
                        }
                    }
                }

                // 3. Update intent pipeline and notify
                Self::finalize_scoped_refresh(&registry, &intent_pipeline, &event_handler, &unified_executor).await;
            }

            RefreshScope::McpServer(server_name) => {
                // 1. Remove tools for the specific MCP server
                let removed_dispatcher = registry.remove_by_mcp_server(&server_name).await;
                let removed_native = native_registry.remove_by_name_prefix(&format!("{}:", server_name)).await;
                debug!(
                    "Removed {} tools for MCP server '{}' ({} from native)",
                    removed_dispatcher, server_name, removed_native
                );

                // 2. Find and restart only the specified server
                if let Some(ref client) = mcp_client {
                    // Find the server config
                    if let Some(server_config) = config
                        .mcp
                        .external_servers
                        .iter()
                        .find(|s| s.name == server_name)
                    {
                        // Stop the specific server
                        client.stop_server(&server_name).await;

                        // Restart with new config
                        let ext_config = ExternalServerConfig {
                            name: server_config.name.clone(),
                            command: server_config.command.clone(),
                            args: server_config.args.clone(),
                            env: server_config.env.clone(),
                            cwd: server_config.cwd.as_ref().map(PathBuf::from),
                            requires_runtime: server_config.requires_runtime.clone(),
                            timeout_seconds: Some(server_config.timeout_seconds),
                        };

                        match client.start_external_server(ext_config).await {
                            Ok(()) => {
                                info!(server = %server_name, "MCP server restarted successfully");

                                // Re-register tools for this server
                                let mcp_bridges = create_bridges(client).await;
                                let server_tools: Vec<_> = mcp_bridges
                                    .into_iter()
                                    .filter(|t| t.name().starts_with(&format!("{}:", server_name)))
                                    .collect();

                                if !server_tools.is_empty() {
                                    native_registry.register_all(server_tools.clone()).await;
                                    registry.register_agent_tools(&server_tools, &server_name).await;
                                    debug!(
                                        "Re-registered {} tools for MCP server '{}'",
                                        server_tools.len(),
                                        server_name
                                    );
                                }

                                // Notify Swift about single server restart success
                                let ffi_report = crate::McpStartupReportFFI {
                                    succeeded_servers: vec![server_name.clone()],
                                    failed_servers: vec![],
                                };
                                event_handler.on_mcp_startup_complete(ffi_report);
                            }
                            Err(e) => {
                                warn!(
                                    server = %server_name,
                                    error = %e,
                                    "Failed to restart MCP server"
                                );

                                // Notify Swift about single server restart failure
                                let ffi_report = crate::McpStartupReportFFI {
                                    succeeded_servers: vec![],
                                    failed_servers: vec![crate::McpServerErrorFFI {
                                        server_name: server_name.clone(),
                                        error_message: e.to_string(),
                                    }],
                                };
                                event_handler.on_mcp_startup_complete(ffi_report);
                            }
                        }
                    } else {
                        // Server was deleted, tools already removed
                        info!(server = %server_name, "MCP server config not found (may have been deleted)");
                    }
                }

                // 3. Update intent pipeline and notify
                Self::finalize_scoped_refresh(&registry, &intent_pipeline, &event_handler, &unified_executor).await;
            }

            RefreshScope::NativeToolsOnly => {
                // 1. Remove native tools from both registries
                let removed_dispatcher = registry.remove_native_tools().await;
                native_registry.clear().await;
                debug!("Removed {} native tools from dispatcher", removed_dispatcher);

                // 2. Re-create and register native tools
                let native_tools = Self::create_native_agent_tools();
                let native_tool_count = native_registry.register_all(native_tools.clone()).await;

                for (service_name, tools) in Self::group_native_tools_by_service(&native_tools) {
                    registry.register_agent_tools(&tools, &service_name).await;
                }
                debug!("Re-registered {} native AgentTools", native_tool_count);

                // 3. Update intent pipeline and notify
                Self::finalize_scoped_refresh(&registry, &intent_pipeline, &event_handler, &unified_executor).await;
            }
        }
    }

    /// Finalize scoped refresh - update intent pipeline, executor and notify Swift
    async fn finalize_scoped_refresh(
        registry: &Arc<crate::dispatcher::ToolRegistry>,
        intent_pipeline: &Option<Arc<crate::routing::IntentRoutingPipeline>>,
        event_handler: &Arc<dyn crate::event_handler::InternalEventHandler>,
        unified_executor: &Arc<super::tool_executor::UnifiedToolExecutor>,
    ) {
        let tool_count = registry.active_count().await;

        // Update intent pipeline with the new tool list
        if let Some(ref pipeline) = intent_pipeline {
            let tools = registry.list_all().await;
            pipeline.update_tools(tools).await;
            debug!("Intent pipeline updated with {} tools after scoped refresh", tool_count);
        }

        // Refresh unified executor tool source cache
        unified_executor.refresh_tool_sources().await;
        debug!("Unified executor refreshed after scoped refresh");

        // Notify Swift that tools have changed
        event_handler.on_tools_changed(tool_count as u32);
        info!("Scoped refresh complete: {} total tools", tool_count);
    }

    /// Internal async refresh implementation
    ///
    /// This is the actual refresh logic used by both sync and async paths.
    async fn refresh_tool_registry_internal(&self) {
        let registry = Arc::clone(&self.tool_registry);
        let native_registry = Arc::clone(&self.native_tool_registry);
        let config = self.lock_config().clone();
        let mcp_client = self.mcp_client.clone();
        let event_handler = Arc::clone(&self.event_handler);
        let intent_pipeline = self.intent_pipeline.clone();
        let unified_executor = Arc::clone(&self.unified_executor);

        Self::refresh_tool_registry_impl(
            registry,
            native_registry,
            config,
            mcp_client,
            event_handler,
            intent_pipeline,
            unified_executor,
        )
        .await;
    }

    /// Shared implementation for tool registry refresh
    ///
    /// Takes all dependencies as parameters to allow usage from both
    /// synchronous (block_on) and background (spawn) contexts.
    async fn refresh_tool_registry_impl(
        registry: Arc<crate::dispatcher::ToolRegistry>,
        native_registry: Arc<crate::tools::NativeToolRegistry>,
        config: crate::Config,
        mcp_client: Option<Arc<crate::mcp::McpClient>>,
        event_handler: Arc<dyn crate::event_handler::InternalEventHandler>,
        intent_pipeline: Option<Arc<crate::routing::IntentRoutingPipeline>>,
        unified_executor: Arc<super::tool_executor::UnifiedToolExecutor>,
    ) {
        // 0. Clear existing tools
        registry.clear().await;
        native_registry.clear().await;

        // 1. Register builtin commands (single source of truth)
        registry.register_builtin_tools().await;

        // 2. Register native AgentTools for execution
        // These are the new native function calling tools
        let native_tools = Self::create_native_agent_tools();
        let native_tool_count = native_registry.register_all(native_tools.clone()).await;
        debug!("Registered {} native AgentTools for execution", native_tool_count);

        // 2b. Also register native tools in the dispatcher registry for UI display
        for (service_name, tools) in Self::group_native_tools_by_service(&native_tools) {
            registry.register_agent_tools(&tools, &service_name).await;
        }

        // 3. Start external MCP servers and register their tools
        let mut mcp_tool_count = 0usize;
        if let Some(ref client) = mcp_client {
            // Convert config to ExternalServerConfig
            let server_configs: Vec<ExternalServerConfig> = config
                .mcp
                .external_servers
                .iter()
                .map(|s| ExternalServerConfig {
                    name: s.name.clone(),
                    command: s.command.clone(),
                    args: s.args.clone(),
                    env: s.env.clone(),
                    cwd: s.cwd.as_ref().map(PathBuf::from),
                    requires_runtime: s.requires_runtime.clone(),
                    timeout_seconds: Some(s.timeout_seconds),
                })
                .collect();

            if !server_configs.is_empty() {
                // Start external servers concurrently
                let startup_report = client.start_external_servers(server_configs).await;

                // Log startup results
                if !startup_report.failed.is_empty() {
                    for (server_name, error_msg) in &startup_report.failed {
                        warn!(
                            server = %server_name,
                            error = %error_msg,
                            "Failed to start MCP server"
                        );
                    }
                }
                info!(
                    succeeded = startup_report.succeeded.len(),
                    failed = startup_report.failed.len(),
                    "MCP servers startup complete"
                );

                // Notify Swift about MCP startup results
                let ffi_report = crate::McpStartupReportFFI::from_internal(&startup_report);
                event_handler.on_mcp_startup_complete(ffi_report);

                // Create bridges for MCP tools and register them
                let mcp_bridges = create_bridges(client).await;
                mcp_tool_count = mcp_bridges.len();

                if mcp_tool_count > 0 {
                    // Register MCP tools in native registry for execution
                    native_registry.register_all(mcp_bridges.clone()).await;

                    // Group MCP tools by server name for proper categorization
                    // MCP tool names have format "server_name:tool_name"
                    let mcp_groups = Self::group_mcp_tools_by_server(&mcp_bridges);
                    for (server_name, tools) in mcp_groups {
                        registry.register_agent_tools(&tools, &server_name).await;
                    }

                    debug!("Registered {} MCP tools from external servers", mcp_tool_count);
                }
            }
        }

        // 4. Register skills
        match crate::initialization::list_installed_skills() {
            Ok(skills) => {
                registry.register_skills(&skills).await;
            }
            Err(e) => {
                warn!(error = %e, "Failed to list installed skills, skipping skill registration");
            }
        }

        // 5. Register custom commands from routing rules
        registry.register_custom_commands(&config.rules).await;

        let tool_count = registry.active_count().await;
        info!(
            "Tool registry refreshed: {} tools ({} native, {} mcp)",
            tool_count, native_tool_count, mcp_tool_count
        );

        // 6. Update intent pipeline with the new tool list
        // This enables L3 LLM routing to be aware of available tools
        if let Some(ref pipeline) = intent_pipeline {
            let tools = registry.list_all().await;
            pipeline.update_tools(tools).await;
            debug!("Intent pipeline updated with {} tools", tool_count);
        }

        // 7. Refresh unified executor tool source cache
        // This ensures the executor can route tools to correct backends
        unified_executor.refresh_tool_sources().await;
        debug!("Unified executor tool sources refreshed");

        // Notify Swift that tools have changed
        event_handler.on_tools_changed(tool_count as u32);
    }

    /// Create all native AgentTool instances
    ///
    /// Returns a flat list of all native tools with default configurations.
    /// These tools can be executed via `execute_native_tool()`.
    fn create_native_agent_tools() -> Vec<Arc<dyn crate::tools::AgentTool>> {
        let mut all_tools: Vec<Arc<dyn crate::tools::AgentTool>> = Vec::new();

        // Filesystem tools - allow access to home directory by default
        let fs_config = FilesystemConfig::with_home_dir();
        all_tools.extend(create_filesystem_tools(fs_config));

        // Git tools - allow all repositories by default (empty list = all allowed)
        let git_config = GitConfig::default();
        all_tools.extend(create_git_tools(git_config));

        // Shell tools - disabled by default for security
        // Users must explicitly configure allowed commands
        let shell_config = ShellConfig::default(); // Disabled by default
        all_tools.extend(create_shell_tools(shell_config));

        // System info tools
        all_tools.extend(create_system_tools());

        // Clipboard tools (returns tuple, extract just the tools)
        let (clipboard_tools, _ctx) = create_clipboard_tools();
        all_tools.extend(clipboard_tools);

        // Screen capture tools
        let screen_config = ScreenConfig::default();
        all_tools.extend(create_screen_tools(screen_config));

        // Web fetch tools (for fetching and extracting web page content)
        let web_config = WebFetchConfig::default();
        all_tools.extend(create_web_tools(web_config));

        all_tools
    }

    /// Group native tools by service name for dispatcher registration
    fn group_native_tools_by_service(
        tools: &[Arc<dyn crate::tools::AgentTool>],
    ) -> Vec<(String, Vec<Arc<dyn crate::tools::AgentTool>>)> {
        use std::collections::HashMap;

        let mut groups: HashMap<String, Vec<Arc<dyn crate::tools::AgentTool>>> = HashMap::new();

        for tool in tools {
            let service_name = match tool.category() {
                crate::tools::ToolCategory::Builtin => "builtin",
                crate::tools::ToolCategory::Native => "native",
                crate::tools::ToolCategory::Skills => "skills",
                crate::tools::ToolCategory::Mcp => "mcp",
                crate::tools::ToolCategory::Custom => "custom",
            };

            groups
                .entry(service_name.to_string())
                .or_default()
                .push(Arc::clone(tool));
        }

        groups.into_iter().collect()
    }

    /// Group MCP tools by server name for dispatcher registration
    ///
    /// MCP tool names have the format "server_name:tool_name".
    /// This function extracts the server name and groups tools accordingly.
    fn group_mcp_tools_by_server(
        tools: &[Arc<dyn crate::tools::AgentTool>],
    ) -> Vec<(String, Vec<Arc<dyn crate::tools::AgentTool>>)> {
        use std::collections::HashMap;

        let mut groups: HashMap<String, Vec<Arc<dyn crate::tools::AgentTool>>> = HashMap::new();

        for tool in tools {
            // Extract server name from tool name (format: "server_name:tool_name")
            let server_name = tool
                .name()
                .split(':')
                .next()
                .unwrap_or("unknown")
                .to_string();

            groups
                .entry(server_name)
                .or_default()
                .push(Arc::clone(tool));
        }

        groups.into_iter().collect()
    }

    /// Get all registered tools from the unified registry
    ///
    /// Returns a list of all active tools for UI display or
    /// prompt generation.
    pub fn list_unified_tools(&self) -> Vec<crate::dispatcher::UnifiedTool> {
        self.runtime
            .block_on(async { self.tool_registry.list_all().await })
    }

    /// Search tools by name or description
    ///
    /// # Arguments
    /// * `query` - Search query string
    ///
    /// # Returns
    /// * `Vec<UnifiedTool>` - Matching tools sorted by relevance
    pub fn search_unified_tools(&self, query: &str) -> Vec<crate::dispatcher::UnifiedTool> {
        self.runtime
            .block_on(async { self.tool_registry.search(query).await })
    }

    /// Get tool registry prompt block for L3 routing
    ///
    /// Returns a markdown-formatted list of all active tools
    /// suitable for injection into the router LLM system prompt.
    pub fn get_tool_prompt_block(&self) -> String {
        self.runtime
            .block_on(async { self.tool_registry.to_prompt_block().await })
    }

    // ========================================================================
    // Dispatcher Layer FFI Methods (UniFFI exports)
    // ========================================================================

    /// List all available tools from unified registry
    ///
    /// Returns tools from all sources: Native, MCP, Skills, Custom.
    /// This is the UniFFI-exposed method that returns FFI-safe types.
    pub fn list_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.list_unified_tools()
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// List builtin tools only
    ///
    /// Returns the 3 system builtin commands (/search, /youtube, /webfetch)
    /// sorted by sort_order.
    pub fn list_builtin_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.runtime
            .block_on(async { self.tool_registry.list_builtin_tools().await })
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// List preset tools for Settings UI (Flat Namespace Mode)
    ///
    /// Returns all non-Custom tools: Builtin + MCP + Skill + Native
    /// These are the "preset" tools displayed in Settings > Routing.
    pub fn list_preset_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.runtime
            .block_on(async { self.tool_registry.list_preset_tools().await })
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// List root-level commands for command completion
    ///
    /// Returns builtin commands + custom commands (but not nested MCP/Skill tools),
    /// sorted by sort_order then alphabetically.
    pub fn list_root_tools(&self) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.runtime
            .block_on(async { self.tool_registry.list_root_commands().await })
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// List tools filtered by source type
    ///
    /// # Arguments
    /// * `source_type` - Filter by this source type
    ///
    /// # Returns
    /// * `Vec<UnifiedToolInfo>` - Matching tools
    pub fn list_tools_by_source(
        &self,
        source_type: crate::dispatcher::ToolSourceType,
    ) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.list_unified_tools()
            .into_iter()
            .filter(|tool| crate::dispatcher::ToolSourceType::from(&tool.source) == source_type)
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    /// Search tools by name or description
    ///
    /// # Arguments
    /// * `query` - Search query string
    ///
    /// # Returns
    /// * `Vec<UnifiedToolInfo>` - Matching tools sorted by relevance
    pub fn search_tools(&self, query: String) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        self.search_unified_tools(&query)
            .into_iter()
            .map(crate::dispatcher::UnifiedToolInfo::from)
            .collect()
    }

    // ========================================================================
    // NATIVE TOOL EXECUTION (native-function-calling)
    // ========================================================================

    /// Execute a native tool by name with JSON arguments
    ///
    /// This method executes a tool registered in the NativeToolRegistry.
    /// Use this for executing AgentTool implementations with typed parameters.
    ///
    /// # Arguments
    ///
    /// * `name` - Tool name (e.g., "file_read", "git_status")
    /// * `args` - JSON string containing tool parameters
    ///
    /// # Returns
    ///
    /// * `Ok(ToolResult)` - Execution result with success/error status
    /// * `Err(AetherError::ToolNotFound)` - Tool not registered
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = core.execute_native_tool(
    ///     "file_read".to_string(),
    ///     r#"{"path": "/tmp/test.txt"}"#.to_string()
    /// )?;
    /// ```
    pub fn execute_native_tool(&self, name: String, args: String) -> Result<ToolResult> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.execute(&name, &args).await
        })
    }

    /// Check if a native tool requires confirmation before execution
    ///
    /// # Arguments
    ///
    /// * `name` - Tool name to check
    ///
    /// # Returns
    ///
    /// * `Some(true)` - Tool requires confirmation
    /// * `Some(false)` - Tool does not require confirmation
    /// * `None` - Tool not found
    pub fn native_tool_requires_confirmation(&self, name: String) -> Option<bool> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.requires_confirmation(&name).await
        })
    }

    /// Get all native tool definitions for LLM prompt generation
    ///
    /// Returns definitions sorted by category then name.
    /// These can be converted to OpenAI/Anthropic tool format.
    pub fn get_native_tool_definitions(&self) -> Vec<crate::tools::ToolDefinition> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.get_definitions().await
        })
    }

    /// Get native tools in OpenAI function calling format
    ///
    /// Returns a JSON array suitable for the OpenAI API `tools` parameter.
    pub fn get_native_tools_openai(&self) -> Vec<serde_json::Value> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.to_openai_tools().await
        })
    }

    /// Get native tools in Anthropic tool format
    ///
    /// Returns a JSON array suitable for the Anthropic API `tools` parameter.
    pub fn get_native_tools_anthropic(&self) -> Vec<serde_json::Value> {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.to_anthropic_tools().await
        })
    }

    /// Get count of registered native tools
    pub fn native_tool_count(&self) -> u32 {
        let registry = Arc::clone(&self.native_tool_registry);

        self.runtime.block_on(async move {
            registry.count().await as u32
        })
    }

    // ========================================================================
    // ASYNC CONFIRMATION METHODS (async-confirmation-flow)
    // ========================================================================

    /// Confirm or cancel a pending confirmation
    ///
    /// Called by Swift when user makes a decision on pending confirmation.
    /// Returns true if confirmation was found and processed, false if not found/expired.
    pub fn confirm_action(
        &self,
        confirmation_id: String,
        decision: crate::dispatcher::UserConfirmationDecision,
    ) -> Result<bool> {
        use crate::dispatcher::ConfirmationState;

        let state = self
            .async_confirmation
            .resume_with_decision(&confirmation_id, decision);

        match state {
            ConfirmationState::Confirmed {
                tool,
                parameters: _,
                routing_layer,
                confidence,
            } => {
                info!(
                    tool = %tool.name,
                    confidence = confidence,
                    layer = ?routing_layer,
                    "Confirmation confirmed, executing tool"
                );
                // TODO: Execute the tool asynchronously
                // For now, just log and return success
                Ok(true)
            }
            ConfirmationState::Cancelled { reason } => {
                info!(reason = %reason, "Confirmation cancelled");
                Ok(true)
            }
            ConfirmationState::TimedOut { confirmation_id } => {
                warn!(id = %confirmation_id, "Confirmation timed out");
                self.event_handler.on_confirmation_expired(confirmation_id);
                Ok(false)
            }
            ConfirmationState::NotRequired | ConfirmationState::Pending(_) => {
                // Should not happen in resume_with_decision
                Ok(false)
            }
        }
    }

    /// Cancel a pending confirmation by ID
    ///
    /// Returns true if confirmation was found and cancelled.
    pub fn cancel_confirmation(&self, confirmation_id: String) -> bool {
        self.async_confirmation.cancel(&confirmation_id)
    }

    /// Get pending confirmation by ID (if still valid)
    pub fn get_pending_confirmation(
        &self,
        confirmation_id: String,
    ) -> Option<crate::dispatcher::PendingConfirmationInfo> {
        self.async_confirmation
            .get_pending(&confirmation_id)
            .map(|p| p.to_ffi())
    }

    /// Get count of pending confirmations
    pub fn get_pending_confirmation_count(&self) -> u32 {
        self.async_confirmation.pending_count() as u32
    }

    /// Cleanup expired confirmations
    ///
    /// Returns count of cleaned confirmations.
    pub fn cleanup_expired_confirmations(&self) -> u32 {
        self.async_confirmation.cleanup_expired().len() as u32
    }

    // ========================================================================
    // COMMAND COMPLETION METHODS (add-command-completion-system)
    // ========================================================================

    /// Get all root-level commands for command completion UI
    ///
    /// Returns commands parsed from config.toml routing rules with ^/ prefix.
    /// Commands are sorted alphabetically by key.
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - List of root commands with key, description, icon, hint, type
    pub fn get_root_commands(&self) -> Vec<crate::command::CommandNode> {
        let config = self.lock_config();
        let language = config.general.language.as_deref().unwrap_or("en");
        let registry = crate::command::CommandRegistry::from_config(&config, language);

        // NOTE: Flat namespace mode - skills are registered directly in ToolRegistry
        // No longer inject /skill namespace here

        registry.get_root_commands()
    }

    /// Get children of a namespace command
    ///
    /// For namespace commands like /mcp, returns the list of child commands.
    /// Currently returns empty for most namespaces (MCP integration reserved for future).
    ///
    /// # Arguments
    /// * `parent_key` - The key of the parent namespace (e.g., "mcp")
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - List of child commands
    pub fn get_command_children(&self, parent_key: String) -> Vec<crate::command::CommandNode> {
        let config = self.lock_config();
        let language = config.general.language.as_deref().unwrap_or("en");
        let registry = crate::command::CommandRegistry::from_config(&config, language);

        // NOTE: Flat namespace mode - no namespace subcommands
        // /mcp and /skill are no longer supported as namespaces

        registry.get_children(&parent_key)
    }

    /// Filter commands by key prefix (case-insensitive)
    ///
    /// Used for autocomplete as user types. Returns commands whose keys
    /// start with the given prefix.
    ///
    /// # Arguments
    /// * `prefix` - The prefix to filter by (e.g., "se" matches "search", "settings")
    ///
    /// # Returns
    /// * `Vec<CommandNode>` - Filtered list of matching commands
    pub fn filter_commands(&self, prefix: String) -> Vec<crate::command::CommandNode> {
        let commands = self.get_root_commands();
        crate::command::CommandRegistry::filter_by_prefix(&commands, &prefix)
    }

    // ========================================================================
    // TOOL REGISTRY-BASED COMMAND COMPLETION (unify-tool-registry)
    // ========================================================================

    /// Get subtools for a namespace command from ToolRegistry
    ///
    /// For commands like /mcp and /skill that have dynamic subtools,
    /// this queries the ToolRegistry to get the list of available subtools.
    ///
    /// # Arguments
    /// * `parent_key` - The key of the parent command (e.g., "mcp", "skill")
    ///
    /// # Returns
    /// * `Vec<UnifiedToolInfo>` - List of subtools, or empty if none
    pub fn get_subtools_from_registry(&self, parent_key: String) -> Vec<crate::dispatcher::UnifiedToolInfo> {
        use crate::dispatcher::ToolSourceType;

        match parent_key.as_str() {
            "mcp" => {
                // Return all MCP tools
                self.list_tools_by_source(ToolSourceType::Mcp)
            }
            "skill" => {
                // Return all skill tools
                self.list_tools_by_source(ToolSourceType::Skill)
            }
            _ => Vec::new(),
        }
    }

    /// Convert UnifiedToolInfo to CommandNode for UI compatibility (Flat Namespace Mode)
    ///
    /// This allows the command completion UI to use ToolRegistry data.
    /// In flat namespace mode:
    /// - All tools are root-level commands (has_children is always false)
    /// - source_type indicates tool origin for badge display
    pub fn tool_to_command_node(tool: &crate::dispatcher::UnifiedToolInfo) -> crate::command::CommandNode {
        use crate::command::CommandNode;

        // In flat namespace mode, use new_with_source for proper source_type
        let mut node = CommandNode::new_with_source(
            tool.name.clone(),
            tool.description.clone(),
            tool.source_type,
        );

        if let Some(icon) = &tool.icon {
            node = node.with_icon(icon.clone());
        }

        // Use id for source_id tracking
        node = node.with_source_id(tool.id.clone());

        // In flat namespace mode, has_children is always false
        // (MCP subtools are registered as separate root commands)
        node.has_children = false;

        node
    }

    /// Get command completions from ToolRegistry
    ///
    /// Returns all root-level commands as CommandNode for UI display.
    /// This is an alternative to get_root_commands() that uses ToolRegistry as source.
    pub fn get_root_commands_from_registry(&self) -> Vec<crate::command::CommandNode> {
        self.list_root_tools()
            .iter()
            .map(Self::tool_to_command_node)
            .collect()
    }

    /// Get subcommand completions from ToolRegistry
    ///
    /// Returns subcommands for a namespace command as CommandNode for UI display.
    /// This is an alternative to get_command_children() that uses ToolRegistry as source.
    pub fn get_subcommands_from_registry(&self, parent_key: String) -> Vec<crate::command::CommandNode> {
        self.get_subtools_from_registry(parent_key)
            .iter()
            .map(Self::tool_to_command_node)
            .collect()
    }
}
