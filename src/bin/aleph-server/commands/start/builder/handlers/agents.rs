use super::{Arc, GatewayServer, MemoryBackend};

pub(in crate::commands::start) fn register_agents_handlers(
    server: &mut GatewayServer,
    manager: &Arc<alephcore::AgentManager>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    // Runtime-sync context: keeps the runtime AgentRegistry + channel bindings
    // in lock-step with TOML writes (create → hot-register, delete → evict +
    // unbind). `None` on a minimal server → TOML-only, prior behavior.
    runtime: Option<Arc<alephcore::gateway::handlers::agents::AgentsRuntimeCtx>>,
) {
    use alephcore::gateway::handlers::agents;

    register_handler!(server, "agents.list", agents::handle_list, manager);
    register_handler!(server, "agents.get", agents::handle_get, manager);
    // agents.create / agents.update / agents.delete are hand-wired (not
    // `register_handler!`) because they take an `Option<Arc<AgentsRuntimeCtx>>`
    // — the macro only threads required `Arc`s.
    {
        let manager = Arc::clone(manager);
        let event_bus = Arc::clone(event_bus);
        let runtime = runtime.clone();
        server.handlers_mut().register("agents.create", move |req| {
            let manager = Arc::clone(&manager);
            let event_bus = Arc::clone(&event_bus);
            let runtime = runtime.clone();
            async move { agents::handle_create(req, manager, event_bus, runtime).await }
        });
    }
    {
        let manager = Arc::clone(manager);
        let event_bus = Arc::clone(event_bus);
        let runtime = runtime.clone();
        server.handlers_mut().register("agents.update", move |req| {
            let manager = Arc::clone(&manager);
            let event_bus = Arc::clone(&event_bus);
            let runtime = runtime.clone();
            async move { agents::handle_update(req, manager, event_bus, runtime).await }
        });
    }
    {
        let manager = Arc::clone(manager);
        let event_bus = Arc::clone(event_bus);
        let runtime = runtime.clone();
        server.handlers_mut().register("agents.delete", move |req| {
            let manager = Arc::clone(&manager);
            let event_bus = Arc::clone(&event_bus);
            let runtime = runtime.clone();
            async move { agents::handle_delete(req, manager, event_bus, runtime).await }
        });
    }
    register_handler!(
        server,
        "agents.set_default",
        agents::handle_set_default,
        manager,
        event_bus
    );
    register_handler!(
        server,
        "agents.files.list",
        agents::handle_files_list,
        manager
    );
    register_handler!(
        server,
        "agents.files.get",
        agents::handle_files_get,
        manager
    );
    register_handler!(
        server,
        "agents.files.set",
        agents::handle_files_set,
        manager
    );
    register_handler!(
        server,
        "agents.files.delete",
        agents::handle_files_delete,
        manager
    );

    // Stateless handler — no manager dependency
    server
        .handlers_mut()
        .register("agents.tools_schema", |req| async move {
            agents::handle_tools_schema(req).await
        });

    // runtimes.install — overrides the event_bus-less placeholder in
    // handlers/mod.rs (which always errors). Wired here because the install
    // progress stream needs the GatewayEventBus. The capability ledger is
    // rebuilt per call from disk, mirroring runtimes.list / runtimes.refresh.
    {
        let event_bus = Arc::clone(event_bus);
        server
            .handlers_mut()
            .register("runtimes.install", move |req| {
                let event_bus = Arc::clone(&event_bus);
                async move {
                    use alephcore::gateway::handlers::{make_runtime_ledger, runtimes};
                    let ledger = match make_runtime_ledger() {
                        Ok(l) => l,
                        Err(e) => {
                            return alephcore::gateway::protocol::JsonRpcResponse::error(
                                req.id,
                                alephcore::gateway::protocol::INTERNAL_ERROR,
                                e,
                            )
                        }
                    };
                    runtimes::handle_install(req, ledger, event_bus).await
                }
            });
    }
}

// ─── register_cron_handlers ─────────────────────────────────────────────────

pub(in crate::commands::start) fn register_cron_handlers(
    server: &mut GatewayServer,
    cron_service: &alephcore::tasks::cron::SharedCronService,
    daemon: bool,
) {
    use alephcore::gateway::handlers::cron;

    register_handler!(server, "cron.list", cron::handle_list, cron_service);
    register_handler!(server, "cron.get", cron::handle_get, cron_service);
    register_handler!(server, "cron.create", cron::handle_create, cron_service);
    register_handler!(server, "cron.update", cron::handle_update, cron_service);
    register_handler!(server, "cron.delete", cron::handle_delete, cron_service);
    register_handler!(server, "cron.status", cron::handle_status, cron_service);
    register_handler!(server, "cron.run", cron::handle_run, cron_service);
    register_handler!(server, "cron.runs", cron::handle_runs, cron_service);
    register_handler!(server, "cron.toggle", cron::handle_toggle, cron_service);

    if !daemon {
        println!("Cron service: enabled (RPC handlers registered)");
        println!();
    }
}

