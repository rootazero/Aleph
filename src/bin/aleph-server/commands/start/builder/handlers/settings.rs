use super::{
    config_handlers, workspace_handlers, AgentEnvStore, Arc, GatewayServer, MemoryBackend,
};
use alephcore::gateway::AgentRegistry;

pub(in crate::commands::start) fn register_workspace_handlers(
    server: &mut GatewayServer,
    workspace_manager: &Arc<AgentEnvStore>,
    // Runtime registry used to reject a bind to a non-existent agent, matching
    // the `agent_switch` tool. `None` on a minimal server → bind stays unchecked.
    agent_registry: Option<&Arc<AgentRegistry>>,
    // Event bus for Bound/Unbound lifecycle events (Panel cross-surface sync).
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
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
    // Hand-wired (not the `register_handler!` macro) because the handler takes
    // an `Option<Arc<AgentRegistry>>` — the macro only threads required `Arc`s.
    {
        let workspace_manager = Arc::clone(workspace_manager);
        let agent_registry = agent_registry.cloned();
        let event_bus = Arc::clone(event_bus);
        server
            .handlers_mut()
            .register("channels.set_agent", move |req| {
                let workspace_manager = Arc::clone(&workspace_manager);
                let agent_registry = agent_registry.clone();
                let event_bus = Arc::clone(&event_bus);
                async move {
                    workspace_handlers::handle_set_agent(
                        req,
                        workspace_manager,
                        agent_registry,
                        Some(event_bus),
                    )
                    .await
                }
            });
    }
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

// ─── register_projects_handlers ──────────────────────────────────────────────

/// Wire the per-user project catalogue (`~/.aleph/projects.json`).
///
/// These methods drive the desktop Panel's "Enter Project" picker: list /
/// add an existing folder / create a blank one / remove / touch / get.
pub(in crate::commands::start) fn register_projects_handlers(
    server: &mut GatewayServer,
    project_store: &Arc<alephcore::projects::ProjectStore>,
    daemon: bool,
) {
    use alephcore::gateway::handlers::projects as projects_handlers;

    register_handler!(
        server,
        "projects.list",
        projects_handlers::handle_list,
        project_store
    );
    register_handler!(
        server,
        "projects.add",
        projects_handlers::handle_add,
        project_store
    );
    register_handler!(
        server,
        "projects.create_blank",
        projects_handlers::handle_create_blank,
        project_store
    );
    register_handler!(
        server,
        "projects.remove",
        projects_handlers::handle_remove,
        project_store
    );
    register_handler!(
        server,
        "projects.touch",
        projects_handlers::handle_touch,
        project_store
    );
    register_handler!(
        server,
        "projects.get",
        projects_handlers::handle_get,
        project_store
    );

    if !daemon {
        println!("Project methods:");
        println!("  - projects.list         : List recent projects (sorted by last_used_at)");
        println!("  - projects.add          : Register an existing folder as a project");
        println!("  - projects.create_blank : Create + register a new empty folder");
        println!("  - projects.remove       : Drop a project entry (does not delete the folder)");
        println!("  - projects.touch        : Bump last_used_at for a project");
        println!("  - projects.get          : Fetch a single project by ID");
        println!();
    }
}

// ─── register_fs_handlers ────────────────────────────────────────────────────

/// Wire the cross-platform directory-browse RPCs (`fs.*`) used by the
/// Panel's `<DirectoryBrowser />`. Scope is gated by
/// `[projects].allowed_roots`; see `gateway::handlers::fs` for safety
/// surface details.
pub(in crate::commands::start) fn register_fs_handlers(
    server: &mut GatewayServer,
    config: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    daemon: bool,
) {
    use alephcore::gateway::handlers::fs as fs_handlers;

    register_handler!(
        server,
        "fs.allowed_roots",
        fs_handlers::handle_allowed_roots,
        config
    );
    register_handler!(server, "fs.home_dir", fs_handlers::handle_home_dir);
    register_handler!(server, "fs.list_dir", fs_handlers::handle_list_dir, config);
    register_handler!(
        server,
        "fs.create_dir",
        fs_handlers::handle_create_dir,
        config
    );
    register_handler!(
        server,
        "fs.read_file",
        fs_handlers::handle_read_file,
        config
    );

    if !daemon {
        println!("Filesystem-browse methods (scoped by [projects].allowed_roots):");
        println!("  - fs.allowed_roots : List configured browseable roots");
        println!("  - fs.home_dir      : Server's $HOME (for default start path)");
        println!("  - fs.list_dir      : List subdirectories + files at a path");
        println!("  - fs.create_dir    : Create a subdirectory inside an allowed root");
        println!("  - fs.read_file     : Read a file's text content (size-capped)");
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
    multi_registry: Option<Arc<alephcore::MultiProviderRegistry>>,
    shared_token_mgr: Arc<alephcore::gateway::security::SharedTokenManager>,
    acp_manager: Option<Arc<alephcore::acp::manager::AcpAdapterManager>>,
) -> Arc<alephcore::ConfigPatcher> {
    use alephcore::gateway::handlers::behavior_config;
    use alephcore::gateway::handlers::browser_config;
    use alephcore::gateway::handlers::config::{handle_get_full_config, handle_patch_config};
    use alephcore::gateway::handlers::embedding_providers;
    use alephcore::gateway::handlers::execution_config;
    use alephcore::gateway::handlers::fetch_config;
    use alephcore::gateway::handlers::general_config;
    use alephcore::gateway::handlers::generation_config;
    use alephcore::gateway::handlers::generation_providers;
    use alephcore::gateway::handlers::memory_config;
    use alephcore::gateway::handlers::moa;
    use alephcore::gateway::handlers::providers;
    use alephcore::gateway::handlers::rerank_config;
    use alephcore::gateway::handlers::route_config;
    use alephcore::gateway::handlers::routing_rules;
    use alephcore::gateway::handlers::search_config;
    use alephcore::gateway::handlers::security_config;
    use alephcore::gateway::handlers::voice;

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
    // create/delete register into / remove from the live registry when present,
    // so a provider added/removed at runtime is usable and an auto-derived
    // failover fallback without a restart (mirrors setDefault's hot path).
    if let Some(ref registry) = multi_registry {
        register_handler!(
            server,
            "providers.create",
            providers::handle_create_hot,
            config,
            event_bus,
            shared_token_mgr,
            registry
        );
    } else {
        register_handler!(
            server,
            "providers.create",
            providers::handle_create,
            config,
            event_bus,
            shared_token_mgr
        );
    }
    // update also hot-reloads the runtime provider instance when a registry is
    // available, so Panel edits to protocol/base_url/model take effect immediately.
    if let Some(ref registry) = multi_registry {
        register_handler!(
            server,
            "providers.update",
            providers::handle_update_hot,
            config,
            event_bus,
            shared_token_mgr,
            registry
        );
    } else {
        register_handler!(
            server,
            "providers.update",
            providers::handle_update,
            config,
            event_bus,
            shared_token_mgr
        );
    }
    if let Some(ref registry) = multi_registry {
        register_handler!(
            server,
            "providers.delete",
            providers::handle_delete_hot,
            config,
            event_bus,
            shared_token_mgr,
            registry
        );
    } else {
        register_handler!(
            server,
            "providers.delete",
            providers::handle_delete,
            config,
            event_bus,
            shared_token_mgr
        );
    }
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
    // Chat-window provider/model picker — joins built-in chat presets with
    // per-user credential state so the panel can render a credential-aware
    // selector without round-tripping through providers.list + catalog.
    register_handler!(
        server,
        "providers.catalog",
        providers::handle_catalog,
        config,
        shared_token_mgr
    );
    // Concurrent liveness sweep over all configured providers (read-only —
    // consumed by `aleph doctor`). Reuses the single-provider probe.
    register_handler!(
        server,
        "providers.healthcheck",
        providers::handle_healthcheck,
        config,
        shared_token_mgr
    );

    // MoA presets (visual config; shares MoaPresetStore with the `moa` tool)
    register_handler!(server, "moa.listPresets", moa::handle_list_presets, config);
    register_handler!(
        server,
        "moa.savePreset",
        moa::handle_save_preset,
        config,
        config_patcher
    );
    register_handler!(
        server,
        "moa.deletePreset",
        moa::handle_delete_preset,
        config,
        config_patcher
    );
    register_handler!(
        server,
        "moa.setDefault",
        moa::handle_set_default,
        config,
        config_patcher
    );
    register_handler!(
        server,
        "moa.setSaveTraces",
        moa::handle_set_save_traces,
        config,
        config_patcher
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
    register_handler!(
        server,
        "generation_providers.list_presets",
        generation_providers::handle_list_presets
    );

    // Panel mic button → browser-recorded audio → STT transcription.
    // Reuses the transcription provider resolved above for inbound voice.
    register_handler!(
        server,
        "voice.transcribe",
        voice::handle_transcribe,
        config,
        shared_token_mgr
    );

    // Streaming STT: Panel frames → backend WS → voice.transcribe.delta events.
    // One shared registry tracks all active streams (Arc for the macro's clone).
    let stream_registry = Arc::new(alephcore::gateway::voice::streaming::StreamRegistry::default());
    register_handler!(
        server,
        "voice.stream.start",
        voice::handle_stream_start,
        config,
        event_bus,
        stream_registry
    );
    register_handler!(
        server,
        "voice.stream.audio",
        voice::handle_stream_audio,
        stream_registry
    );
    register_handler!(
        server,
        "voice.stream.stop",
        voice::handle_stream_stop,
        stream_registry
    );

    // AI speech-formatting: fast-model pass cleaning a raw transcript into
    // written text for display (does NOT gate the agent). Same ctx as
    // voice.transcribe (config + vault for provider/api_key resolution).
    register_handler!(
        server,
        "voice.format",
        voice::handle_format,
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

    // Fetch config (vault-backed API key storage)
    register_handler!(
        server,
        "fetch_config.get",
        fetch_config::handle_get,
        config,
        shared_token_mgr
    );
    register_handler!(
        server,
        "fetch_config.update",
        fetch_config::handle_update,
        config,
        event_bus,
        shared_token_mgr
    );
    register_handler!(
        server,
        "fetch_config.test",
        fetch_config::handle_test,
        config,
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

    // Local/cloud route mode (hot-applies to the live failover chain)
    register_handler!(server, "route_config.get", route_config::handle_get, config);
    register_handler!(
        server,
        "route_config.update",
        route_config::handle_update,
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
        register_handler!(
            server,
            "acp.sessions.list",
            acp_config::handle_sessions_list,
            acp
        );
        register_handler!(
            server,
            "acp.sessions.cancel",
            acp_config::handle_sessions_cancel,
            acp
        );
        register_handler!(
            server,
            "acp.sessions.shutdown",
            acp_config::handle_sessions_shutdown,
            acp
        );
    }

    // Hand the patcher back to the caller so it can late-bind `self_config`
    // and `moa` tool instances (see `BuiltinToolRegistry::set_config_patcher`).
    config_patcher
}

// ─── register_voice_capability_handlers ──────────────────────────────────────

/// Wire the Panel's native voice-capture + TTS-playback RPCs.
///
/// - `voice.record_start` / `voice.record_stop` proxy the desktop bridge's
///   native `AVFoundation` recorder — the macOS path used when the unsigned
///   `WKWebView` cannot reach `getUserMedia`. Capture only; the bytes go back to
///   the Panel which posts them to `voice.transcribe`.
/// - `voice.synthesize` reuses the channel TTS path so the Panel can play the
///   agent's reply back as speech.
///
/// Capture and playback are endpoint I/O (R1/R6); STT/LLM/TTS stay in the core.
pub(in crate::commands::start) fn register_voice_capability_handlers(
    server: &mut GatewayServer,
    config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    // `alephcore::sync_primitives::RwLock` to match `AgentResult.generation_registry`
    // (`alephcore::sync_primitives::RwLock`); the handler snapshots it under
    // the sync lock, so no guard is held across an await.
    generation_registry: Option<
        Arc<alephcore::sync_primitives::RwLock<alephcore::generation::GenerationProviderRegistry>>,
    >,
) {
    use alephcore::gateway::handlers::voice;

    // A fresh platform handle — cheap, and mirrors how the presence / mic-level
    // reporters and the builtin tool registry each construct their own.
    let platform: Arc<dyn aleph_desktop::DesktopPlatform> = {
        #[cfg(target_os = "macos")]
        {
            Arc::new(aleph_desktop_macos::MacOSPlatform::new())
        }
        #[cfg(target_os = "linux")]
        {
            Arc::new(aleph_desktop_linux::LinuxPlatform::new())
        }
        #[cfg(target_os = "windows")]
        {
            Arc::new(aleph_desktop_windows::WindowsPlatform::new())
        }
    };

    register_handler!(
        server,
        "voice.record_start",
        voice::handle_record_start,
        platform
    );
    register_handler!(
        server,
        "voice.record_stop",
        voice::handle_record_stop,
        platform
    );

    if let Some(gen_reg) = generation_registry {
        register_handler!(
            server,
            "voice.synthesize",
            voice::handle_synthesize,
            config,
            gen_reg
        );
    }
}

// ─── register_daemon_handlers ────────────────────────────────────────────────
