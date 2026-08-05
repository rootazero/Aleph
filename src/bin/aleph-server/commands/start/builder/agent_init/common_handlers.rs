//! Common-mode RPC handler registration.
//!
//! Extracted verbatim from `agent_init/mod.rs`. These handlers are wired
//! identically for both the real-execution and simulated branches, after the
//! branch-specific setup completes. Registration order and the readiness
//! signal are preserved exactly.

use alephcore::sync_primitives::Arc;

use alephcore::gateway::handlers::agent::{
    self as agent_handlers, handle_cancel as handle_agent_cancel,
    handle_status as handle_agent_status, AgentRunManager,
};
use alephcore::gateway::handlers::chat as chat_handlers;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::GatewayConfig as FullGatewayConfig;
use alephcore::gateway::GatewayServer;

/// Register `trace.list` / `trace.get` / `trace.by_runs`. When a state database
/// is present they replay durable traces; otherwise they return
/// SERVICE_UNAVAILABLE with an environment-specific reason. Extracted verbatim
/// from the real-execution branch of `agent_init/mod.rs`.
pub(super) fn register_trace_handlers(
    server: &mut GatewayServer,
    resilience_db: Option<Arc<alephcore::resilience::StateDatabase>>,
    session_store: Arc<dyn alephcore::gateway::session_store::SessionStore>,
) {
    // Phase-2 always overrides phase-1 to guarantee a deterministic response.
    // When state DB is absent, the override returns SERVICE_UNAVAILABLE with
    // a tighter, environment-specific reason — never the phase-1 generic.
    if let Some(trace_db) = resilience_db {
        let trace_list_db = trace_db.clone();
        server.handlers_mut().register("trace.list", move |req| {
            let db = trace_list_db.clone();
            async move { alephcore::gateway::handlers::trace_replay::handle_list(req, db).await }
        });

        let trace_get_db = trace_db.clone();
        server.handlers_mut().register("trace.get", move |req| {
            let db = trace_get_db.clone();
            async move { alephcore::gateway::handlers::trace_replay::handle_get(req, db).await }
        });

        // `trace.by_runs` is the one member-reachable method in this family
        // (the Panel replays tool calls on every session open), so it is
        // owner-scoped in the handler rather than admin-gated — it takes the
        // `SessionStore` to KeyCheck the addressed session and to intersect
        // the requested runs with that session's own. See its doc.
        let trace_by_runs_db = trace_db;
        let trace_sessions = session_store;
        server.handlers_mut().register("trace.by_runs", move |req| {
            let db = trace_by_runs_db.clone();
            let sessions = trace_sessions.clone();
            async move {
                alephcore::gateway::handlers::trace_replay::handle_by_runs(req, db, sessions).await
            }
        });
    } else {
        server
            .handlers_mut()
            .register("trace.list", |req| async move {
                alephcore::gateway::protocol::JsonRpcResponse::error(
                    req.id,
                    alephcore::gateway::protocol::SERVICE_UNAVAILABLE,
                    "trace.list disabled: no state_database configured".to_string(),
                )
            });
        server
            .handlers_mut()
            .register("trace.get", |req| async move {
                alephcore::gateway::protocol::JsonRpcResponse::error(
                    req.id,
                    alephcore::gateway::protocol::SERVICE_UNAVAILABLE,
                    "trace.get disabled: no state_database configured".to_string(),
                )
            });
        server
            .handlers_mut()
            .register("trace.by_runs", |req| async move {
                alephcore::gateway::protocol::JsonRpcResponse::error(
                    req.id,
                    alephcore::gateway::protocol::SERVICE_UNAVAILABLE,
                    "trace.by_runs disabled: no state_database configured".to_string(),
                )
            });
    }
}

