//! Unified dispatch registry (`ToolCatalog`) construction + command/tool RPC
//! wiring.
//!
//! Extracted verbatim from `agent_init/mod.rs`. This block runs after the
//! real/simulated branch split — it is AI-provider-independent (it only maps
//! command names to metadata) and produces the `ToolCatalog` the caller stores
//! on `AgentHandlersResult.tool_catalog`.
//!
//! Side effects preserved exactly: registers `commands.list` / `tools.catalog`
//! / `tools.invoke` / `tools.effective` / `tools.cancel_call` /
//! `tools.in_flight` / `command.execute` handlers, injects the `CommandParser`
//! into the deferred `command_parser_cell`, spawns the `MemoryProducerScheduler`
//! (handle intentionally leaked for server lifetime), and threads the memory
//! extension registry into the global `ExtensionManager`.

use alephcore::sync_primitives::{Arc, RwLock};

use alephcore::executor::BuiltinToolRegistry;
use alephcore::gateway::GatewayServer;

/// Build the unified dispatch registry and wire the command/tool RPC handlers.
/// Returns the constructed `ToolCatalog` (the caller publishes it on
/// `AgentHandlersResult`).
#[allow(clippy::too_many_arguments)]
pub(super) async fn init_tool_catalog(
    server: &mut GatewayServer,
    generation_registry: &Arc<RwLock<alephcore::generation::GenerationProviderRegistry>>,
    app_config: &alephcore::Config,
    tool_reg_out: Option<Arc<BuiltinToolRegistry>>,
    command_parser_cell: &Arc<tokio::sync::RwLock<Option<Arc<alephcore::command::CommandParser>>>>,
    memory_db: &alephcore::memory::store::MemoryBackend,
    memory_ext_registry: &std::sync::Arc<alephcore::memory::extensions::MemoryExtensionRegistry>,
    daemon: bool,
    tool_health: Arc<alephcore::tool_metadata::ToolHealthCache>,
) -> Arc<alephcore::tool_metadata::ToolCatalog> {
    use alephcore::executor::BUILTIN_TOOL_DEFINITIONS;
    use alephcore::tool_metadata::ToolCatalog;

    // Shares the cache the `ExecutionEngine` already holds: the engine is
    // wrapped in an `Arc` long before this runs, so it cannot be handed the
    // catalog's own cache afterwards. Creating one cache up front and giving
    // the same handle to both is what makes the probes registered below
    // reachable from the per-request tool service.
    let tool_catalog = Arc::new(ToolCatalog::with_health(tool_health));

    // Register curated multi-word slash commands (skill_read/skill_list,
    // groupchat, session_new, cron_manage, voice, goal, help).
    tool_catalog.register_builtin_tools().await;

    // Also register executor builtin tools as commands (search, screenshot, ocr, etc.)
    register_builtin_definitions(&tool_catalog).await;

    // Runtime-only shorthand targets: provider-gated generation tools
    // (video/audio/speech_generate) that are NOT in BUILTIN_TOOL_DEFINITIONS,
    // so the defs loop above never sees them. Seed a discovery entry per target
    // — driven by the same single alias source — so /video /audio /speech
    // surface on channels and in /help exactly like /image (whose
    // image_generate IS in defs). Without this the aliases resolve only on the
    // Panel/CLI fast path and stay invisible everywhere else.
    {
        use alephcore::tool_metadata::{
            shorthand_aliases_for, ToolSource as DToolSource, UnifiedTool as DUnifiedTool,
            RUNTIME_ONLY_ALIAS_TARGETS,
        };
        for &(target, description) in RUNTIME_ONLY_ALIAS_TARGETS {
            let aliases = shorthand_aliases_for(target);
            // Defensive: only seed when a shorthand row actually points here.
            // The executability guard test asserts this holds for every entry.
            if aliases.is_empty() {
                continue;
            }
            let tool = DUnifiedTool::new(
                format!("builtin:{target}"),
                target,
                description,
                DToolSource::Builtin,
            )
            .with_aliases(aliases);
            tool_catalog.register_with_conflict_resolution(tool).await;
        }
    }

    // ── Capability health probes (hermes-style runtime gating) ──────────
    // Attach probes to the catalog's shared `ToolHealthCache` so the LLM
    // tool list — and the `<tool_runtime_state>` hints — reflect live
    // capability, not just boot-time registration. The same `Arc` reaches the
    // per-request tool service (engine field -> `build_request_tool_service`
    // -> `ScopedToolService::with_health`), and
    // `ScopedToolService::refreshed_health_snapshot` is what actually runs the
    // probes, TTL-gated and concurrently. Probes are keyed by the executor's
    // LLM-facing tool name, so one fires even when the catalog stores a
    // different slash-command name.
    {
        use alephcore::generation::GenerationType;
        use alephcore::tools::probes::browser::BrowserRuntimeProbe;
        use alephcore::tools::probes::generation::GenerationProbe;

        // Browser: one shared probe (reuses `find_chromium`, with an `npx`
        // fallback for the Playwright-managed backend) gates the whole
        // `browser_*` family. Without any browser runtime the LLM no
        // longer sees ~24 unusable browser tools.
        let browser_probe = Arc::new(BrowserRuntimeProbe::new());
        for def in BUILTIN_TOOL_DEFINITIONS {
            if def.name.starts_with("browser_") {
                tool_catalog.register_health_probe(def.name, browser_probe.clone());
            }
        }

        // Generation: gate each media tool on a live provider for its
        // type. Only register a probe when a provider exists at boot —
        // mirroring the static registration gate in `optional_tools`, so a
        // never-configured capability stays silent while a provider removed
        // mid-session is still caught at runtime by the probe.
        for (tool_name, gen_type) in [
            ("image_generate", GenerationType::Image),
            ("speech_generate", GenerationType::Speech),
            ("audio_generate", GenerationType::Audio),
            ("video_generate", GenerationType::Video),
        ] {
            let has_provider = {
                let reg = generation_registry
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                !reg.providers_for_type(gen_type).is_empty()
            };
            if has_provider {
                tool_catalog.register_health_probe(
                    tool_name,
                    Arc::new(GenerationProbe::new(generation_registry.clone(), gen_type)),
                );
            }
        }

        if !daemon {
            println!("  Capability probes: browser + generation wired to health cache");
        }
    }

    // Register custom commands from config routing rules
    if !app_config.rules.is_empty() {
        tool_catalog
            .register_custom_commands(&app_config.rules)
            .await;
    }

    // Register skills and plugin tools from ExtensionManager (if initialized)
    {
        use alephcore::domain::Entity;
        use alephcore::gateway::handlers::plugins::get_extension_manager;
        if let Ok(ext_manager) = get_extension_manager() {
            // Ensure extensions are discovered and loaded (skills + plugins)
            if let Err(e) = ext_manager.ensure_loaded().await {
                tracing::warn!("Failed to load extensions: {}", e);
            }

            {
                let skill_manifests = ext_manager.skill_system().list_skills().await;
                let skill_infos: Vec<alephcore::skill::SkillInfo> = skill_manifests
                    .iter()
                    .filter(|s| s.is_user_invocable())
                    .map(|s| alephcore::skill::SkillInfo {
                        id: s.id().as_str().to_string(),
                        name: s.name().to_string(),
                        description: s.description().to_string(),
                    })
                    .collect();
                tool_catalog.register_skills(&skill_infos).await;
                if !daemon {
                    println!(
                        "  Dispatch registry: {} skills registered",
                        skill_infos.len()
                    );
                }
            }

            // Register plugin commands (from CC-format plugins' commands/ directories)
            {
                let commands = ext_manager.get_all_commands().await;
                // Gate on `plugin_id`, the field every production construction
                // site actually assigns. This filter read `plugin_name` until
                // 2026-08-19 — a field with zero producers — so it discarded
                // every plugin command ever parsed and this block registered
                // nothing. `qualified_name()` is the registry's own key
                // derivation, so the dispatch id and the lookup key cannot
                // drift apart again.
                let command_skill_infos: Vec<alephcore::skill::SkillInfo> = commands
                    .iter()
                    .filter(|cmd| !cmd.plugin_id.is_empty())
                    .map(|cmd| alephcore::skill::SkillInfo {
                        id: cmd.qualified_name(),
                        name: cmd.name.clone(),
                        description: cmd.description.clone(),
                    })
                    .collect();

                if !command_skill_infos.is_empty() {
                    tool_catalog.register_skills(&command_skill_infos).await;
                    if !daemon {
                        println!(
                            "  Dispatch registry: {} plugin commands registered",
                            command_skill_infos.len()
                        );
                    }
                }
            }

            // Register plugin tools from discovered manifests
            {
                let registry = ext_manager.get_plugin_registry().await;
                let plugin_tools: Vec<(String, String, String)> = registry
                    .list_plugins()
                    .into_iter()
                    .filter(|p| p.status.is_active())
                    .flat_map(|plugin| {
                        match alephcore::extension::manifest::parse_manifest_from_dir_sync(
                            &plugin.root_dir,
                        ) {
                            Ok(manifest) => manifest
                                .tools_v2
                                .unwrap_or_default()
                                .into_iter()
                                .map(|t| {
                                    (
                                        plugin.id.clone(),
                                        t.name.clone(),
                                        t.description.unwrap_or_default(),
                                    )
                                })
                                .collect::<Vec<_>>(),
                            Err(_) => Vec::new(),
                        }
                    })
                    .collect();

                if !plugin_tools.is_empty() {
                    tool_catalog.register_plugin_tools(&plugin_tools).await;
                    if !daemon {
                        println!(
                            "  Dispatch registry: {} plugin tools registered",
                            plugin_tools.len()
                        );
                    }
                }
            }
        }
    }

    if !daemon {
        println!("  Dispatch registry initialized");
    }

    // Wire commands.list to use unified dispatch registry instead of hardcoded builtins
    {
        let reg = tool_catalog.clone();
        server.handlers_mut().register("commands.list", move |req| {
            let registry = reg.clone();
            async move {
                alephcore::gateway::handlers::commands::handle_list_from_registry(req, &registry)
                    .await
            }
        });
        if !daemon {
            println!("  commands.list: wired to unified dispatch registry");
        }
    }

    // Wire tools.catalog to return all active tools grouped by source
    {
        let reg = tool_catalog.clone();
        server.handlers_mut().register("tools.catalog", move |req| {
            let registry = reg.clone();
            async move {
                alephcore::gateway::handlers::tools_visibility::handle_catalog(req, &registry).await
            }
        });
        if !daemon {
            println!("  tools.catalog: wired to unified dispatch registry");
        }
    }

    // Wire tools.invoke to execute a single builtin tool directly,
    // bypassing the LLM agent loop. Intended for E2E test harnesses
    // (note_layer probes, deterministic tool exercising). Production
    // callers should still go through agent.run.
    //
    // D2/P3 fix: pass the live AgentRegistry so handle_invoke can apply
    // the same allowlist that the LLM faces — preventing arbitrary
    // operators from bypassing per-agent tool scoping.
    //
    // Only wire when a real BuiltinToolRegistry is present (real mode);
    // in simulated mode the SERVICE_UNAVAILABLE placeholder from
    // HandlerRegistry::new remains.
    if let Some(reg) = tool_reg_out.clone() {
        let agents_for_invoke: std::sync::Arc<alephcore::agents::AgentRegistry> = {
            let r = std::sync::Arc::new(alephcore::agents::AgentRegistry::with_builtins());
            let aleph_home = alephcore::discovery::aleph_home_dir().ok();
            // B1-03: pass `None` for project_dir at boot. Project agents are
            // scoped per-run via lookup_with_overlay, not loaded into the
            // process-global registry.
            if let Some(home) = aleph_home.as_deref() {
                if let Err(e) = r.register_from_dirs(home, None) {
                    tracing::warn!(
                        error = %e,
                        "tools.invoke: failed to load user agent defs; allowlist degrades to builtins-only"
                    );
                }
            }
            r
        };
        server.handlers_mut().register("tools.invoke", move |req| {
            let registry = reg.clone();
            let agents = Some(agents_for_invoke.clone());
            async move {
                alephcore::gateway::handlers::tools_invoke::handle_invoke(req, registry, agents)
                    .await
            }
        });
        if !daemon {
            println!("  tools.invoke: wired with agent allowlist gating");
        }
    }

    // Wire tools.effective to return tools available to a specific agent.
    // D1 fix: previously rebuilt a builtins-only AgentRegistry per call,
    // hiding user-customized agents. Mirror the orchestrator's setup
    // (mod.rs ~1112 + orchestrator_init.rs:70) by loading user/project
    // AgentDefs from filesystem so the visibility surface matches what
    // the agent loop actually sees.
    {
        let reg = tool_catalog.clone();
        let agent_def_registry = {
            let r = std::sync::Arc::new(alephcore::agents::AgentRegistry::with_builtins());
            let aleph_home = alephcore::discovery::aleph_home_dir().ok();
            // B1-03: pass `None` for project_dir at boot. Project agents are
            // scoped per-run via lookup_with_overlay, not loaded into the
            // process-global registry.
            if let Some(home) = aleph_home.as_deref() {
                match r.register_from_dirs(home, None) {
                    Ok(shadows) => {
                        if !daemon && !shadows.is_empty() {
                            println!(
                                "  tools.effective: loaded user agents (+{} shadow overrides)",
                                shadows.len()
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "tools.effective: failed to load user agent defs; \
                             falling back to builtins-only"
                        );
                    }
                }
            }
            r
        };
        server
            .handlers_mut()
            .register("tools.effective", move |req| {
                let registry = reg.clone();
                let agents = agent_def_registry.clone();
                async move {
                    let agent_id = req
                        .params
                        .as_ref()
                        .and_then(|p| p.get("agent_id"))
                        .and_then(|v| v.as_str())
                        .map(std::string::ToString::to_string);
                    let agent_def = match &agent_id {
                        Some(id) => agents.get(id),
                        None => agents.get("main"),
                    };
                    alephcore::gateway::handlers::tools_visibility::handle_effective(
                        req,
                        &registry,
                        agent_def.as_ref(),
                    )
                    .await
                }
            });
        if !daemon {
            println!("  tools.effective: wired to unified dispatch registry + agent defs");
        }
    }

    // Wire tools.cancel_call + tools.in_flight against the process-wide
    // in-flight registry installed in `start/mod.rs` Gap-B-follow-up boot
    // path. `tools.cancel_call` fires the harness-issued per-call
    // CancellationToken keyed by `tool_call_id`; `tools.in_flight` lists
    // every live registration for CLI / panel diagnostics.
    if let Some(reg) = alephcore::tools::in_flight::global_in_flight_tool_calls() {
        let reg_cancel = reg.clone();
        server
            .handlers_mut()
            .register("tools.cancel_call", move |req| {
                let r = reg_cancel.clone();
                async move {
                    alephcore::gateway::handlers::tools_cancel::handle_cancel(req, r).await
                }
            });
        let reg_list = reg;
        server
            .handlers_mut()
            .register("tools.in_flight", move |req| {
                let r = reg_list.clone();
                async move {
                    alephcore::gateway::handlers::tools_cancel::handle_in_flight(req, r).await
                }
            });
        if !daemon {
            println!("  tools.cancel_call + tools.in_flight: wired to in-flight registry");
        }
    }

    // Wire command.execute to resolve slash commands via CommandParser + ToolRegistry
    {
        let parser = Arc::new(alephcore::command::CommandParser::new(tool_catalog.clone()));

        // Inject parser into chat.send handler (created earlier, uses deferred cell)
        {
            let mut cell = command_parser_cell.write().await;
            *cell = Some(parser.clone());
        }

        let reg = tool_catalog.clone();
        server
            .handlers_mut()
            .register("command.execute", move |req| {
                let p = parser.clone();
                let r = reg.clone();
                async move {
                    alephcore::gateway::handlers::commands::handle_execute(req, p, r).await
                }
            });
        if !daemon {
            println!("  command.execute: wired to unified command parser + registry");
        }
    }

    // ── Spec 4 Task 11: spawn MemoryProducerScheduler ────────────────────
    {
        use alephcore::memory::extensions::MemoryProducerScheduler;
        let raw_store: std::sync::Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore> =
            memory_db.clone();
        let scheduler = std::sync::Arc::new(MemoryProducerScheduler::new(
            memory_ext_registry.clone(),
            raw_store,
        ));
        let _scheduler_handle = scheduler.spawn();
        // JoinHandle intentionally leaked here — the task runs for the
        // server lifetime. Shutdown via process exit.
        if !daemon {
            println!("  MemoryProducerScheduler: spawned");
        }
    }

    // ── Spec 4 Task 11: wire memory_registry into ExtensionManager ───────
    // After the registry is constructed we inject it into the global
    // ExtensionManager so any plugin loaded at runtime via
    // `load_runtime_plugin` / `ensure_plugin_loaded` also gets
    // `load_plugin_with_memory` (MCP memory extension auto-registration).
    {
        use alephcore::gateway::handlers::plugins::get_extension_manager;
        if let Ok(ext_manager) = get_extension_manager() {
            // ExtensionManager is stored behind Arc; we can only thread the
            // registry through the methods that take `&self`.
            // Inject via set_memory_registry if available (no-op if the
            // Arc is already shared across threads).
            ext_manager.set_memory_registry(memory_ext_registry.clone());
        }
    }

    tool_catalog
}

/// Register every entry in `BUILTIN_TOOL_DEFINITIONS` as a `ToolCatalog` row,
/// except for canonical names that the curated `register_builtin_tools` set
/// has already taken (`skill_list`, `skill_read`, `session_new`,
/// `cron_manage`). Without the skip-if-conflict guard, the same-priority
/// Builtin conflict resolves by renaming the defs entry to `name-system` — a
/// ghost the LLM cannot dispatch, and one that crowds the `/help` listing
/// with renamed lookalikes. The curated entry's metadata and aliases win
/// because it was registered first.
pub(super) async fn register_builtin_definitions(
    tool_catalog: &alephcore::tool_metadata::ToolCatalog,
) {
    use alephcore::executor::BUILTIN_TOOL_DEFINITIONS;
    use alephcore::tool_metadata::{
        shorthand_aliases_for, ToolSource as DToolSource, UnifiedTool as DUnifiedTool,
    };

    for def in BUILTIN_TOOL_DEFINITIONS {
        if tool_catalog.check_conflict(def.name).await.is_some() {
            continue;
        }
        let mut tool = DUnifiedTool::new(
            format!("builtin:{}", def.name),
            def.name,
            def.description,
            DToolSource::Builtin,
        );
        let aliases = shorthand_aliases_for(def.name);
        if !aliases.is_empty() {
            tool = tool.with_aliases(aliases);
        }
        tool_catalog.register_with_conflict_resolution(tool).await;
    }
}

#[cfg(test)]
mod tests {
    use super::register_builtin_definitions;
    use alephcore::tool_metadata::ToolCatalog;

    /// After curated `register_builtin_tools()` runs first, the defs loop
    /// must NOT register the overlapping builtins a second time —
    /// `skill_list`, `skill_read`, `session_new`, `cron_manage` are all
    /// already in the curated set, and a second `register_with_conflict_resolution`
    /// of a same-priority `Builtin` would rename the defs entry to
    /// `name-system`, producing a ghost the LLM cannot dispatch.
    #[tokio::test]
    async fn register_builtin_definitions_skips_curated_names() {
        let catalog = ToolCatalog::new();
        catalog.register_builtin_tools().await;

        register_builtin_definitions(&catalog).await;

        let names: Vec<String> = catalog
            .list_root_commands()
            .await
            .into_iter()
            .map(|t| t.name)
            .collect();
        let ghosts: Vec<&String> = names.iter().filter(|n| n.ends_with("-system")).collect();
        assert!(
            ghosts.is_empty(),
            "no `name-system` ghost renames after curated + defs: {names:?}"
        );
        for canonical in ["skill_list", "skill_read", "session_new", "cron_manage"] {
            assert!(
                catalog.check_conflict(canonical).await.is_some(),
                "curated entry for `{canonical}` must still be registered"
            );
        }
    }
}
