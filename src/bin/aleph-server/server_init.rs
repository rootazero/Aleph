//! Server initialization helpers for Aleph Gateway
//!
//! This module contains helper functions for server initialization,
//! including `WebChat` serving and agent run handling.

use std::net::SocketAddr;
use std::path::PathBuf;

use alephcore::sync_primitives::Arc;

use alephcore::gateway::busy_queue::spawn_queued_run;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::router::AgentRouter;
use alephcore::gateway::{AgentRegistry, ExecutionEngine, GatewayEventEmitter};

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
///
/// `agent.run` is `chat.send` with a different param spelling — same router,
/// same engine, same wait lane — so it carries the SAME P1 visibility
/// obligation as its twin below (`handle_chat_send_with_engine`), and for the
/// same reason: it accepts a caller-supplied `session_key`, routes it through
/// `AgentRouter`, and starts a real run against it. Without the check a
/// member could write a turn into any other user's session and read the whole
/// transcript back out of the model, bypassing every `KeyChecked` guard the
/// `sessions.*`/`chat.*` family installs. Hence the `session_manager`
/// parameter, which this function needs for nothing else.
#[allow(clippy::too_many_arguments)]
pub async fn handle_run_with_engine<P, R>(
    request: alephcore::gateway::JsonRpcRequest,
    engine: Arc<ExecutionEngine<P, R>>,
    event_bus: Arc<GatewayEventBus>,
    router: Arc<AgentRouter>,
    agent_registry: Arc<AgentRegistry>,
    app_config: Arc<tokio::sync::RwLock<alephcore::Config>>,
    _workspace_manager: Option<Arc<alephcore::gateway::AgentEnvStore>>,
    session_manager: Arc<dyn alephcore::gateway::session_store::SessionStore>,
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

    // P1 visibility chokepoint — identical placement and semantics to the
    // `chat.send` twin below (immediately after `router.route`, before any
    // agent resolution or run start): a caller-supplied `session_key` that
    // already belongs to someone else is refused; a brand-new key is NOT a
    // denial (the run that follows creates and stamps it). See
    // `visibility::existing_session_is_visible`.
    if !alephcore::gateway::visibility::existing_session_is_visible(
        session_manager.as_ref(),
        &session_key,
    )
    .await
    {
        return alephcore::gateway::visibility::not_found_response(request.id);
    }

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
    session_manager: Arc<dyn alephcore::gateway::session_store::SessionStore>,
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

    // P1 visibility chokepoint: a caller-supplied session_key that already
    // belongs to someone else must be refused before a run ever starts — a
    // brand-new key (nothing created yet) is NOT a denial, it is the
    // ordinary "first message of a new conversation" case and proceeds so
    // the run can create+stamp it. See `visibility::existing_session_is_visible`.
    if !alephcore::gateway::visibility::existing_session_is_visible(
        session_manager.as_ref(),
        &session_key,
    )
    .await
    {
        return alephcore::gateway::visibility::not_found_response(request.id);
    }

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

#[cfg(test)]
mod tests {
    /// Both run-start entry points in this file take a caller-supplied
    /// `session_key`, route it through `AgentRouter`, and start a real run
    /// against the result — so both owe the P1 visibility check, and
    /// `agent.run` shipped without it (final-review finding C1).
    ///
    /// This is a SOURCE pin, not an effect test, and deliberately so: the
    /// effect is only observable by calling these functions, which requires a
    /// live `ExecutionEngine<P, R>` (a provider registry plus a tool
    /// registry), and they live in the binary crate — `alephcore`'s test
    /// surface cannot reach them, and constructing the engine here would test
    /// the engine, not the guard. The repo already uses this technique where
    /// the real wire is invisible to host tests (`queue_row_key`'s
    /// `include_str!` pin on the Panel's `<For>` call — see the root
    /// CLAUDE.md's busy-queue note). What it buys is precise: deleting either
    /// call fails a test by name instead of silently reopening the hole.
    ///
    /// The predicate's own deny/allow/brand-new-key behaviour is covered by
    /// effect tests against a real `SessionStore` in
    /// `alephcore::gateway::visibility`.
    #[test]
    fn both_run_start_paths_check_session_visibility() {
        // Normalize line endings: this file is CRLF on disk on Windows, and
        // `include_str!` hands over the raw bytes. Matching on a bare "\n}"
        // bound against CRLF text silently finds nothing and runs the bound
        // to end-of-file — which is precisely the vacuous pass the
        // "mod tests" assertion below exists to catch, and did catch.
        let src = include_str!("server_init.rs").replace('\r', "");
        let guard = "visibility::existing_session_is_visible";

        for entry in ["handle_run_with_engine", "handle_chat_send_with_engine"] {
            let body = src
                .split_once(&format!("pub async fn {entry}<P, R>"))
                .unwrap_or_else(|| panic!("{entry} must exist in this file"))
                .1;
            // Bound the search to this function: stop at the next `\n}` that
            // closes it, so a sibling's guard can never satisfy this entry.
            let body = body.split("\n}\n").next().unwrap_or(body);
            // The bound is what makes the assertion below non-vacuous, so
            // check it held: this module's own `guard` literal sits after the
            // last function, and an over-long body would swallow it and pass
            // for free.
            assert!(
                !body.contains("mod tests"),
                "{entry}'s body bound leaked past the end of the file — the \
                 assertion below would pass on this module's own source"
            );
            // Assert on the CALL, not the name: the guard string also appears
            // in each function's explanatory comment, so `contains(guard)`
            // alone stays green if someone deletes the `if` block and leaves
            // the comment. The prose mentions end in a backtick, never `(`.
            assert!(
                body.contains(&format!("{guard}(")),
                "{entry} starts a run against a caller-supplied session_key and \
                 must call {guard} before doing so (final-review C1)"
            );
        }
    }
}
