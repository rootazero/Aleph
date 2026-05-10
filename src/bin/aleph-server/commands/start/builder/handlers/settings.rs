use super::*;

pub(in crate::commands::start) fn register_workspace_handlers(
    server: &mut GatewayServer,
    workspace_manager: &Arc<AgentEnvStore>,
    _memory_db: &MemoryBackend,
    daemon: bool,
) {
    register_handler!(
        server,
        "workspace.create",
        workspace_handlers::handle_create,
        workspace_manager
    );
    register_handler!(
        server,
        "workspace.list",
        workspace_handlers::handle_list,
        workspace_manager
    );
    register_handler!(
        server,
        "workspace.get",
        workspace_handlers::handle_get,
        workspace_manager
    );
    register_handler!(
        server,
        "workspace.update",
        workspace_handlers::handle_update,
        workspace_manager
    );
    register_handler!(
        server,
        "workspace.archive",
        workspace_handlers::handle_archive,
        workspace_manager
    );
    register_handler!(
        server,
        "channels.set_agent",
        workspace_handlers::handle_set_agent,
        workspace_manager
    );
    register_handler!(
        server,
        "agents.bindings",
        workspace_handlers::handle_agent_bindings,
        workspace_manager
    );

    if !daemon {
        println!("Workspace methods:");
        println!("  - workspace.create    : Create a new workspace");
        println!("  - workspace.list      : List all workspaces");
        println!("  - workspace.get       : Get workspace by ID");
        println!("  - workspace.update    : Update workspace metadata");
        println!("  - workspace.archive   : Archive (soft-delete) a workspace");
        println!("  - channels.set_agent  : Bind or unbind an agent to a channel");
        println!("  - agents.bindings     : Get all agent->channel bindings");
        println!();
    }
}

