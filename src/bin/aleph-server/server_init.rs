//! Server initialization helpers for Aleph Gateway
//!
//! This module contains helper functions for server initialization,
//! including `WebChat` serving and agent run handling.

use std::net::SocketAddr;
use std::path::PathBuf;

use alephcore::sync_primitives::Arc;

use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::{
    AgentRegistry, EventEmitter, ExecutionEngine, GatewayEventEmitter, StreamEvent,
};

/// Spawn a Panel / CLI run onto the shared per-session busy wait lane.
///
/// Both RPC entry points (`agent.run` and `chat.send`) route through here so
/// the local surfaces get the same follow-up queue the inbound router gives
/// external channels. Before this, a Panel message that could not be delivered
/// mid-run — an attachment-bearing steer (which `try_inject_steering`
/// deliberately defers, because a steering event has no media seam) or a
/// steering burst at `max_pending_steering` — surfaced as an `AgentBusy` error
/// bubble and the message was simply lost. Now it waits its turn like every
/// other queued message.
///
/// Error reporting is split by whether the attempt actually ran: the engine
/// already emits `RunError` for anything that fails *inside* `execute`, so
/// re-emitting here would show the Panel two error bubbles for one failure.
/// Only the queue-stage outcomes (lane full, waited out the window) are this
/// function's to report; a purge (`/stop`) is announced once by the stop
/// handler for the whole batch.
fn spawn_queued_run<P, R>(
    engine: Arc<ExecutionEngine<P, R>>,
    run_request: alephcore::gateway::RunRequest,
    agent: Arc<alephcore::gateway::agent_instance::AgentInstance>,
    emitter: Arc<GatewayEventEmitter>,
    busy_cfg: alephcore::gateway::busy_queue::BusyQueueConfig,
    locale: alephcore::gateway::i18n::Locale,
    label: &'static str,
) where
    P: alephcore::thinker::ProviderRegistry + 'static,
    R: alephcore::executor::ToolRegistry + 'static,
{
    use alephcore::gateway::busy_queue::{deliver_with_ticket, register, DeliveryOutcome};

    let run_id = run_request.run_id.clone();
    let session_key = run_request.session_key.to_key_string();
    let agent_id = run_request.session_key.agent_id().to_string();
    // Take the FIFO ticket synchronously on the arrival path so lane order
    // matches arrival order rather than task-scheduling order.
    let ticket = register(&session_key, busy_cfg.max_per_session, &run_id);

    tokio::spawn(async move {
        let outcome = match ticket {
            None => DeliveryOutcome::Rejected,
            Some(ticket) => {
                let mut attempt =
                    || engine.execute(run_request.clone(), agent.clone(), emitter.clone());
                deliver_with_ticket(ticket, busy_cfg, &mut attempt).await
            }
        };

        // Which failure (if any) this surface still owes a terminal frame for.
        // `DeliveryOutcome::user_error` covers the two never-ran outcomes; the
        // extra `Purged` arm is surface-specific: a channel hears about a purge
        // once, in the `/stop` reply, but the Panel has an in-flight bubble
        // keyed to this `run_id` and nothing else will ever close it — without a
        // terminal frame the user is left with a spinner they just stopped.
        let pending_error = match outcome {
            DeliveryOutcome::Executed(Ok(())) => {
                tracing::info!(run_id = %run_id, "{label} completed successfully");
                return;
            }
            DeliveryOutcome::Executed(Err(ref e)) => {
                // Raw chain goes to server logs only; the engine already put a
                // sanitized receipt on the wire (shared single source with
                // `ExecutionError::user_receipt`).
                tracing::error!(run_id = %run_id, error = %e, "{label} failed");
                None
            }
            DeliveryOutcome::Purged => {
                tracing::info!(run_id = %run_id, session = %session_key,
                    "{label} dropped: an explicit stop abandoned it while it was queued");
                Some(alephcore::gateway::execution_engine::ExecutionError::Cancelled)
            }
            DeliveryOutcome::Rejected | DeliveryOutcome::TimedOut => outcome.user_error(&agent_id),
        };

        let Some(e) = pending_error else {
            return;
        };
        tracing::warn!(run_id = %run_id, session = %session_key, error = %e,
            "{label} never reached the agent");
        let (error_code, error_message) = e.user_receipt(locale);
        let seq = emitter.next_seq();
        if let Err(emit_err) = emitter
            .emit(StreamEvent::RunError {
                run_id: run_id.clone(),
                seq,
                error: error_message,
                error_code: Some(error_code.to_string()),
            })
            .await
        {
            tracing::warn!("Failed to emit run error event: {}", emit_err);
        }
    });
}