/// Register status/cancel/chat/agent.list/gateway.* handlers shared by both
/// execution modes, then signal gateway readiness. `session_store` is consumed
/// (moved into the `chat.clear` handler) exactly as in the original inline body.
pub(super) fn register_common_handlers(
    server: &mut GatewayServer,
    run_manager: &Option<Arc<AgentRunManager>>,
    session_store: Arc<dyn alephcore::gateway::session_store::SessionStore>,
    router: &Arc<AgentRouter>,
    full_config: &FullGatewayConfig,
    daemon: bool,
) {
    // Register status/cancel (work for both real and simulated modes)
    if let Some(ref rm) = run_manager {
        let rm_status = rm.clone();
        server.handlers_mut().register("agent.status", move |req| {
            let manager = rm_status.clone();
            async move { handle_agent_status(req, manager).await }
        });

        let rm_cancel = rm.clone();
        server.handlers_mut().register("agent.cancel", move |req| {
            let manager = rm_cancel.clone();
            async move { handle_agent_cancel(req, manager).await }
        });

        // Register chat handlers (abort, history, clear work for both real and simulated)
        let rm_abort = rm.clone();
        let sm_abort = session_store.clone();
        server.handlers_mut().register("chat.abort", move |req| {
            let manager = rm_abort.clone();
            let store = sm_abort.clone();
            async move { chat_handlers::handle_abort(req, manager, store).await }
        });
    }

    let sm_history = session_store.clone();
    server.handlers_mut().register("chat.history", move |req| {
        let manager = sm_history.clone();
        async move { chat_handlers::handle_history(req, manager).await }
    });

    let sm_clear = session_store.clone();
    server.handlers_mut().register("chat.clear", move |req| {
        let manager = sm_clear.clone();
        async move { chat_handlers::handle_clear(req, manager).await }
    });

    let sm_rewind = session_store;
    server.handlers_mut().register("chat.rewind", move |req| {
        let manager = sm_rewind.clone();
        async move { chat_handlers::handle_rewind(req, manager).await }
    });

    // agent.list — returns available agents from the router
    {
        let router_list = router.clone();
        server.handlers_mut().register("agent.list", move |req| {
            let r = router_list.clone();
            async move { agent_handlers::handle_list(req, r).await }
        });
    }

    if !daemon {
        println!("Agent control methods:");
        println!("  - agent.run            : Execute agent request with streaming");
        println!("  - agent.status         : Query run status by run_id");
        println!("  - agent.cancel         : Cancel an active run");
        println!("  - agent.list           : List available agents");
        println!("  - chat.send            : Send chat message (wraps agent.run)");
        println!("  - chat.abort           : Abort message generation");
        println!("  - chat.history         : Get chat history");
        println!("  - chat.clear           : Clear chat history");
        println!();
    }

    // G4: register gateway.identity.get with a small captured snapshot.
    // The snapshot deliberately omits Arc<GatewaySharedState> — capturing
    // the handler-registry Arc here would make this very handlers_mut()
    // call (which uses Arc::get_mut) panic.
    {
        use alephcore::gateway::handlers::gateway_identity::{
            handle_gateway_identity_get, GatewayIdentitySnapshot,
        };
        // +1 accounts for the gateway.identity.get handler itself, which
        // is registered immediately after this count is taken.
        let method_count = server.handlers_mut().len() + 1;
        let identity_snapshot = GatewayIdentitySnapshot {
            instance_id: server.instance_id.clone(),
            started_at_unix: server.started_at_unix,
            state_versions: server.state_versions.clone(),
            method_count,
        };
        server
            .handlers_mut()
            .register("gateway.identity.get", move |req| {
                let snap = identity_snapshot.clone();
                async move { handle_gateway_identity_get(req, snap).await }
            });
        if !daemon {
            println!("  gateway.identity.get: wired");
        }
    }

    // G4b: register gateway.metrics.lanes with the live LaneManager.
    // Cloning Arc<LaneManager> is cheap; the handler reads available_permits()
    // off the underlying tokio::Semaphore — racy by design (gauge, not txn).
    {
        use alephcore::gateway::handlers::gateway_metrics::handle_gateway_metrics_lanes;
        let lane_mgr = server.lane_manager.clone();
        server
            .handlers_mut()
            .register("gateway.metrics.lanes", move |req| {
                let mgr = lane_mgr.clone();
                async move { handle_gateway_metrics_lanes(req, mgr).await }
            });
        if !daemon {
            println!("  gateway.metrics.lanes: wired");
        }
    }

    // G4c: register gateway.credentials with a snapshot of the live
    // GatewayServerConfig. Cloning the config into an Arc once at boot is
    // cheap and avoids holding the FullGatewayConfig across the handler.
    {
        use alephcore::gateway::handlers::gateway_credentials::handle_gateway_credentials;
        let gateway_cfg = std::sync::Arc::new(full_config.gateway.clone());
        server
            .handlers_mut()
            .register("gateway.credentials", move |req| {
                let cfg = gateway_cfg.clone();
                async move { handle_gateway_credentials(req, cfg).await }
            });
        if !daemon {
            println!("  gateway.credentials: wired");
        }
    }

    // G4d: register gateway.metrics.run_concurrency with the live
    // AgentRunManager (Task 8, audit 3.4). Reaches the execution engine's
    // `ConcurrencyLimiter` snapshot through the same `Arc<dyn
    // ExecutionAdapter>` indirection `agent.status`/`agent.cancel` already
    // use — `run_manager` is `Some` in both real and simulated boot modes.
    if let Some(ref rm) = run_manager {
        use alephcore::gateway::handlers::gateway_metrics::handle_gateway_metrics_run_concurrency;
        let rm_concurrency = rm.clone();
        server
            .handlers_mut()
            .register("gateway.metrics.run_concurrency", move |req| {
                let manager = rm_concurrency.clone();
                async move { handle_gateway_metrics_run_concurrency(req, manager).await }
            });
        if !daemon {
            println!("  gateway.metrics.run_concurrency: wired");
        }
    }

    // G2: signal readiness. /ready returns 200 from this point onward;
    // before this, it returns 503 so proxies don't route to a gateway
    // whose handler tree is still being wired.
    server
        .ready
        .store(true, std::sync::atomic::Ordering::Release);
    if !daemon {
        println!("  Gateway readiness: signaled (ready=true)");
    }
}