// ─── register_heartbeat_handlers ────────────────────────────────────────────

pub(in crate::commands::start) fn register_heartbeat_handlers(
    server: &mut GatewayServer,
    heartbeat_service: &alephcore::tasks::heartbeat::SharedHeartbeatService,
    daemon: bool,
) {
    use alephcore::gateway::handlers::heartbeat;

    register_handler!(
        server,
        "heartbeat.list",
        heartbeat::handle_list,
        heartbeat_service
    );
    register_handler!(
        server,
        "heartbeat.get",
        heartbeat::handle_get,
        heartbeat_service
    );
    register_handler!(
        server,
        "heartbeat.create",
        heartbeat::handle_create,
        heartbeat_service
    );
    register_handler!(
        server,
        "heartbeat.update",
        heartbeat::handle_update,
        heartbeat_service
    );
    register_handler!(
        server,
        "heartbeat.delete",
        heartbeat::handle_delete,
        heartbeat_service
    );
    register_handler!(
        server,
        "heartbeat.toggle",
        heartbeat::handle_toggle,
        heartbeat_service
    );
    register_handler!(
        server,
        "heartbeat.wake",
        heartbeat::handle_wake,
        heartbeat_service
    );
    register_handler!(
        server,
        "heartbeat.runs",
        heartbeat::handle_runs,
        heartbeat_service
    );

    if !daemon {
        println!("Heartbeat service: enabled (RPC handlers registered)");
        println!();
    }
}

