use super::*;

pub(in crate::commands::start) fn register_memory_handlers(
    server: &mut GatewayServer,
    memory_db: &MemoryBackend,
    compression_service: &Option<
        std::sync::Arc<alephcore::memory::compression::CompressionService>,
    >,
    embedder: &Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    event_bus: &std::sync::Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    daemon: bool,
) {
    register_handler!(
        server,
        "memory.search",
        memory_handlers::handle_search,
        memory_db
    );
    register_handler!(
        server,
        "memory.stats",
        memory_handlers::handle_stats,
        memory_db
    );
    register_handler!(
        server,
        "memory.delete",
        memory_handlers::handle_delete,
        memory_db
    );
    register_handler!(
        server,
        "memory.clear",
        memory_handlers::handle_clear,
        memory_db
    );
    register_handler!(
        server,
        "memory.listFacts",
        memory_handlers::handle_list_facts,
        memory_db
    );
    register_handler!(
        server,
        "memory.clearFacts",
        memory_handlers::handle_clear_facts,
        memory_db
    );
    register_handler!(
        server,
        "memory.appList",
        memory_handlers::handle_app_list,
        memory_db
    );
    // Read-only per-tool usage introspection over ToolInvocation raw rows.
    register_handler!(
        server,
        "insights.tools",
        alephcore::gateway::handlers::insights::handle_tools,
        memory_db
    );
    if let Some(cs) = compression_service {
        register_handler!(
            server,
            "memory.compress",
            memory_handlers::handle_compress,
            cs
        );
    } else {
        server
            .handlers_mut()
            .register("memory.compress", |req| async move {
                alephcore::gateway::protocol::JsonRpcResponse::error(
                    req.id,
                    alephcore::gateway::protocol::INTERNAL_ERROR,
                    "Compression not available: missing AI or embedding provider".to_string(),
                )
            });
    }

    // Reembed migration handlers
    if let Some(emb) = embedder {
        let reembed_state =
            std::sync::Arc::new(alephcore::gateway::handlers::memory::ReembedState::new());

        {
            let db = ::std::sync::Arc::clone(memory_db);
            let emb = ::std::sync::Arc::clone(emb);
            let eb = ::std::sync::Arc::clone(event_bus);
            let rs = ::std::sync::Arc::clone(&reembed_state);
            let mem_dir = alephcore::utils::paths::get_note_memory_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(""));
            server
                .handlers_mut()
                .register("memory.reembed", move |req| {
                    let db = ::std::sync::Arc::clone(&db);
                    let emb = ::std::sync::Arc::clone(&emb);
                    let eb = ::std::sync::Arc::clone(&eb);
                    let rs = ::std::sync::Arc::clone(&rs);
                    let md = mem_dir.clone();
                    async move { memory_handlers::handle_reembed(req, db, md, emb, eb, rs).await }
                });
        }

        {
            let rs = ::std::sync::Arc::clone(&reembed_state);
            server
                .handlers_mut()
                .register("memory.reembed.cancel", move |req| {
                    let rs = ::std::sync::Arc::clone(&rs);
                    async move { memory_handlers::handle_reembed_cancel(req, rs).await }
                });
        }
    } else {
        server
            .handlers_mut()
            .register("memory.reembed", |req| async move {
                alephcore::gateway::protocol::JsonRpcResponse::error(
                    req.id,
                    alephcore::gateway::protocol::INTERNAL_ERROR,
                    "Reembed not available: missing embedding provider".to_string(),
                )
            });
        server
            .handlers_mut()
            .register("memory.reembed.cancel", |req| async move {
                alephcore::gateway::protocol::JsonRpcResponse::error(
                    req.id,
                    alephcore::gateway::protocol::INTERNAL_ERROR,
                    "Reembed not available: missing embedding provider".to_string(),
                )
            });
    }

    if !daemon {
        println!("Memory methods:");
        println!("  - memory.search     : Search memories");
        println!("  - memory.stats      : Get memory statistics");
        println!("  - memory.delete     : Delete a memory");
        println!("  - memory.clear      : Clear memories");
        println!("  - memory.compress   : Trigger compression");
        println!("  - memory.reembed    : Re-embed all memories");
        println!("  - insights.tools    : Per-tool usage breakdown");
        println!();
    }
}

