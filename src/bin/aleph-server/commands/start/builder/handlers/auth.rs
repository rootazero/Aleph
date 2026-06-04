use super::*;

// ─── register_auth_handlers ──────────────────────────────────────────────────

pub(in crate::commands::start) fn register_auth_handlers(
    server: &mut GatewayServer,
    auth_ctx: &Arc<auth_handlers::AuthContext>,
) {
    register_handler!(server, "connect", auth_handlers::handle_connect, auth_ctx);
    register_handler!(
        server,
        "connect.challenge",
        auth_handlers::handle_connect_challenge,
        auth_ctx
    );
    register_handler!(
        server,
        "pairing.approve",
        auth_handlers::handle_pairing_approve,
        auth_ctx
    );
    register_handler!(
        server,
        "pairing.reject",
        auth_handlers::handle_pairing_reject,
        auth_ctx
    );
    register_handler!(
        server,
        "pairing.list",
        auth_handlers::handle_pairing_list,
        auth_ctx
    );
    // Anonymous cold-browser pairing surface — reachable without a token
    // because the calling browser doesn't have one yet (see
    // gateway::server::handler::allow_unauth_browser_pairing for the
    // auth bypass; note it is intentionally NOT loopback-gated — a remote
    // LAN browser is an expected caller).
    register_handler!(
        server,
        "pairing.start_browser",
        auth_handlers::handle_pairing_start_browser,
        auth_ctx
    );
    register_handler!(
        server,
        "pairing.poll",
        auth_handlers::handle_pairing_poll,
        auth_ctx
    );
    register_handler!(
        server,
        "devices.list",
        auth_handlers::handle_devices_list,
        auth_ctx
    );
    register_handler!(
        server,
        "devices.revoke",
        auth_handlers::handle_devices_revoke,
        auth_ctx
    );

    // Auth management tools (R9: Everything is a Tool)
    register_handler!(
        server,
        "auth.show_token",
        auth_tools_handlers::handle_auth_show_token,
        auth_ctx
    );
    register_handler!(
        server,
        "auth.reset_token",
        auth_tools_handlers::handle_auth_reset_token,
        auth_ctx
    );
    register_handler!(
        server,
        "auth.list_sessions",
        auth_tools_handlers::handle_auth_list_sessions,
        auth_ctx
    );
    register_handler!(
        server,
        "auth.revoke_session",
        auth_tools_handlers::handle_auth_revoke_session,
        auth_ctx
    );

    // Bootstrap nonce issuer — pairs with the loopback HTTP route
    // `/auth/bootstrap?nonce=…` to hand the local browser an
    // authenticated session cookie without showing the user a token.
    register_handler!(
        server,
        "gateway.bootstrap.issue",
        auth_handlers::handle_gateway_bootstrap_issue,
        auth_ctx
    );

    // Vault secret CRUD over JSON-RPC. Mirrors `/v1/admin/secrets`
    // so thin clients don't need an HTTP fallback path.
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