/// Serve `WebChat` static files
pub async fn serve_webchat(
    addr: SocketAddr,
    static_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use axum::Router;
    use tower_http::services::{ServeDir, ServeFile};

    tracing::info!("Starting WebChat server on http://{}", addr);

    // Create fallback for SPA routing
    let index_path = static_dir.join("index.html");
    let serve_dir = ServeDir::new(&static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(&index_path));

    // Loopback-only origin rule, shared with the `/ws` upgrade guard so the
    // same-machine parsing lives in one place (`OriginPolicy`) instead of being
    // duplicated inline. The static-asset server has no same-origin notion, so
    // `host` is `None`: only loopback / `tauri:` origins pass.
    let policy = alephcore::gateway::origin_policy::OriginPolicy::loopback_only();
    let allowed_origins = tower_http::cors::AllowOrigin::predicate(move |origin, _| {
        policy.is_allowed(origin.to_str().ok(), None)
    });
    let app = Router::new().fallback_service(serve_dir).layer(
        tower_http::cors::CorsLayer::new()
            .allow_origin(allowed_origins)
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::HEAD,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers([
                axum::http::header::ACCEPT,
                axum::http::header::CONTENT_TYPE,
                axum::http::header::AUTHORIZATION,
            ]),
    );

    // Create listener
    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Serve
    axum::serve(listener, app).await?;

    Ok(())
}

