use super::{memory_handlers, GatewayServer, MemoryBackend};
use alephcore::gateway::handlers::memory_curated;

/// Rebuild the embedder from the *current* config so a provider switched via
/// `embedding_providers.setActive` takes effect for reembed without restarting
/// the daemon. Returns `None` when no active provider can be built (caller falls
/// back to the boot embedder).
///
/// Embedding API keys are `#[serde(skip)]` and never persisted, so they are
/// re-hydrated from the vault (`embed:<id>`) — mirroring the boot path — before
/// constructing the provider.
async fn resolve_active_embedder(
    app_config: &std::sync::Arc<tokio::sync::RwLock<alephcore::Config>>,
    shared_token_mgr: &std::sync::Arc<alephcore::gateway::security::SharedTokenManager>,
) -> Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>> {
    let mut settings = app_config.read().await.memory.embedding.clone();
    for p in &mut settings.providers {
        if p.api_key.as_ref().is_none_or(std::string::String::is_empty) {
            if let Ok(Some(secret)) = shared_token_mgr.get_secret(&format!("embed:{}", p.id)) {
                p.api_key = Some(secret.expose().to_string());
            }
        }
    }
    let manager = alephcore::memory::EmbeddingManager::new(settings);
    manager.init().await.ok()?;
    manager.get_active_provider().await
}