// ─── init_compression_service ────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) fn init_compression_service(
    memory_db: &MemoryBackend,
    provider: std::sync::Arc<dyn alephcore::providers::AiProvider>,
    embedder: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>,
    policy: &alephcore::CompressionPolicy,
    daemon: bool,
    command_handler: Option<
        std::sync::Arc<alephcore::memory::events::handler::MemoryCommandHandler>,
    >,
    compound_ingestor: Option<
        std::sync::Arc<dyn alephcore::memory::notes::ingest::CompoundIngestor>,
    >,
    profile_synthesizer: Option<
        std::sync::Arc<dyn alephcore::memory::notes::profile::synthesizer::ProfileSynthesizer>,
    >,
) -> std::sync::Arc<alephcore::memory::compression::CompressionService> {
    use alephcore::memory::compression::{CompressionConfig, CompressionService};

    let config = CompressionConfig::from_policy(policy);
    let mut service = CompressionService::new(memory_db.clone(), provider, embedder, config);
    if let Some(handler) = command_handler {
        service = service.with_command_handler(handler);
    }
    if let Some(ing) = compound_ingestor {
        service = service.with_compound_ingestor(ing);
    }
    // Spec 7 T9: inject profile synthesizer so SessionEnd triggers USER.md update.
    if let Some(ps) = profile_synthesizer {
        service = service.with_profile_synthesizer(ps);
    }
    let service = std::sync::Arc::new(service);

    // Start background compression loop (hourly check)
    let _handle = service.clone().start_background_task();

    if !daemon {
        println!(
            "Compression service started (interval: {}s, turn threshold: {})",
            policy.background_interval_seconds, policy.turn_threshold
        );
    }

    service
}

// ─── init_command_handler ───────────────────────────────────────────────────

pub(in crate::commands::start) async fn init_command_handler(
    state_db: &std::sync::Arc<alephcore::resilience::database::StateDatabase>,
    memory_db: &alephcore::memory::store::MemoryBackend,
    _daemon: bool,
) -> std::sync::Arc<alephcore::memory::events::handler::MemoryCommandHandler> {
    use alephcore::memory::events::handler::MemoryCommandHandler;

    std::sync::Arc::new(MemoryCommandHandler::new(
        std::sync::Arc::clone(state_db),
        Some(memory_db.clone()),
    ))
}

// ─── init_memory_context_provider ────────────────────────────────────────────

pub(in crate::commands::start) fn init_memory_context_provider(
    memory_db: &MemoryBackend,
    embedder: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>,
    provider: Option<std::sync::Arc<dyn alephcore::providers::AiProvider>>,
    assembler_config: alephcore::AssemblerConfig,
) -> std::sync::Arc<alephcore::thinker::MemoryContextProvider> {
    init_memory_context_provider_with_extensions(
        memory_db,
        embedder,
        provider,
        assembler_config,
        None,
        None,
        None,
    )
}

/// Like `init_memory_context_provider` but wires an optional extension registry, wiki handle,
/// and profile synthesizer.
pub(in crate::commands::start) fn init_memory_context_provider_with_extensions(
    memory_db: &MemoryBackend,
    embedder: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>,
    provider: Option<std::sync::Arc<dyn alephcore::providers::AiProvider>>,
    assembler_config: alephcore::AssemblerConfig,
    extensions: Option<std::sync::Arc<alephcore::memory::extensions::MemoryExtensionRegistry>>,
    wiki: Option<std::sync::Arc<dyn alephcore::memory::notes::orientation::NoteOrientation>>,
    profile_synthesizer: Option<
        std::sync::Arc<dyn alephcore::memory::notes::profile::synthesizer::ProfileSynthesizer>,
    >,
) -> std::sync::Arc<alephcore::thinker::MemoryContextProvider> {
    let mcp = match provider {
        Some(p) => alephcore::thinker::MemoryContextProvider::with_provider(
            memory_db.clone(),
            embedder,
            p,
            assembler_config,
            alephcore::thinker::MemoryContextConfig::default(),
        ),
        None => alephcore::thinker::MemoryContextProvider::new(memory_db.clone(), embedder),
    };
    let mcp = if let Some(ext) = extensions {
        mcp.with_extensions(ext)
    } else {
        mcp
    };
    // Spec 5 Task 12: wire wiki orientation for build_orientation_user_message.
    let mcp = if let Some(w) = wiki {
        mcp.with_orientation(w)
    } else {
        mcp
    };
    // Spec 7 Task 9: wire profile synthesizer for build_profile_user_message.
    let mcp = if let Some(ps) = profile_synthesizer {
        mcp.with_profile(ps)
    } else {
        mcp
    };
    std::sync::Arc::new(mcp)
}

// ─── register_workspace_handlers ─────────────────────────────────────────────