// ─── register_config_handlers ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) fn register_config_handlers(
    server: &mut GatewayServer,
    config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    config_patcher: Arc<alephcore::ConfigPatcher>,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    device_store: Arc<alephcore::gateway::device_store::DeviceStore>,
    multi_registry: Option<Arc<alephcore::MultiProviderRegistry>>,
    shared_token_mgr: Arc<alephcore::gateway::security::SharedTokenManager>,
    acp_manager: Option<Arc<alephcore::acp::manager::AcpAdapterManager>>,
) {
    use alephcore::gateway::handlers::agent_config;
    use alephcore::gateway::handlers::behavior_config;
    use alephcore::gateway::handlers::browser_config;
    use alephcore::gateway::handlers::config::{handle_get_full_config, handle_patch_config};
    use alephcore::gateway::handlers::embedding_providers;
    use alephcore::gateway::handlers::execution_config;
    use alephcore::gateway::handlers::general_config;
    use alephcore::gateway::handlers::generation_config;
    use alephcore::gateway::handlers::generation_providers;
    use alephcore::gateway::handlers::mcp_config;
    use alephcore::gateway::handlers::memory_config;
    use alephcore::gateway::handlers::providers;
    use alephcore::gateway::handlers::rerank_config;
    use alephcore::gateway::handlers::routing_rules;
    use alephcore::gateway::handlers::search_config;
    use alephcore::gateway::handlers::security_config;

    // Config CRUD
    register_handler!(
        server,
        "config.get",
        handle_get_full_config,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "config.patch",
        handle_patch_config,
        config_patcher,
        event_bus,
        shared_token_mgr
    );

    // Global tool permissions
    register_handler!(
        server,
        "config.get_tool_permissions",
        config_handlers::handle_get_tool_permissions,
        config
    );
    register_handler!(
        server,
        "config.update_tool_permissions",
        config_handlers::handle_update_tool_permissions,
        config,
        event_bus
    );

    // Providers (vault-backed API key storage)
    register_handler!(
        server,
        "providers.list",
        providers::handle_list,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "providers.get",
        providers::handle_get,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "providers.create",
        providers::handle_create,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "providers.update",
        providers::handle_update,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "providers.delete",
        providers::handle_delete,
        config,
        event_bus,
        shared_token_mgr
    );
    // providers.setDefault needs the swappable registry for hot-switching.
    // Pass vault so the runtime swap can inject api_key from vault into the cloned ProviderConfig
    // (api_key is #[serde(skip)] and never lives on disk).
    if let Some(ref registry) = multi_registry {
        register_handler!(
            server,
            "providers.setDefault",
            providers::handle_set_default,
            config,
            event_bus,
            registry,
            shared_token_mgr
        );
    } else {
        // Fallback: config-only update without runtime provider swap
        register_handler!(
            server,
            "providers.setDefault",
            providers::handle_set_default_config_only,
            config,
            event_bus
        );
    }
    if let Some(ref registry) = multi_registry {
        register_handler!(
            server,
            "providers.test",
            providers::handle_test,
            config,
            shared_token_mgr,
            registry
        );
    } else {
        register_handler!(
            server,
            "providers.test",
            providers::handle_test_no_registry,
            config,
            shared_token_mgr
        );
    }
    register_handler!(
        server,
        "providers.needsSetup",
        providers::handle_needs_setup,
        config
    );

    // Routing rules
    register_handler!(
        server,
        "routing_rules.list",
        routing_rules::handle_list,
        config
    );
    register_handler!(
        server,
        "routing_rules.get",
        routing_rules::handle_get,
        config
    );
    register_handler!(
        server,
        "routing_rules.create",
        routing_rules::handle_create,
        config,
        event_bus
    );
    register_handler!(
        server,
        "routing_rules.update",
        routing_rules::handle_update,
        config,
        event_bus
    );
    register_handler!(
        server,
        "routing_rules.delete",
        routing_rules::handle_delete,
        config,
        event_bus
    );
    register_handler!(
        server,
        "routing_rules.move",
        routing_rules::handle_move,
        config,
        event_bus
    );

    // MCP config
    register_handler!(server, "mcp_config.list", mcp_config::handle_list, config);
    register_handler!(server, "mcp_config.get", mcp_config::handle_get, config);
    register_handler!(
        server,
        "mcp_config.create",
        mcp_config::handle_create,
        config,
        event_bus
    );
    register_handler!(
        server,
        "mcp_config.update",
        mcp_config::handle_update,
        config,
        event_bus
    );
    register_handler!(
        server,
        "mcp_config.delete",
        mcp_config::handle_delete,
        config,
        event_bus
    );

    // Memory config
    register_handler!(
        server,
        "memory_config.get",
        memory_config::handle_get,
        config
    );
    register_handler!(
        server,
        "memory_config.update",
        memory_config::handle_update,
        config,
        event_bus
    );

    // Rerank config (dedicated handlers, API key in vault)
    register_handler!(
        server,
        "rerank_config.get",
        rerank_config::handle_get,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "rerank_config.update",
        rerank_config::handle_update,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "rerank_config.test",
        rerank_config::handle_test,
        config,
        shared_token_mgr
    );

    // Security config
    register_handler!(
        server,
        "security_config.get",
        security_config::handle_get,
        config_patcher
    );
    register_handler!(
        server,
        "security_config.update",
        security_config::handle_update,
        config_patcher,
        event_bus
    );
    register_handler!(
        server,
        "security_config.list_devices",
        security_config::handle_list_devices,
        device_store
    );
    register_handler!(
        server,
        "security_config.revoke_device",
        security_config::handle_revoke_device,
        device_store,
        event_bus
    );

    // Generation providers (vault-backed API key storage)
    register_handler!(
        server,
        "generation_providers.list",
        generation_providers::handle_list,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "generation_providers.get",
        generation_providers::handle_get,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "generation_providers.create",
        generation_providers::handle_create,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "generation_providers.update",
        generation_providers::handle_update,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "generation_providers.delete",
        generation_providers::handle_delete,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "generation_providers.setDefault",
        generation_providers::handle_set_default,
        config,
        event_bus
    );
    register_handler!(
        server,
        "generation_providers.test",
        generation_providers::handle_test_connection,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "generation_providers.voices",
        generation_providers::handle_voices,
        config,
        shared_token_mgr
    );

    // Embedding providers (vault-backed API key storage)
    register_handler!(
        server,
        "embedding_providers.list",
        embedding_providers::handle_list,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "embedding_providers.get",
        embedding_providers::handle_get,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "embedding_providers.add",
        embedding_providers::handle_add,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "embedding_providers.update",
        embedding_providers::handle_update,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "embedding_providers.remove",
        embedding_providers::handle_remove,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "embedding_providers.setActive",
        embedding_providers::handle_set_active,
        config,
        event_bus
    );
    register_handler!(
        server,
        "embedding_providers.test",
        embedding_providers::handle_test,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "embedding_providers.presets",
        embedding_providers::handle_presets
    );

    // Agent config
    register_handler!(server, "agent_config.get", agent_config::handle_get, config);
    register_handler!(
        server,
        "agent_config.update",
        agent_config::handle_update,
        config,
        event_bus
    );
    register_handler!(
        server,
        "agent_config.get_file_ops",
        agent_config::handle_get_file_ops,
        config
    );
    register_handler!(
        server,
        "agent_config.update_file_ops",
        agent_config::handle_update_file_ops,
        config,
        event_bus
    );
    register_handler!(
        server,
        "agent_config.get_code_exec",
        agent_config::handle_get_code_exec,
        config
    );
    register_handler!(
        server,
        "agent_config.update_code_exec",
        agent_config::handle_update_code_exec,
        config,
        event_bus
    );
    register_handler!(
        server,
        "agent_config.get_tool_permissions",
        agent_config::handle_get_tool_permissions,
        config
    );
    register_handler!(
        server,
        "agent_config.update_tool_permissions",
        agent_config::handle_update_tool_permissions,
        config,
        event_bus
    );

    // General config
    register_handler!(
        server,
        "general_config.get",
        general_config::handle_get,
        config
    );
    register_handler!(
        server,
        "general_config.update",
        general_config::handle_update,
        config,
        event_bus
    );

    // Browser config
    register_handler!(
        server,
        "browser_config.get",
        browser_config::handle_get,
        config
    );
    register_handler!(
        server,
        "browser_config.update",
        browser_config::handle_update,
        config,
        event_bus
    );

    // Behavior config
    register_handler!(
        server,
        "behavior_config.get",
        behavior_config::handle_get,
        config
    );
    register_handler!(
        server,
        "behavior_config.update",
        behavior_config::handle_update,
        config,
        event_bus
    );

    // Generation config
    register_handler!(
        server,
        "generation_config.get",
        generation_config::handle_get,
        config
    );
    register_handler!(
        server,
        "generation_config.update",
        generation_config::handle_update,
        config,
        event_bus
    );

    // Search config (vault-backed API key storage)
    register_handler!(
        server,
        "search_config.get",
        search_config::handle_get,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "search_config.update",
        search_config::handle_update,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "search_config.test",
        search_config::handle_test,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "search_config.deleteBackend",
        search_config::handle_delete_backend,
        config,
        event_bus,
        shared_token_mgr
    );

    // Execution config
    register_handler!(
        server,
        "execution_config.get",
        execution_config::handle_get,
        config
    );
    register_handler!(
        server,
        "execution_config.update",
        execution_config::handle_update,
        config,
        event_bus
    );

    // ACP harness config (only if ACP manager is available)
    if let Some(ref acp) = acp_manager {
        use alephcore::gateway::handlers::acp_config;

        register_handler!(server, "acp.list", acp_config::handle_list, acp, config);
        register_handler!(server, "acp.get", acp_config::handle_get, acp, config);
        register_handler!(
            server,
            "acp.create",
            acp_config::handle_create,
            acp,
            config,
            event_bus
        );
        register_handler!(
            server,
            "acp.update",
            acp_config::handle_update,
            acp,
            config,
            event_bus
        );
        register_handler!(
            server,
            "acp.delete",
            acp_config::handle_delete,
            acp,
            config,
            event_bus
        );
        register_handler!(server, "acp.test", acp_config::handle_test, acp);
        register_handler!(
            server,
            "acp.set_enabled",
            acp_config::handle_set_enabled,
            acp,
            config,
            event_bus
        );
        register_handler!(server, "acp.presets", acp_config::handle_presets);
        register_handler!(server, "acp.presets_meta", acp_config::handle_presets_meta);
    }
}

// ─── register_daemon_handlers ────────────────────────────────────────────────

