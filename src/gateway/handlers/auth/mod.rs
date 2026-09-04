//! Shared handler context types (formerly the auth module).
//!
//! The auth/pairing/devices handler family was removed in the LAN-trust
//! architecture revert (T3-T5). This module keeps only:
//!
//! * [`TransportPolicy`] — keepalive policy advertised at the `connect`
//!   handshake (consumed by `handlers::connect`).
//! * [`AuthContext`] — the shared dependency bundle for the surviving
//!   `secrets.*` handlers (vault) and `cluster.*` handlers
//!   (node enrollment records + live registry).
//!
//! The name `AuthContext` is historical; renaming it is deferred to keep
//! T5 churn minimal (callers: `handlers/secrets.rs`, `handlers/cluster.rs`,
//! `aleph-server` boot wiring).

use crate::sync_primitives::Arc;
use serde::Serialize;

use crate::gateway::security::{SecurityStore, SharedTokenManager};

/// Transport-layer policy advertised to the client at handshake time.
#[derive(Debug, Clone, Serialize)]
pub struct TransportPolicy {
    pub ping_interval_secs: u64,
    pub idle_timeout_secs: u64,
}

impl TransportPolicy {
    /// Static defaults that mirror [`super::super::server::GatewayConfig`].
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            ping_interval_secs: 30,
            idle_timeout_secs: 90,
        }
    }
}

/// Shared context for the `secrets.*` and `cluster.*` handlers.
///
/// Every field is read by a live handler:
/// * `shared_token_mgr` — `secrets.rs` (vault CRUD).
/// * `security_store` — `cluster.rs` (enrolled-node device records for the
///   `environments.list` offline merge + deregister).
/// * `node_registry` — `cluster.rs` (online sessions / eviction).
pub struct AuthContext {
    pub shared_token_mgr: Arc<SharedTokenManager>,
    pub security_store: Arc<SecurityStore>,
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
    /// Bus for emitting lifecycle events from the cluster RPC handlers
    /// (currently `node.disconnected` from `cluster.deregister`). Absent in
    /// probe-mode / partially-booted processes; the handler still takes
    /// effect, it just does not publish.
    pub event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Returns the scratch guard alongside the context. The vault is a real
    /// file on disk, and it used to be a FIXED path in the world-writable
    /// `/tmp`: shared by every concurrent test process and owned by nobody.
    pub(crate) fn create_test_context() -> (tempfile::TempDir, Arc<AuthContext>) {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let (scratch, dir) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let shared_token_mgr = Arc::new(SharedTokenManager::new(
            store.clone(),
            dir.join("test.vault"),
        ));
        let _ = shared_token_mgr.generate_token();

        (
            scratch,
            Arc::new(AuthContext {
                shared_token_mgr,
                security_store: store,
                node_registry: Arc::new(crate::cluster::NodeRegistry::new()),
                event_bus: None,
            }),
        )
    }
}
