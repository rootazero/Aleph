use super::*;

pub(in crate::commands::start) fn register_daemon_handlers(
    server: &mut GatewayServer,
    start_time: std::time::Instant,
    daemon: bool,
) {
    use alephcore::gateway::handlers::daemon_control;

    // daemon.logs is already registered as stateless in HandlerRegistry::new()
    // Wire daemon.status with the actual start_time
    let start = start_time;
    server.handlers_mut().register("daemon.status", move |req| {
        let st = start;
        async move { daemon_control::handle_status(req, st).await }
    });

    // Wire daemon.shutdown (stateless — sends SIGTERM to self)
    register_handler!(server, "daemon.shutdown", daemon_control::handle_shutdown);

    if !daemon {
        println!("Daemon methods:");
        println!("  - daemon.status   : Server runtime status");
        println!("  - daemon.shutdown : Graceful shutdown");
        println!("  - daemon.logs     : View recent logs");
        println!();
    }
}

// ─── register_oauth_handlers ─────────────────────────────────────────────────

pub(in crate::commands::start) fn register_oauth_handlers(
    server: &mut GatewayServer,
    oauth_state: &oauth_handlers::SharedOAuthState,
    config: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    vault: &Arc<alephcore::gateway::security::SharedTokenManager>,
    daemon: bool,
) {
    register_handler!(
        server,
        "providers.oauthLogin",
        oauth_handlers::handle_oauth_login,
        oauth_state,
        config,
        vault
    );
    register_handler!(
        server,
        "providers.oauthLogout",
        oauth_handlers::handle_oauth_logout,
        oauth_state,
        config,
        vault
    );
    register_handler!(
        server,
        "providers.oauthStatus",
        oauth_handlers::handle_oauth_status,
        oauth_state,
        config,
        vault
    );

    if !daemon {
        println!("OAuth methods:");
        println!("  - providers.oauthLogin  : Start browser OAuth login");
        println!("  - providers.oauthLogout : Clear OAuth token");
        println!("  - providers.oauthStatus : Check OAuth status");
        println!();
    }
}

// ─── register_identity_handlers ──────────────────────────────────────────────

pub(in crate::commands::start) fn register_identity_handlers(
    server: &mut GatewayServer,
    resolver: &SharedIdentityResolver,
) {
    register_handler!(
        server,
        "identity.get",
        identity_handlers::handle_get,
        resolver
    );
    register_handler!(
        server,
        "identity.set",
        identity_handlers::handle_set,
        resolver
    );
    register_handler!(
        server,
        "identity.clear",
        identity_handlers::handle_clear,
        resolver
    );
    register_handler!(
        server,
        "identity.list",
        identity_handlers::handle_list,
        resolver
    );
}

// ─── register_group_chat_handlers ───────────────────────────────────────────

pub(in crate::commands::start) fn register_group_chat_handlers(
    server: &mut GatewayServer,
    orch: &SharedOrchestrator,
    executor: &Arc<GroupChatExecutor>,
    daemon: bool,
) {
    register_handler!(
        server,
        "group_chat.start",
        group_chat_handlers::handle_start,
        orch,
        executor
    );
    register_handler!(
        server,
        "group_chat.continue",
        group_chat_handlers::handle_continue,
        orch,
        executor
    );
    register_handler!(
        server,
        "group_chat.mention",
        group_chat_handlers::handle_mention,
        orch,
        executor
    );
    register_handler!(
        server,
        "group_chat.end",
        group_chat_handlers::handle_end,
        orch
    );
    register_handler!(
        server,
        "group_chat.list",
        group_chat_handlers::handle_list,
        orch
    );
    register_handler!(
        server,
        "group_chat.history",
        group_chat_handlers::handle_history,
        orch
    );

    if !daemon {
        println!("Group Chat methods:");
        println!("  - group_chat.start    : Start a new group chat session");
        println!("  - group_chat.continue : Continue with new message");
        println!("  - group_chat.mention  : Mention specific personas");
        println!("  - group_chat.end      : End a session");
        println!("  - group_chat.list     : List active sessions");
        println!("  - group_chat.history  : Get conversation history");
        println!();
    }
}

// ─── register_agents_handlers ───────────────────────────────────────────────
