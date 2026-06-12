use super::{GatewayServer, Arc, auth_handlers};

// ─── register_core_handlers ──────────────────────────────────────────────────

pub(in crate::commands::start) fn register_core_handlers(
    server: &mut GatewayServer,
    auth_ctx: &Arc<auth_handlers::AuthContext>,
) {
    // LAN-trust connect: minimal no-auth handshake.
    let connect_ctx = Arc::new(alephcore::gateway::handlers::connect::ConnectContext {
        state_versions: Arc::clone(&auth_ctx.state_versions),
        transport_policy: auth_ctx.transport_policy.clone(),
    });
    register_handler!(
        server,
        "connect",
        alephcore::gateway::handlers::connect::handle_connect,
        connect_ctx
    );

    // Cluster node enrollment + environment listing.
    use alephcore::gateway::handlers::cluster as cluster_handlers;
    register_handler!(
        server,
        "cluster.enroll",
        cluster_handlers::handle_cluster_enroll,
        auth_ctx
    );
    register_handler!(
        server,
        "cluster.deregister",
        cluster_handlers::handle_cluster_deregister,
        auth_ctx
    );
    register_handler!(
        server,
        "environments.list",
        cluster_handlers::handle_environments_list,
        auth_ctx
    );

    // Vault secret CRUD over JSON-RPC.
    use alephcore::gateway::handlers::secrets as secrets_handlers;
    register_handler!(
        server,
        "secrets.list",
        secrets_handlers::handle_secrets_list,
        auth_ctx
    );
    register_handler!(
        server,
        "secrets.set",
        secrets_handlers::handle_secrets_set,
        auth_ctx
    );
    register_handler!(
        server,
        "secrets.delete",
        secrets_handlers::handle_secrets_delete,
        auth_ctx
    );
    register_handler!(
        server,
        "secrets.verify",
        secrets_handlers::handle_secrets_verify,
        auth_ctx
    );
    register_handler!(
        server,
        "secrets.providers",
        secrets_handlers::handle_secrets_providers,
        auth_ctx
    );
}

// ─── register_guest_handlers ─────────────────────────────────────────────────

pub(in crate::commands::start) fn register_guest_handlers(
    server: &mut GatewayServer,
    invitation_manager: &Arc<alephcore::gateway::security::InvitationManager>,
    session_manager: &Arc<alephcore::gateway::security::GuestSessionManager>,
    event_bus: &Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::guests;

    register_handler!(
        server,
        "guests.createInvitation",
        guests::handle_create_invitation,
        invitation_manager,
        event_bus
    );
    register_handler!(
        server,
        "guests.listPending",
        guests::handle_list_guests,
        invitation_manager
    );
    register_handler!(
        server,
        "guests.revokeInvitation",
        guests::handle_revoke_invitation,
        invitation_manager,
        event_bus
    );
    register_handler!(
        server,
        "guests.listSessions",
        guests::handle_list_sessions,
        session_manager
    );
    register_handler!(
        server,
        "guests.terminateSession",
        guests::handle_terminate_session,
        session_manager,
        event_bus
    );
    register_handler!(
        server,
        "guests.getActivityLogs",
        guests::handle_get_activity_logs,
        session_manager
    );
}
