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
///
/// ⚠️ **Ordering**: `trace.list`/`trace.get` capture the server's
/// `SecurityAuditLog` here so they can record cross-user content reads (human
/// ruling, 2026-08-07 — see `handlers::trace_replay`'s module doc). That means
/// this call must stay AFTER `GatewayServer::set_audit_log`, which today it is
/// (the setter runs in `start/mod.rs` well before `register_agent_handlers`).
/// Move it earlier and the two handlers keep working while the audit trail
/// silently disappears — the exact severed-wire shape this repo keeps paying
/// for. `None` is a legitimate value only in test/probe servers, which is why
/// this is a comment rather than an assertion.
pub(super) fn register_trace_handlers(
    server: &mut GatewayServer,
    resilience_db: Option<Arc<alephcore::resilience::StateDatabase>>,
    session_store: Arc<dyn alephcore::gateway::session_store::SessionStore>,
) {
    let audit_log = server.audit_log();
    // Phase-2 always overrides phase-1 to guarantee a deterministic response.
    // When state DB is absent, the override returns SERVICE_UNAVAILABLE with
    // a tighter, environment-specific reason — never the phase-1 generic.
    if let Some(trace_db) = resilience_db {
        let trace_list_db = trace_db.clone();
        let trace_list_sessions = session_store.clone();
        let trace_list_audit = audit_log.clone();
        server.handlers_mut().register("trace.list", move |req| {
            let db = trace_list_db.clone();
            let sessions = trace_list_sessions.clone();
            let audit = trace_list_audit.clone();
            async move {
                alephcore::gateway::handlers::trace_replay::handle_list(req, db, sessions, audit)
                    .await
            }
        });

        let trace_get_db = trace_db.clone();
        let trace_get_sessions = session_store.clone();
        let trace_get_audit = audit_log;
        server.handlers_mut().register("trace.get", move |req| {
            let db = trace_get_db.clone();
            let sessions = trace_get_sessions.clone();
            let audit = trace_get_audit.clone();
            async move {
                alephcore::gateway::handlers::trace_replay::handle_get(req, db, sessions, audit)
                    .await
            }
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
/// execution modes, then signal gateway readiness. `session_store` is cloned
/// into each handler that needs it; the last consumer is
/// `gateway.metrics.run_concurrency`, which narrows its session-key arrays to
/// the caller (see that handler's doc).
pub(super) fn register_common_handlers(
    server: &mut GatewayServer,
    run_manager: &Option<Arc<AgentRunManager>>,
    session_store: Arc<dyn alephcore::gateway::session_store::SessionStore>,
    router: &Arc<AgentRouter>,
    agent_registry: &Option<Arc<alephcore::gateway::agent_instance::AgentRegistry>>,
    full_config: &FullGatewayConfig,
    daemon: bool,
) {
    // Register status/cancel (work for both real and simulated modes)
    if let Some(ref rm) = run_manager {
        // Both take `session_store`: a bare `run_id` is a caller-supplied
        // identifier like any other, so it resolves run → session → the one
        // visibility predicate before either handler acts on it.
        let rm_status = rm.clone();
        let sm_status = session_store.clone();
        server.handlers_mut().register("agent.status", move |req| {
            let manager = rm_status.clone();
            let store = sm_status.clone();
            async move { handle_agent_status(req, manager, store).await }
        });

        let rm_cancel = rm.clone();
        let sm_cancel = session_store.clone();
        server.handlers_mut().register("agent.cancel", move |req| {
            let manager = rm_cancel.clone();
            let store = sm_cancel.clone();
            async move { handle_agent_cancel(req, manager, store).await }
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

    // Registered unconditionally (outside the `run_manager` branch above) so a
    // transcript is always readable. `run_manager` is threaded in as an Option
    // only to answer "is a turn in flight on this session right now" — see
    // `handle_history`'s doc for why that pointer rides on this response.
    let sm_history = session_store.clone();
    let rm_history = run_manager.clone();
    server.handlers_mut().register("chat.history", move |req| {
        let manager = sm_history.clone();
        let runs = rm_history.clone();
        async move { chat_handlers::handle_history(req, manager, runs).await }
    });

    let sm_clear = session_store.clone();
    server.handlers_mut().register("chat.clear", move |req| {
        let manager = sm_clear.clone();
        async move { chat_handlers::handle_clear(req, manager).await }
    });

    let sm_rewind = session_store.clone();
    server.handlers_mut().register("chat.rewind", move |req| {
        let manager = sm_rewind.clone();
        async move { chat_handlers::handle_rewind(req, manager).await }
    });

    // agent.resume — the on-demand half of the boot resume scan. Registered
    // unconditionally (not under `run_manager`, and not under `[resume]
    // enabled`): the handler resolves the coordinator itself at call time and
    // answers honestly when there is none, which is strictly better than a
    // method that silently does not exist on some boots.
    //
    // `agent_registry` is what makes the agent-admission gate real on this
    // face: `agent.resume` is member-open (`method_admin.rs`), so without it a
    // revoked user could put an agent back to work. `None` is the
    // Simulated-execution build, which has no registry and runs no tools —
    // see `resume_named_session`'s doc for why that is a construction fact
    // rather than a hole, and why this is a parameter and not a global.
    let sm_resume = session_store.clone();
    let reg_resume = agent_registry.clone();
    server.handlers_mut().register("agent.resume", move |req| {
        let manager = sm_resume.clone();
        let agents = reg_resume.clone();
        async move {
            alephcore::gateway::handlers::resume::handle_resume(req, manager, agents).await
        }
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
    // use — `run_manager` is `Some` in both real and simulated boot modes. It
    // also takes the `SessionStore`: the two session-key arrays it returns are
    // narrowed to the caller (the RPC half of the `stream.running_set_changed`
    // projection — see that handler's doc), which needs each key's owner row.
    if let Some(ref rm) = run_manager {
        use alephcore::gateway::handlers::gateway_metrics::handle_gateway_metrics_run_concurrency;
        let rm_concurrency = rm.clone();
        let concurrency_sessions = session_store.clone();
        server
            .handlers_mut()
            .register("gateway.metrics.run_concurrency", move |req| {
                let manager = rm_concurrency.clone();
                let sessions = concurrency_sessions.clone();
                async move { handle_gateway_metrics_run_concurrency(req, manager, sessions).await }
            });
        if !daemon {
            println!("  gateway.metrics.run_concurrency: wired");
        }
    }

    // Round-8 (§4.11): register gateway.metrics.subagent_concurrency. The
    // handler reaches the process-global `BackgroundAgentTracker` directly
    // (no per-instance state), so registration is unconditional — unlike
    // `run_concurrency` which depends on `run_manager` being wired. Reading
    // the snapshot is O(running + completed) and lock-only; safe to call
    // from any Query-lane caller (panel, CLI, doctor).
    {
        use alephcore::gateway::handlers::gateway_metrics::handle_gateway_metrics_subagent_concurrency;
        server.handlers_mut().register(
            "gateway.metrics.subagent_concurrency",
            move |req| async move { handle_gateway_metrics_subagent_concurrency(req).await },
        );
        if !daemon {
            println!("  gateway.metrics.subagent_concurrency: wired");
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
