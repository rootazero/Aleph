use super::{memory_handlers, GatewayServer, MemoryBackend};
use alephcore::gateway::handlers::memory_curated;

pub(in crate::commands::start) fn register_memory_handlers(
    server: &mut GatewayServer,
    raw_store: &std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
    event_bus: &std::sync::Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    app_config: &std::sync::Arc<tokio::sync::RwLock<alephcore::Config>>,
    memory_context_provider: Option<
        std::sync::Arc<alephcore::thinker::memory_context_provider::RuntimeMemoryContext>,
    >,
    memory_runtime: &std::sync::Arc<alephcore::memory::runtime::MemoryRuntime>,
    daemon: bool,
) {
    register_handler!(
        server,
        "memory.search",
        memory_handlers::handle_search,
        raw_store
    );
    register_handler!(
        server,
        "memory.delete",
        memory_handlers::handle_delete,
        raw_store
    );
    register_handler!(
        server,
        "insights.tools",
        alephcore::gateway::handlers::insights::handle_tools,
        raw_store
    );

    register_handler!(
        server,
        "memory.stats",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "memory.listFacts",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "memory.list_corrections",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "memory.trace",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "memory.compress",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "memory.reembed",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "memory.reembed.cancel",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "memory.retrieve_with_trace",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "dreaming.list_insights",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );
    register_handler!(
        server,
        "dreaming.run_now",
        memory_handlers::runtime::handle_memory,
        memory_runtime,
        app_config,
        event_bus
    );

    // These handlers preserve their visibility gate before resolving a scoped
    // store through the stable runtime context.
    {
        let context = memory_context_provider.clone();
        server
            .handlers_mut()
            .register("memory.curated.list", move |request| {
                let context = context.clone();
                async move { memory_curated::handle_list(request, context).await }
            });
    }
    {
        let context = memory_context_provider.clone();
        server
            .handlers_mut()
            .register("memory.curated.replace", move |request| {
                let context = context.clone();
                async move { memory_curated::handle_replace(request, context).await }
            });
    }
    {
        let context = memory_context_provider;
        server
            .handlers_mut()
            .register("memory.curated.remove", move |request| {
                let context = context.clone();
                async move { memory_curated::handle_remove(request, context).await }
            });
    }
    if !daemon {
        println!("Memory methods registered (raw sessions and admitted derived work)");
    }
}

// ─── init_memory_context_provider ────────────────────────────────────────────

/// `curated` is a required parameter rather than a defaulted one on purpose.
/// `[memory.curated]` reached the provider through exactly one call — a unit
/// test that built its own provider — for as long as this factory omitted it,
/// so every knob in the section persisted to `config.toml` and changed nothing.
/// Making it a parameter turns "forgot to pass the section" into a compile
/// error at every construction site, which no test can do.
pub(in crate::commands::start) fn init_memory_context_provider(
    memory_db: &MemoryBackend,
    raw_store: std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
    embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    provider: Option<std::sync::Arc<dyn alephcore::providers::AiProvider>>,
    assembler_config: alephcore::AssemblerConfig,
    injection_mode: alephcore::MemoryInjectionMode,
    curated: alephcore::memory::curated::CuratedConfig,
    // `[memory.orientation] max_tokens` — the token budget one orientation
    // snapshot may spend. Threaded like `curated` because an unthreaded knob
    // persisted to `config.toml` and changed nothing (same lesson).
    orientation_max_tokens: usize,
) -> std::sync::Arc<alephcore::thinker::MemoryContextProvider> {
    init_memory_context_provider_with_extensions(
        memory_db,
        raw_store,
        embedder,
        provider,
        assembler_config,
        injection_mode,
        curated,
        orientation_max_tokens,
        None,
        None,
        None,
    )
}

/// Like `init_memory_context_provider` but wires an optional extension registry, wiki handle,
/// and profile synthesizer.
///
/// `embedder: None` = FTS-only deployment; memory injection still runs.
#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) fn init_memory_context_provider_with_extensions(
    memory_db: &MemoryBackend,
    raw_store: std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
    embedder: Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    provider: Option<std::sync::Arc<dyn alephcore::providers::AiProvider>>,
    assembler_config: alephcore::AssemblerConfig,
    // `memory.injection_mode` from config. Without this the provider stays on
    // the default (Hybrid) and an `injection_mode = "tools"` deployment would
    // still auto-inject memory/orientation/profile into every prompt.
    injection_mode: alephcore::MemoryInjectionMode,
    // `[memory.curated]` — the char budgets and the open-loops age ceiling that
    // decide how the always-on curated envelope renders. See
    // `init_memory_context_provider`'s doc for why this is not defaulted.
    curated: alephcore::memory::curated::CuratedConfig,
    // `[memory.orientation] max_tokens` — see `init_memory_context_provider`.
    orientation_max_tokens: usize,
    extensions: Option<std::sync::Arc<alephcore::memory::extensions::MemoryExtensionRegistry>>,
    wiki: Option<std::sync::Arc<dyn alephcore::memory::notes::orientation::NoteOrientation>>,
    profile_synthesizer: Option<
        std::sync::Arc<dyn alephcore::memory::notes::profile::synthesizer::ProfileSynthesizer>,
    >,
) -> std::sync::Arc<alephcore::thinker::MemoryContextProvider> {
    let mcp = match provider {
        Some(p) => alephcore::thinker::MemoryContextProvider::with_provider(
            memory_db.clone(),
            raw_store,
            embedder,
            p,
            assembler_config,
            alephcore::thinker::MemoryContextConfig::default(),
        ),
        None => {
            alephcore::thinker::MemoryContextProvider::new(memory_db.clone(), raw_store, embedder)
        }
    };
    let mcp = mcp
        .with_injection_mode(injection_mode)
        .with_curated_config(curated)
        .with_orientation_budget(alephcore::memory::notes::orientation::types::TokenBudget {
            max_tokens: orientation_max_tokens,
        });
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

// ─── register_session_management_handlers ────────────────────────────────────

pub(in crate::commands::start) fn register_session_management_handlers(
    server: &mut GatewayServer,
    session_store: &std::sync::Arc<alephcore::memory::store::markdown::MarkdownSessionStore>,
    gateway_store: &std::sync::Arc<dyn alephcore::gateway::session_store::SessionStore>,
    daemon: bool,
) {
    use alephcore::gateway::handlers::memory::sessions;

    register_handler!(
        server,
        "memory.sessions.list",
        sessions::handle_list,
        session_store,
        gateway_store
    );
    register_handler!(
        server,
        "memory.sessions.get",
        sessions::handle_get,
        session_store,
        gateway_store
    );
    register_handler!(
        server,
        "memory.sessions.search",
        sessions::handle_search,
        session_store,
        gateway_store
    );
    register_handler!(
        server,
        "memory.sessions.update_compression",
        sessions::handle_update_compression,
        session_store,
        gateway_store
    );

    if !daemon {
        println!("Session management methods:");
        println!("  - memory.sessions.list               : List sessions");
        println!("  - memory.sessions.get                : Get session details");
        println!("  - memory.sessions.search             : Search transcripts");
        println!("  - memory.sessions.update_compression : Update compression status");
        println!();
    }
}

// ─── register_workspace_handlers ─────────────────────────────────────────────
