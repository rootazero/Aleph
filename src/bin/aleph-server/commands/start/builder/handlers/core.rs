use super::{auth_handlers, Arc, GatewayServer};

// ─── register_core_handlers ──────────────────────────────────────────────────

pub(in crate::commands::start) fn register_core_handlers(
    server: &mut GatewayServer,
    auth_ctx: &Arc<auth_handlers::AuthContext>,
    transport_policy: auth_handlers::TransportPolicy,
) {
    // The `connect` handshake. It is not credential-free: `resolve_connect_auth`
    // runs inside it and is what admits or walls a remote connection.
    let connect_ctx = Arc::new(alephcore::gateway::handlers::connect::ConnectContext {
        state_versions: server.state_versions.clone(),
        transport_policy,
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