// ─── register_teams_handlers ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(in crate::commands::start) fn register_teams_handlers(
    server: &mut GatewayServer,
    store: &Arc<dyn alephcore::teams::TeamStore>,
    coord_store: &Arc<dyn alephcore::agents::swarm::tasks::CoordTaskStore>,
    event_store: Option<&Arc<dyn alephcore::teams::events::EventLogStore>>,
    snapshot_store: Option<&Arc<alephcore::teams::SqliteSnapshotStore>>,
    state_db: Option<&Arc<alephcore::resilience::StateDatabase>>,
    artifact_store: Option<&Arc<dyn alephcore::teams::artifacts::ArtifactStore>>,
    msg_store: Option<Arc<dyn alephcore::teams::messages::MessageStore>>,
    agent_manager: Option<Arc<alephcore::AgentManager>>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::teams;

    // NOTE: teams.chat.send is registered in agent_init/mod.rs (it needs the execution context, unlike these store-only handlers).
    register_handler!(server, "teams.list", teams::handle_list, store);
    register_handler!(server, "teams.get", teams::handle_get, store, coord_store);
    // create / rename / disband publish `team.changed` so the group-chat sidebar
    // and the teams tab stay in sync without a manual refresh.
    register_handler!(
        server,
        "teams.create",
        teams::handle_create,
        store,
        event_bus
    );
    register_handler!(
        server,
        "teams.rename",
        teams::handle_rename,
        store,
        event_bus
    );
    register_handler!(
        server,
        "teams.disband",
        teams::handle_disband,
        store,
        event_bus
    );
    // teams.delete — cascade when all four subordinate stores are configured;
    // fall back to single-store delete (legacy behavior) otherwise.
    match (
        event_store,
        snapshot_store,
        artifact_store,
        msg_store.clone(),
    ) {
        (Some(ev), Some(snap), Some(art), Some(msg)) => {
            let store_c = Arc::clone(store);
            let coord_c = Arc::clone(coord_store);
            let ev_c = Arc::clone(ev);
            let snap_c = Arc::clone(snap);
            let art_c = Arc::clone(art);
            let msg_c = Arc::clone(&msg);
            let bus_c = Arc::clone(event_bus);
            server.handlers_mut().register("teams.delete", move |req| {
                let store_c = Arc::clone(&store_c);
                let coord_c = Arc::clone(&coord_c);
                let ev_c = Arc::clone(&ev_c);
                let snap_c = Arc::clone(&snap_c);
                let art_c = Arc::clone(&art_c);
                let msg_c = Arc::clone(&msg_c);
                let bus_c = Arc::clone(&bus_c);
                async move {
                    teams::handle_delete(req, store_c, coord_c, msg_c, ev_c, art_c, snap_c, bus_c)
                        .await
                }
            });
        }
        _ => {
            // Fallback: TeamStore-only delete (old behavior); subordinate store
            // absence is acceptable in simulated / test configurations.
            register_handler!(
                server,
                "teams.delete",
                teams::handle_delete_basic,
                store,
                event_bus
            );
        }
    }

    // agents.teams — enriched with members_preview + last_message when the
    // optional agent_manager and msg_store are wired at boot.
    {
        let ts = Arc::clone(store);
        let am = agent_manager.clone();
        let ms = msg_store.clone();
        server.handlers_mut().register("agents.teams", move |req| {
            let ts = Arc::clone(&ts);
            let am = am.clone();
            let ms = ms.clone();
            async move { teams::handle_agent_teams(req, ts, am, ms).await }
        });
    }
    // Templates RPC is stateless — the handler discovers builtins +
    // ~/.aleph/teams/templates on every call. Materialization itself runs
    // through the `team_from_template` builtin tool (R8) so we don't
    // double-plumb the heavy dep set through the gateway.
    register_handler!(server, "teams.list_templates", teams::handle_list_templates);
    register_handler!(
        server,
        "teams.list_tasks",
        teams::handle_list_tasks,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.create_task",
        teams::handle_create_task,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.update_task",
        teams::handle_update_task,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.list_task_runs",
        teams::handle_list_task_runs,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.add_task_comment",
        teams::handle_add_task_comment,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.list_task_comments",
        teams::handle_list_task_comments,
        store,
        coord_store
    );

    // Phase C — step-level workflow review RPCs. Panel calls these from
    // the workflow-graph drawer; the lead agent can also drive them via
    // the workflow_step_review builtin tool (R8).
    register_handler!(
        server,
        "teams.workflow.approve_step",
        teams::handle_workflow_approve_step,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.workflow.reject_step",
        teams::handle_workflow_reject_step,
        store,
        coord_store
    );

    // R3 — task-level control surface. Pause/resume/retry/skip operate on
    // any task state (admin-context), complementing the reviewer-context
    // workflow.* family. See `team_task_control` builtin for the
    // LLM-facing equivalent.
    register_handler!(
        server,
        "teams.task.pause",
        teams::handle_task_pause,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.task.resume",
        teams::handle_task_resume,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.task.retry",
        teams::handle_task_retry,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.task.skip",
        teams::handle_task_skip,
        store,
        coord_store
    );

    // R3 — teams.task.trace: unified audit timeline aggregating runs +
    // comments + events + artifacts in one round-trip. Both optional
    // stores degrade to empty arrays when unavailable rather than
    // erroring (best-effort observability). Done by hand because the
    // register_handler! macro is fixed to 1-3 mandatory Arcs — here we
    // need two Option<Arc<_>> contexts.
    {
        let ts = Arc::clone(store);
        let cs = Arc::clone(coord_store);
        let es = event_store.cloned();
        let ar = artifact_store.cloned();
        server
            .handlers_mut()
            .register("teams.task.trace", move |req| {
                let ts = Arc::clone(&ts);
                let cs = Arc::clone(&cs);
                let es = es.clone();
                let ar = ar.clone();
                async move { teams::handle_task_trace(req, ts, cs, es, ar).await }
            });
    }

    // teams.chat.thread — durable team work thread hydrate (tasks + artifacts
    // merged chronologically). Done by hand like teams.task.trace because it
    // takes coord_store (mandatory) + artifact_store (optional Option<Arc>).
    {
        let ts = Arc::clone(store);
        let cs = Arc::clone(coord_store);
        let ar = artifact_store.cloned();
        server
            .handlers_mut()
            .register("teams.chat.thread", move |req| {
                let ts = Arc::clone(&ts);
                let cs = Arc::clone(&cs);
                let ar = ar.clone();
                async move { teams::handle_chat_thread(req, ts, cs, ar).await }
            });
    }

    // teams.chat.history — replay durable group-chat transcript as attribution
    // bubbles. Only registered when the message store is wired at boot.
    if let Some(ms) = msg_store.clone() {
        let ts = Arc::clone(store);
        server
            .handlers_mut()
            .register("teams.chat.history", move |req| {
                let ts = Arc::clone(&ts);
                let ms = Arc::clone(&ms);
                async move { teams::handle_chat_history(req, ts, ms).await }
            });
    }

    // R3 — exit journal read surface. Write is via the
    // `task_exit_journal` builtin tool (LLM-self-called).
    register_handler!(
        server,
        "teams.task.journal.get",
        teams::handle_task_journal_get,
        store,
        coord_store
    );
    register_handler!(
        server,
        "teams.task.journal.list",
        teams::handle_task_journal_list,
        store,
        coord_store
    );

    // ACP-backed team member operations — attach an external coding CLI
    // session (Claude Code, Codex, ...) as a first-class team member so
    // the autonomous dispatcher can route tasks to it.
    register_handler!(
        server,
        "teams.acp_member.add",
        teams::handle_acp_member_add,
        store
    );
    register_handler!(
        server,
        "teams.acp_member.remove",
        teams::handle_acp_member_remove,
        store
    );
    register_handler!(
        server,
        "teams.acp_member.list",
        teams::handle_acp_member_list,
        store
    );

    // Event timeline RPC is best-effort: only wired when event_store was
    // constructed at boot. Without it, the placeholder in handlers/mod.rs
    // surfaces a clear "store unavailable" error.
    if let Some(es) = event_store {
        register_handler!(
            server,
            "teams.list_task_events",
            teams::handle_list_task_events,
            store,
            coord_store,
            es
        );
    }

    // Snapshot lifecycle RPCs — only wired when the SnapshotStore was
    // constructed at boot (it shares the coord.db connection). Callers can
    // still drive snapshots through the `team_snapshot` builtin tool when
    // these handlers are absent.
    if let Some(snap) = snapshot_store {
        register_handler!(
            server,
            "teams.snapshot.create",
            teams::handle_snapshot_create,
            store,
            coord_store,
            snap
        );
        register_handler!(
            server,
            "teams.snapshot.list",
            teams::handle_snapshot_list,
            store,
            snap
        );
        register_handler!(
            server,
            "teams.snapshot.get",
            teams::handle_snapshot_get,
            store,
            snap
        );
        register_handler!(
            server,
            "teams.snapshot.restore",
            teams::handle_snapshot_restore,
            store,
            coord_store,
            snap
        );
        register_handler!(
            server,
            "teams.snapshot.delete",
            teams::handle_snapshot_delete,
            store,
            snap
        );
    }

    // teams.usage — per-team provider token aggregation. Requires state.db
    // (where ProviderUsage trace events live); in simulated mode the handler
    // is simply absent and the placeholder in handlers/mod.rs surfaces a clear
    // error.
    if let Some(db) = state_db {
        register_handler!(server, "teams.usage", teams::handle_usage, store, db);
    }
}