/// Handle agent.run with real `ExecutionEngine`
pub async fn handle_run_with_engine<P, R>(
    request: alephcore::gateway::JsonRpcRequest,
    engine: Arc<ExecutionEngine<P, R>>,
    event_bus: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agent_registry: Arc<AgentRegistry>,
    app_config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    _workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>>,
) -> alephcore::gateway::JsonRpcResponse
where
    P: alephcore::thinker::ProviderRegistry + 'static,
    R: alephcore::executor::ToolRegistry + 'static,
{
    use alephcore::gateway::handlers::agent::{build_run_request, AgentRunParams};
    use alephcore::gateway::protocol::{INTERNAL_ERROR, INVALID_PARAMS};
    use serde::Serialize;
    use serde_json::{json, Value};

    /// Result of agent.run request
    #[derive(Debug, Clone, Serialize)]
    struct AgentRunResult {
        pub run_id: String,
        pub session_key: String,
        pub accepted_at: String,
    }

    // Parse params
    let params: AgentRunParams = match request.params {
        Some(Value::Object(map)) => match serde_json::from_value(Value::Object(map)) {
            Ok(p) => p,
            Err(e) => {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {e}"),
                );
            }
        },
        _ => {
            return alephcore::gateway::JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing or invalid params object",
            );
        }
    };

    // Validate input
    if params.input.trim().is_empty() {
        return alephcore::gateway::JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Input cannot be empty",
        );
    }

    // Generate run ID
    let run_id = uuid::Uuid::new_v4().to_string();

    // Resolve session key
    let session_key = router
        .route(
            params.session_key.as_deref(),
            params.channel.as_deref(),
            params.peer_id.as_deref(),
            None, // agent_id: legacy path, no explicit agent
        )
        .await;

    let session_key_str = session_key.to_key_string();
    let accepted_at = chrono::Utc::now().to_rfc3339();

    // Resolve agent from session_key (which encodes the correct agent_id)
    let resolved_agent_id = session_key.agent_id().to_string();

    let agent = {
        let agent_opt = agent_registry.get(&resolved_agent_id).await;
        match agent_opt.or(agent_registry.get_default().await) {
            Some(a) => a,
            None => {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    "No default agent available",
                );
            }
        }
    };

    // Create emitter for streaming events, respecting output_mode config
    let (output_mode, locale, busy_cfg) = {
        let cfg = app_config.read().await;
        let behavior = cfg.behavior.as_ref();
        let mode_str = behavior.map_or("typewriter", |b| b.output_mode.as_str());
        (
            alephcore::gateway::OutputMode::from_config(mode_str),
            alephcore::gateway::i18n::Locale::from_config(cfg.general.language.as_deref()),
            alephcore::gateway::busy_queue::BusyQueueConfig::from_execution(&cfg.execution),
        )
    };
    let emitter = Arc::new(GatewayEventEmitter::with_output_mode(
        event_bus.clone(),
        output_mode,
    ));

    // The RunRequest (metadata, project_root gate, attachments, model
    // override, voice pin) is built by the one shared builder both this real
    // engine path and the Simulated-fallback `AgentRunManager::start_run`
    // call — see `gateway::handlers::agent::build_run_request`.
    let run_request =
        match build_run_request(run_id.clone(), &session_key, params, Some(&app_config)).await {
            Ok(r) => r,
            Err(e) => {
                return alephcore::gateway::JsonRpcResponse::error(request.id, INVALID_PARAMS, e);
            }
        };

    // Spawn onto the shared per-session busy wait lane (same queue the inbound
    // router uses for channel messages).
    spawn_queued_run(
        engine.clone(),
        run_request,
        agent,
        emitter,
        busy_cfg,
        locale,
        "Agent run",
    );

    // Return immediate response
    let result = AgentRunResult {
        run_id,
        session_key: session_key_str,
        accepted_at,
    };

    alephcore::gateway::JsonRpcResponse::success(request.id, json!(result))
}

