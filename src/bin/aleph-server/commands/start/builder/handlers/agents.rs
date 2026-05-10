use super::*;

pub(in crate::commands::start) fn register_agents_handlers(
    server: &mut GatewayServer,
    manager: &Arc<alephcore::AgentManager>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::agents;

    register_handler!(server, "agents.list", agents::handle_list, manager);
    register_handler!(server, "agents.get", agents::handle_get, manager);
    register_handler!(
        server,
        "agents.create",
        agents::handle_create,
        manager,
        event_bus
    );
    register_handler!(
        server,
        "agents.update",
        agents::handle_update,
        manager,
        event_bus
    );
    register_handler!(
        server,
        "agents.delete",
        agents::handle_delete,
        manager,
        event_bus
    );
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

pub(in crate::commands::start) fn register_teams_handlers(
    server: &mut GatewayServer,
    store: &Arc<dyn alephcore::teams::TeamStore>,
    coord_store: &Arc<dyn alephcore::agents::swarm::tasks::CoordTaskStore>,
) {
    use alephcore::gateway::handlers::teams;

    register_handler!(server, "teams.list", teams::handle_list, store);
    register_handler!(server, "teams.get", teams::handle_get, store, coord_store);
    register_handler!(server, "teams.disband", teams::handle_disband, store);
    register_handler!(server, "teams.delete", teams::handle_delete, store);
    register_handler!(server, "agents.teams", teams::handle_agent_teams, store);
}

// ─── register_graph_handlers ──────────────────────────────────────────────────

pub(in crate::commands::start) fn register_graph_handlers(
    server: &mut GatewayServer,
    memory_db: &MemoryBackend,
    _default_agent_id: &str,
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
            .register("graph.neighbors", move |req| {
                let db = ::std::sync::Arc::clone(&db);
                async move { graph::handle_neighbors_impl(req, db).await }
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
}
