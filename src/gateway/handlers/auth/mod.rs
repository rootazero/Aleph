//! Authentication types — shared context and data structures.
//!
//! Handler implementations for auth/pairing/devices have been removed as part
//! of the LAN-trust architecture revert (Task 3). This module holds only the
//! types still referenced by surviving handlers (cluster, secrets, connect)
//! and T4-doomed files (auth_middleware). Will be further reduced in T5.

use crate::sync_primitives::Arc;
use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::gateway::config::AuthMode;
use crate::gateway::device_store::DeviceStore;
use crate::gateway::presence::PresenceTracker;
use crate::gateway::security::SecurityStore;
use crate::gateway::security::{PairingManager, SharedTokenManager, TokenManager};
use crate::gateway::server::ConnectionState;

/// Permission/tier helpers (previously in tier.rs submodule, inlined here).
pub mod tier {
    const WILDCARD: &str = "*";

    /// UI-facing tier label for a device holding `permissions`.
    pub fn tier_for_permissions(permissions: &[String]) -> &'static str {
        if permissions.iter().any(|p| p == WILDCARD) {
            "config"
        } else {
            "chat"
        }
    }
}

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

/// Authentication context for handlers.
///
/// Shared by cluster.*/secrets.* handlers and T4-doomed auth_middleware.
/// Will be deleted in T5 when security/device stores are removed.
pub struct AuthContext {
    pub token_manager: Arc<TokenManager>,
    pub pairing_manager: Arc<PairingManager>,
    pub device_store: Arc<DeviceStore>,
    pub security_store: Arc<SecurityStore>,
    pub invitation_manager: Arc<crate::gateway::security::InvitationManager>,
    pub guest_session_manager: Arc<crate::gateway::security::GuestSessionManager>,
    pub event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
    pub auth_mode: AuthMode,
    pub allow_guest: bool,
    pub enable_pairing: bool,
    pub shared_token_mgr: Arc<SharedTokenManager>,
    pub state_versions: Arc<crate::gateway::state_version::StateVersionTracker>,
    pub transport_policy: TransportPolicy,
    pub instance_id: String,
    pub started_at_unix: i64,
    pub presence: Arc<PresenceTracker>,
    pub max_connections: u32,
    pub challenge_manager: Arc<crate::gateway::challenge::ChallengeManager>,
    pub require_challenge: bool,
    pub bootstrap_mgr: Arc<crate::gateway::bootstrap::BootstrapNonceManager>,
    pub session_mgr: Arc<crate::gateway::session::HttpSessionManager>,
    pub bind_port: u16,
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
}

impl AuthContext {
    #[must_use]
    pub fn public_bind_for_loopback(&self) -> String {
        format!("127.0.0.1:{}", self.bind_port)
    }
}


#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::gateway::security::store::DeviceUpsertData;

    pub(crate) fn create_test_context() -> Arc<AuthContext> {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        store
            .upsert_device(&DeviceUpsertData {
                device_id: "test-dev",
                device_name: "Test",
                device_type: None,
                public_key: &[1u8; 32],
                fingerprint: "fp",
                role: "operator",
                scopes: &[],
            })
            .unwrap();

        let invitation_manager = Arc::new(crate::gateway::security::InvitationManager::new());
        let guest_session_manager = Arc::new(crate::gateway::security::GuestSessionManager::new());
        let event_bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
        let shared_token_mgr = Arc::new(SharedTokenManager::new(
            store.clone(),
            "/tmp/aleph_test.vault",
        ));
        let _ = shared_token_mgr.generate_token();
        let session_mgr = Arc::new(crate::gateway::session::HttpSessionManager::new(
            store.clone(),
            24,
        ));

        Arc::new(AuthContext {
            token_manager: Arc::new(TokenManager::new(store.clone())),
            pairing_manager: Arc::new(PairingManager::new(store.clone())),
            device_store: Arc::new(DeviceStore::in_memory().unwrap()),
            security_store: store,
            invitation_manager,
            guest_session_manager,
            event_bus,
            auth_mode: AuthMode::Token,
            allow_guest: true,
            enable_pairing: true,
            shared_token_mgr,
            state_versions: Arc::new(crate::gateway::state_version::StateVersionTracker::new()),
            transport_policy: TransportPolicy::defaults(),
            instance_id: "test-instance".to_string(),
            started_at_unix: 1_700_000_000,
            presence: Arc::new(PresenceTracker::new()),
            max_connections: 1000,
            challenge_manager: Arc::new(crate::gateway::challenge::ChallengeManager::new()),
            require_challenge: false,
            bootstrap_mgr: Arc::new(crate::gateway::bootstrap::BootstrapNonceManager::default()),
            session_mgr,
            bind_port: 18790,
            connections: Arc::new(RwLock::new(HashMap::new())),
            node_registry: Arc::new(crate::cluster::NodeRegistry::new()),
        })
    }
}