/// Handle chat.send with real `ExecutionEngine`
///
/// Same as `handle_run_with_engine` but accepts `chat.send` param format
/// (message instead of input) and returns chat-friendly response.
#[allow(clippy::too_many_arguments)]
pub async fn handle_chat_send_with_engine<P, R>(
    request: alephcore::gateway::JsonRpcRequest,
    engine: Arc<ExecutionEngine<P, R>>,
    event_bus: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agent_registry: Arc<AgentRegistry>,
    app_config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    _workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>>,
    _provider_registry: Arc<P>,
    _session_manager: Arc<dyn alephcore::gateway::session_store::SessionStore>,
    command_parser: Option<Arc<alephcore::command::CommandParser>>,
) -> alephcore::gateway::JsonRpcResponse
where
    P: alephcore::thinker::ProviderRegistry + 'static,
    R: alephcore::executor::ToolRegistry + 'static,
{
    use alephcore::gateway::handlers::agent::{build_run_request, AgentRunParams};
    use alephcore::gateway::handlers::chat::SendParams;
    use alephcore::gateway::protocol::{INTERNAL_ERROR, INVALID_PARAMS};
    use serde::Serialize;
    use serde_json::{json, Value};

    #[derive(Debug, Clone, Serialize)]
    struct ChatSendResult {
        pub run_id: String,
        pub session_key: String,
        pub streaming: bool,
    }

    // Parse params
    let params: SendParams = match request.params {
        Some(Value::Object(map)) => match serde_json::from_value(Value::Object(map)) {
            Ok(p) => p,
            Err(e) => {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {e}"),
                );
            }
        },
        _ => {
            return alephcore::gateway::JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing or invalid params object",
            );
        }
    };

    // Validate message
    if params.message.trim().is_empty() {
        return alephcore::gateway::JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Message cannot be empty",
        );
    }

    // Generate run ID
    let run_id = uuid::Uuid::new_v4().to_string();

    // Resolve session key (with explicit agent_id from Panel if provided)
    let session_key = router
        .route(
            params.session_key.as_deref(),
            params.channel.as_deref(),
            None,
            params.agent_id.as_deref(),
        )
        .await;

    let session_key_str = session_key.to_key_string();

    // Resolve agent from session_key (which now encodes the correct agent_id)
    let resolved_agent_id = session_key.agent_id().to_string();

    let agent = {
        let agent_opt = agent_registry.get(&resolved_agent_id).await;
        match agent_opt.or(agent_registry.get_default().await) {
            Some(a) => a,
            None => {
                return alephcore::gateway::JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    "No default agent available",
                );
            }
        }
    };

    // Create emitter for streaming events, respecting output_mode config
    let (output_mode, locale, busy_cfg) = {
        let cfg = app_config.read().await;
        let behavior = cfg.behavior.as_ref();
        let mode_str = behavior.map_or("typewriter", |b| b.output_mode.as_str());
        (
            alephcore::gateway::OutputMode::from_config(mode_str),
            alephcore::gateway::i18n::Locale::from_config(cfg.general.language.as_deref()),
            alephcore::gateway::busy_queue::BusyQueueConfig::from_execution(&cfg.execution),
        )
    };
    let emitter = Arc::new(GatewayEventEmitter::with_output_mode(
        event_bus.clone(),
        output_mode,
    ));

    // `chat.send` is `agent.run` with a `message` field: project it onto the
    // canonical params and let the one shared builder produce the RunRequest,
    // so attachments, the model picker, the voice pin, the project_root gate
    // and the surface/authorization metadata are honored identically on this
    // real-`ExecutionEngine` path and on the Simulated-fallback
    // `AgentRunManager::start_run`. Every one of those was previously dropped
    // here (`attachments: Vec::new()`, `model_override: None`, no
    // `platform` / `caller_role` / `conversation_id`).
    let run_params = AgentRunParams {
        input: params.message.clone(),
        session_key: params.session_key,
        channel: params.channel,
        peer_id: None, // Panel has no per-user peer IDs
        stream: params.stream,
        thinking: params.thinking,
        attachments: params.attachments,
        agent_id: params.agent_id,
        project_root: params.project_root,
        model_override: params.model_override,
        exec_tier: params.exec_tier,
        mode: params.mode,
        voice_input: params.voice_input,
    };
    let mut run_request = match build_run_request(
        run_id.clone(),
        &session_key,
        run_params,
        Some(&app_config),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return alephcore::gateway::JsonRpcResponse::error(request.id, INVALID_PARAMS, e);
        }
    };

    // Slash command detection: resolve via CommandParser and emit the
    // source-aware mode JSON (preserves Skill instructions / Custom system
    // prompt / MCP server name so the fast path can act on them). `chat.send`
    // only — the inbound router does this for channel turns.
    if params.message.trim().starts_with('/') {
        if let Some(ref parser) = command_parser {
            let slash_text = params.message.trim();
            if let Some(parsed) = parser.parse_async(slash_text).await {
                if let Some(mode_json) =
                    alephcore::gateway::inbound_router::serialize_parsed_command(&parsed)
                {
                    tracing::info!(
                        "[chat.send] Slash command resolved: name={}, args={:?}",
                        parsed.command_name,
                        parsed.arguments
                    );
                    run_request.metadata.insert(
                        alephcore::gateway::inbound_router::SLASH_COMMAND_MODE_KEY.to_string(),
                        mode_json,
                    );
                }
            }
        }
    }

    // Spawn onto the shared per-session busy wait lane (same queue the inbound
    // router uses for channel messages).
    spawn_queued_run(
        engine.clone(),
        run_request,
        agent,
        emitter,
        busy_cfg,
        locale,
        "Chat run",
    );

    // Auto-topic generation is now handled by ExecutionEngine on first message

    // Return immediate response
    let result = ChatSendResult {
        run_id,
        session_key: session_key_str,
        streaming: params.stream,
    };

    alephcore::gateway::JsonRpcResponse::success(request.id, json!(result))
}