pub(in crate::commands::start) fn register_memory_handlers(
    server: &mut GatewayServer,
    memory_db: &MemoryBackend,
    compression_service: &Option<
        std::sync::Arc<alephcore::memory::compression::CompressionService>,
    >,
    embedder: &Option<std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>>,
    event_bus: &std::sync::Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    // Live config + vault so reembed can rebuild the embedder from the
    // *currently active* provider (switched via embedding_providers.setActive)
    // instead of the one frozen at boot.
    app_config: &std::sync::Arc<tokio::sync::RwLock<alephcore::Config>>,
    shared_token_mgr: &std::sync::Arc<alephcore::gateway::security::SharedTokenManager>,
    // Curated hot-tier store resolver. `None` in a process with no agent
    // runtime — see the `memory.curated.*` registrations below for why they
    // are still registered in that case.
    memory_context_provider: Option<
        std::sync::Arc<alephcore::thinker::MemoryContextProvider>,
    >,
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
        "memory.listFacts",
        memory_handlers::handle_list_facts,
        memory_db
    );
    // Read-only corrections governance: raw correction rows + distillation status.
    register_handler!(
        server,
        "memory.list_corrections",
        memory_handlers::handle_list_corrections,
        memory_db
    );
    // Curated hot memory (`MEMORY.md`) — the third memory pillar's manage face.
    //
    // Registered UNCONDITIONALLY, even with no `MemoryContextProvider`: a
    // missing method is indistinguishable from an older core that never had
    // it, while a registered one can say *why* it cannot serve
    // (SERVICE_UNAVAILABLE). Same reasoning as the `memory.compress`
    // placeholder below — and the inverse of `dreaming.list_insights`, whose
    // absent phase-1 placeholder made it METHOD_NOT_FOUND during boot.
    //
    // Three literal `.register("…")` calls rather than a loop over a
    // name/verb table, and no local macro: `method_census`'s scanner reads
    // THIS source and recognises exactly this shape. A dynamic method name —
    // or a macro it has not been taught — is a family it cannot see, and a
    // family it cannot see is one whose ruling nobody has to record.
    // `register_handler!` is not usable here: it `Arc::clone`s its context,
    // and this one is an `Option<Arc<_>>` (absent in a process with no agent
    // runtime).
    {
        let mcp = memory_context_provider.clone();
        server
            .handlers_mut()
            .register("memory.curated.list", move |req| {
                let mcp = mcp.clone();
                async move { memory_curated::handle_list(req, mcp).await }
            });
    }
    {
        let mcp = memory_context_provider.clone();
        server
            .handlers_mut()
            .register("memory.curated.replace", move |req| {
                let mcp = mcp.clone();
                async move { memory_curated::handle_replace(req, mcp).await }
            });
    }
    {
        let mcp = memory_context_provider.clone();
        server
            .handlers_mut()
            .register("memory.curated.remove", move |req| {
                let mcp = mcp.clone();
                async move { memory_curated::handle_remove(req, mcp).await }
            });
    }

    // Read-only evidence-chain walk: note / raw / profile-section → ground-truth
    // raws, plus the curated write-decision ledger. Takes the provider because
    // both of those live under the caller's COMPOSED partition — see
    // `handle_trace`'s doc; without it the ledger is empty on every scoped
    // session, which on a stock single-machine install means every session.
    {
        let db = memory_db.clone();
        let mcp = memory_context_provider.clone();
        server.handlers_mut().register("memory.trace", move |req| {
            let db = db.clone();
            let mcp = mcp.clone();
            async move { memory_handlers::handle_trace(req, db, mcp).await }
        });
    }
    // Read-only per-tool usage introspection over ToolInvocation raw rows.
    register_handler!(
        server,
        "insights.tools",
        alephcore::gateway::handlers::insights::handle_tools,
        memory_db
    );
    // Read-only dream insights listing (daily digests + synthesis + run history).
    register_handler!(
        server,
        "dreaming.list_insights",
        alephcore::gateway::handlers::dreaming::handle_list_insights,
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

        // Resolve the note-memory dir up front: if it can't be resolved, wire an
        // error stub rather than registering the handler against an empty path
        // (which would silently re-embed against the wrong/CWD-relative location).
        match alephcore::utils::paths::get_note_memory_dir() {
            Ok(mem_dir) => {
                let db = ::std::sync::Arc::clone(memory_db);
                let emb = ::std::sync::Arc::clone(emb);
                let eb = ::std::sync::Arc::clone(event_bus);
                let rs = ::std::sync::Arc::clone(&reembed_state);
                let cfg = ::std::sync::Arc::clone(app_config);
                let vault = ::std::sync::Arc::clone(shared_token_mgr);
                server
                    .handlers_mut()
                    .register("memory.reembed", move |req| {
                        let db = ::std::sync::Arc::clone(&db);
                        let emb = ::std::sync::Arc::clone(&emb);
                        let eb = ::std::sync::Arc::clone(&eb);
                        let rs = ::std::sync::Arc::clone(&rs);
                        let cfg = ::std::sync::Arc::clone(&cfg);
                        let vault = ::std::sync::Arc::clone(&vault);
                        let md = mem_dir.clone();
                        async move {
                            // Resolve the active provider at call time so a provider
                            // switched in the panel takes effect without a restart;
                            // fall back to the boot embedder if it can't be rebuilt.
                            let embedder =
                                resolve_active_embedder(&cfg, &vault).await.unwrap_or(emb);
                            memory_handlers::handle_reembed(req, db, md, embedder, eb, rs).await
                        }
                    });
            }
            Err(e) => {
                tracing::warn!(error = %e, "memory.reembed unavailable: cannot resolve note memory dir");
                server
                    .handlers_mut()
                    .register("memory.reembed", |req| async move {
                        alephcore::gateway::protocol::JsonRpcResponse::error(
                            req.id,
                            alephcore::gateway::protocol::INTERNAL_ERROR,
                            "Reembed not available: note memory directory unresolved".to_string(),
                        )
                    });
            }
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

        // Embedding vector-space drift check: if the active provider differs
        // from the model that produced the stored vectors, recommend a reembed.
        // A standing mismatch re-surfaces on every boot until reembedded. The
        // signature is recorded by `reembed_all`; an `Unknown` (never reembedded)
        // store stays silent.
        {
            let db = ::std::sync::Arc::clone(memory_db);
            let emb = ::std::sync::Arc::clone(emb);
            tokio::spawn(async move {
                use alephcore::memory::embedding_signature as sig;
                let current = sig::provider_signature(emb.as_ref());
                let stored = db.get_embedding_signature().ok().flatten();
                if let sig::SignatureStatus::Mismatch { stored, current } =
                    sig::compare(stored.as_deref(), &current)
                {
                    tracing::warn!(
                        stored = %stored,
                        current = %current,
                        "Embedding model changed since notes were last embedded — \
                         run memory.reembed to keep retrieval accurate."
                    );
                }
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

    // memory.retrieve_with_trace — real scoring-pipeline trace for the debug panel.
    {
        let memory_db = std::sync::Arc::clone(memory_db);
        let embedder = embedder.clone();
        let app_config = std::sync::Arc::clone(app_config);
        server
            .handlers_mut()
            .register("memory.retrieve_with_trace", move |req| {
                let memory_db = std::sync::Arc::clone(&memory_db);
                let embedder = embedder.clone();
                let app_config = std::sync::Arc::clone(&app_config);
                async move {
                    alephcore::gateway::handlers::memory_config::handle_retrieve_with_trace(
                        req, memory_db, embedder, app_config,
                    )
                    .await
                }
            });
    }

    if !daemon {
        println!("Memory methods:");
        println!("  - memory.search     : Search memories");
        println!("  - memory.stats      : Get memory statistics");
        println!("  - memory.delete     : Delete a memory");
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
    compound_ingestor: Option<
        std::sync::Arc<dyn alephcore::memory::notes::ingest::CompoundIngestor>,
    >,
    profile_synthesizer: Option<
        std::sync::Arc<dyn alephcore::memory::notes::profile::synthesizer::ProfileSynthesizer>,
    >,
    extension_registry: Option<
        std::sync::Arc<alephcore::memory::extensions::MemoryExtensionRegistry>,
    >,
) -> std::sync::Arc<alephcore::memory::compression::CompressionService> {
    use alephcore::memory::compression::{CompressionConfig, CompressionService};

    let config = CompressionConfig::from_policy(policy);
    let mut service = CompressionService::new(memory_db.clone(), provider, embedder, config);
    if let Some(ing) = compound_ingestor {
        service = service.with_compound_ingestor(ing);
    }
    // Spec 7 T9: inject profile synthesizer so SessionEnd triggers USER.md update.
    if let Some(ps) = profile_synthesizer {
        service = service.with_profile_synthesizer(ps);
    }
    // X1 C3: thread the memory-extension registry so compress_to_notes fires
    // on_pre_compress before ingest.
    if let Some(reg) = extension_registry {
        service = service.with_extension_registry(reg);
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
    _memory_db: &alephcore::memory::store::MemoryBackend,
    _daemon: bool,
) -> std::sync::Arc<alephcore::memory::events::handler::MemoryCommandHandler> {
    use alephcore::memory::events::handler::MemoryCommandHandler;

    std::sync::Arc::new(MemoryCommandHandler::new(std::sync::Arc::clone(state_db)))
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
            embedder,
            p,
            assembler_config,
            alephcore::thinker::MemoryContextConfig::default(),
        ),
        None => alephcore::thinker::MemoryContextProvider::new(memory_db.clone(), embedder),
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

// ─── register_workspace_handlers ─────────────────────────────────────────────
