use super::{memory_handlers, GatewayServer};
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