// ─── register_graph_handlers ──────────────────────────────────────────────────

pub(in crate::commands::start) fn register_graph_handlers(
    server: &mut GatewayServer,
    memory_db: &MemoryBackend,
    indexer: Option<
        &::std::sync::Arc<
            alephcore::memory::notes::NoteIndexer<
                alephcore::memory::store::sqlite::SqliteMemoryBackend,
            >,
        >,
    >,
) {
    use alephcore::gateway::handlers::graph;

    {
        let db = ::std::sync::Arc::clone(memory_db);
        server.handlers_mut().register("graph.query", move |req| {
            let db = ::std::sync::Arc::clone(&db);
            async move { graph::handle_query_impl(req, db).await }
        });
    }

    {
        let db = ::std::sync::Arc::clone(memory_db);
        server
            .handlers_mut()
            .register("graph.node_detail", move |req| {
                let db = ::std::sync::Arc::clone(&db);
                async move { graph::handle_node_detail_impl(req, db).await }
            });
    }

    {
        let db = ::std::sync::Arc::clone(memory_db);
        server.handlers_mut().register("graph.search", move |req| {
            let db = ::std::sync::Arc::clone(&db);
            async move { graph::handle_search_impl(req, db).await }
        });
    }

    // graph.update_note / graph.rename_note / graph.delete_note need the
    // NoteIndexer (write path). Only registered when the indexer is available;
    // otherwise these methods are simply absent (METHOD_NOT_FOUND).
    if let Some(indexer) = indexer {
        {
            let indexer = ::std::sync::Arc::clone(indexer);
            server
                .handlers_mut()
                .register("graph.update_note", move |req| {
                    let indexer = ::std::sync::Arc::clone(&indexer);
                    async move { graph::handle_update_note_impl(req, indexer).await }
                });
        }
        {
            let indexer = ::std::sync::Arc::clone(indexer);
            server
                .handlers_mut()
                .register("graph.rename_note", move |req| {
                    let indexer = ::std::sync::Arc::clone(&indexer);
                    async move { graph::handle_rename_note_impl(req, indexer).await }
                });
        }
        {
            let indexer = ::std::sync::Arc::clone(indexer);
            server
                .handlers_mut()
                .register("graph.delete_note", move |req| {
                    let indexer = ::std::sync::Arc::clone(&indexer);
                    async move { graph::handle_delete_note_impl(req, indexer).await }
                });
        }
    }
}
