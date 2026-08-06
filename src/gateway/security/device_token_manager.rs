//! Device token management for remote Panel authentication.
//!
//! This module implements a one-time bootstrap ticket + per-device token flow:
//!
//! 1. An already-authorized caller requests `gateway.ticket.create`.
//! 2. Server returns a short-lived bootstrap ticket (`aleph-bt-<uuid>`).
//! 3. New device opens `?bt=<ticket>` and submits it in the `connect` handshake.
//! 4. Server validates the ticket, creates a device record, and issues a
//!    long-lived device token (`aleph-dt-<uuid>`).
//! 5. Device stores the device token and uses it for subsequent reconnects.
//!
//! The long-lived shared Gateway token is no longer placed in URLs / QR codes.

use sha2::Digest;
use uuid::Uuid;

use crate::gateway::security::store::{
    BootstrapTicketError, ConsumedBootstrapTicket, DeviceRow, DeviceTokenRow, DeviceUpsertData,
    SecurityStore, OWNER_USER_ID,
};
use crate::sync_primitives::Arc;

/// Default bootstrap ticket lifetime: 5 minutes.
const DEFAULT_BOOTSTRAP_TTL_MS: i64 = 5 * 60 * 1000;

/// Default device token lifetime: 10 years (effectively indefinite for MVP).
const DEFAULT_DEVICE_TOKEN_TTL_MS: i64 = 10 * 365 * 24 * 60 * 60 * 1000;

/// `devices.device_type` value for a paired remote Panel.
///
/// Single source: the roster filter, the pairing upsert, and the id-namespace
/// guard must all agree, or a device becomes listable-but-unpairable (or worse,
/// pairable-but-unlistable — see [`DeviceTokenManager::exchange_bootstrap_ticket`]).
/// Cluster nodes are `role = "node"` with a NULL `device_type`.
pub const PANEL_DEVICE_TYPE: &str = "panel";

/// Device-token manager errors.
#[derive(Debug, thiserror::Error)]
pub enum DeviceTokenError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Invalid or expired bootstrap ticket")]
    InvalidBootstrapTicket,
    /// The client-asserted `device_id` already names a device that is **not** a
    /// paired Panel (a cluster node, or any row minted outside this flow).
    /// Pairing onto it would issue an operator token the Panel device roster
    /// cannot see or revoke — see [`DeviceTokenManager::exchange_bootstrap_ticket`].
    #[error("Device id {0} already belongs to a non-Panel device")]
    DeviceIdConflict(String),
}

impl From<BootstrapTicketError> for DeviceTokenError {
    fn from(_: BootstrapTicketError) -> Self {
        Self::InvalidBootstrapTicket
    }
}

impl From<rusqlite::Error> for DeviceTokenError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.to_string())
    }
}

/// Result of exchanging a bootstrap ticket for a device token.
#[derive(Debug, Clone)]
pub struct BootstrapExchangeResult {
    pub device_id: String,
    pub device_token: String,
}

/// Manages bootstrap tickets and per-device tokens.
#[derive(Clone)]
pub struct DeviceTokenManager {
    store: Arc<SecurityStore>,
}

impl DeviceTokenManager {
    /// Create a new manager backed by the given security store.
    #[must_use]
    pub const fn new(store: Arc<SecurityStore>) -> Self {
        Self { store }
    }

    /// Generate a new bootstrap ticket.
    ///
    /// `ttl_ms` defaults to 5 minutes when `None`. `user_id` binds the ticket
    /// to a user — the device that exchanges it inherits the binding (see
    /// [`Self::exchange_bootstrap_ticket`]); `None` leaves the ticket unbound,
    /// which defaults a brand-new device to the owner and preserves an
    /// existing binding on re-pair.
    pub fn create_bootstrap_ticket(
        &self,
        ttl_ms: Option<i64>,
        user_id: Option<&str>,
    ) -> Result<String, DeviceTokenError> {
        let code = format!("aleph-bt-{}", Uuid::new_v4());
        let ttl = ttl_ms.unwrap_or(DEFAULT_BOOTSTRAP_TTL_MS);
        // Negative TTL is allowed for tests that need an immediately-expired ticket.
        let ttl = if ttl < 0 { ttl } else { ttl.max(60_000) };
        self.store.create_bootstrap_ticket(&code, ttl, user_id)?;
        Ok(code)
    }

    /// Exchange a bootstrap ticket for a per-device token.
    ///
    /// If `device_id` is not provided, a random UUID is assigned. The device
    /// record is created with operator role and wildcard scope for the MVP.
    ///
    /// `device_id` is **client-asserted** (the Panel keeps it in localStorage),
    /// and the `devices` table is one flat namespace shared with cluster nodes.
    /// `upsert_device`'s ON CONFLICT deliberately does not rewrite `device_type`
    /// — so pairing onto an existing non-Panel row would leave the row typed
    /// `node`/NULL while [`Self::issue_device_token`] minted a working operator
    /// token, producing a credential that [`Self::list_panel_devices`] hides and
    /// that `revoke_all_panel_devices` (hence `gateway.token.rotate`) cannot
    /// reach. That is the same shape as the 2026-07-17 "revoked device
    /// resurrects" bug, one namespace over, so this fails closed **before** the
    /// ticket is consumed (a collision must not burn the operator's one-time
    /// ticket, and a ticket holder learns nothing it could not learn by pairing
    /// successfully and calling `gateway.devices.list`).
    pub fn exchange_bootstrap_ticket(
        &self,
        ticket: &str,
        device_id: Option<String>,
        device_name: Option<String>,
        _public_key: Option<Vec<u8>>,
    ) -> Result<BootstrapExchangeResult, DeviceTokenError> {
        let device_id = device_id.unwrap_or_else(|| format!("device-{}", Uuid::new_v4()));
        let device_name = device_name.unwrap_or_else(|| "Remote Panel".to_string());

        if let Some(existing) = self.store.get_device(&device_id)? {
            if existing.device_type.as_deref() != Some(PANEL_DEVICE_TYPE) {
                return Err(DeviceTokenError::DeviceIdConflict(device_id));
            }
        }

        // Atomically consume the ticket before issuing the device token.
        let consumed: ConsumedBootstrapTicket = self
            .store
            .consume_bootstrap_ticket(ticket, Some(&device_id))?;

        // Create or update the device record. For the MVP every paired remote
        // device gets the operator role and wildcard scope. The ticket's user
        // binding rides through as `Some`/`None` — `upsert_device`'s ON
        // CONFLICT COALESCEs it against the existing row
        // (`COALESCE(excluded.user_id, devices.user_id)`), so an unbound
        // re-pair (`None`) never clobbers an already-bound device. Passing
        // `Some(OWNER_USER_ID)` unconditionally here would defeat that
        // COALESCE and silently reassign an already-owned device back to the
        // owner on an unbound re-pair.
        self.store.upsert_device(&DeviceUpsertData {
            device_id: &device_id,
            device_name: &device_name,
            device_type: Some(PANEL_DEVICE_TYPE),
            public_key: b"",
            fingerprint: &device_id,
            role: "operator",
            scopes: &["*".to_string()],
            user_id: consumed.user_id.as_deref(),
        })?;

        // Unbound ticket + still-unbound device after the upsert above (i.e.
        // a brand-new pairing, not a re-pair of an already-owned device):
        // default to the owner. Must run strictly after the upsert and be
        // conditioned on `user_id IS NULL` — see the doc comment above.
        self.store
            .set_device_user_if_unbound(&device_id, OWNER_USER_ID)?;

        // Issue the device token.
        let token_id = format!("dt-{}", Uuid::new_v4());
        let device_token = format!("aleph-dt-{}", Uuid::new_v4());
        let token_hash = hash_device_token(&device_token);
        let expires_at = current_timestamp_ms() + DEFAULT_DEVICE_TOKEN_TTL_MS;

        self.store.issue_device_token(
            &token_id,
            &device_id,
            &token_hash,
            "operator",
            &["*".to_string()],
            expires_at,
        )?;

        Ok(BootstrapExchangeResult {
            device_id,
            device_token,
        })
    }

    /// Validate a device token and return its row if active.
    pub fn validate_device_token(
        &self,
        device_token: &str,
    ) -> Result<Option<DeviceTokenRow>, DeviceTokenError> {
        let hash = hash_device_token(device_token);
        Ok(self.store.validate_device_token_hash(&hash)?)
    }

    /// Revoke every token for a device and mark the device revoked.
    pub fn revoke_device(&self, device_id: &str) -> Result<bool, DeviceTokenError> {
        self.store.revoke_device_tokens(device_id)?;
        Ok(self.store.revoke_device(device_id)?)
    }

    /// Prune expired bootstrap tickets and device tokens older than `before_ms`.
    pub fn prune_expired(&self, before_ms: i64) -> Result<(usize, usize), DeviceTokenError> {
        let tickets = self.store.prune_expired_bootstrap_tickets(before_ms)?;
        let tokens = self.store.prune_expired_device_tokens(before_ms)?;
        Ok((tickets, tokens))
    }

    /// Prune expired bootstrap tickets and device tokens as of now. Thin
    /// wrapper for opportunistic pruning at RPC chokepoints (no daemon task).
    pub fn prune_now(&self) -> Result<(usize, usize), DeviceTokenError> {
        self.prune_expired(current_timestamp_ms())
    }

    /// List paired remote Panel devices (active only).
    ///
    /// Excludes cluster nodes (`role = "node"`), which are enrolled and
    /// deregistered through their own path (`cluster.rs`); only rows issued by
    /// the bootstrap-ticket exchange (`device_type = "panel"`) are returned.
    pub fn list_panel_devices(&self) -> Result<Vec<DeviceRow>, DeviceTokenError> {
        Ok(self
            .store
            .list_devices()?
            .into_iter()
            .filter(|d| d.device_type.as_deref() == Some(PANEL_DEVICE_TYPE))
            .collect())
    }

    /// Revoke one paired Panel device and all its tokens.
    ///
    /// Guarded to `device_type = "panel"` so a Panel-facing device-management
    /// RPC can never revoke a cluster node. Returns `false` when the id is
    /// unknown, already revoked, or not a Panel device.
    pub fn revoke_panel_device(&self, device_id: &str) -> Result<bool, DeviceTokenError> {
        let is_panel = self
            .list_panel_devices()?
            .iter()
            .any(|d| d.device_id == device_id);
        if !is_panel {
            return Ok(false);
        }
        self.revoke_device(device_id)
    }

    /// Revoke every paired Panel device and its tokens. Used when the shared
    /// Gateway token is rotated ("revoke all remotes"). Cluster nodes are left
    /// untouched. Returns the number of devices revoked.
    pub fn revoke_all_panel_devices(&self) -> Result<usize, DeviceTokenError> {
        let mut revoked = 0;
        for device in self.list_panel_devices()? {
            if self.revoke_device(&device.device_id)? {
                revoked += 1;
            }
        }
        Ok(revoked)
    }
}

/// Hash a device token for storage using SHA-256.
fn hash_device_token(token: &str) -> String {
    hex::encode(sha2::Sha256::digest(token.as_bytes()))
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::UserRole;

    fn manager() -> DeviceTokenManager {
        DeviceTokenManager::new(Arc::new(SecurityStore::in_memory().unwrap()))
    }

    /// Like [`manager`], but also hands back the backing store so a test can
    /// inspect user bindings directly (`store.device_user(..)`) without going
    /// through the manager's own API.
    fn manager_fixture() -> (DeviceTokenManager, Arc<SecurityStore>) {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = DeviceTokenManager::new(store.clone());
        (mgr, store)
    }

    #[test]
    fn ticket_bound_to_user_stamps_device_user() {
        let (mgr, store) = manager_fixture();
        store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
        let code = mgr.create_bootstrap_ticket(None, Some("u-alice")).unwrap();
        let result = mgr
            .exchange_bootstrap_ticket(&code, Some("dev-a".into()), None, None)
            .unwrap();
        assert_eq!(store.device_user("dev-a").unwrap().as_deref(), Some("u-alice"));
        let _ = result; // token issuance shape unchanged
    }

    #[test]
    fn unbound_ticket_defaults_device_to_owner() {
        let (mgr, store) = manager_fixture();
        let code = mgr.create_bootstrap_ticket(None, None).unwrap();
        mgr.exchange_bootstrap_ticket(&code, Some("dev-b".into()), None, None)
            .unwrap();
        assert_eq!(store.device_user("dev-b").unwrap().as_deref(), Some(OWNER_USER_ID));
    }

    #[test]
    fn repairing_does_not_silently_reassign_device_user() {
        // Mine-4 sibling: ON CONFLICT must COALESCE, not overwrite with NULL.
        let (mgr, store) = manager_fixture();
        store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
        let t1 = mgr.create_bootstrap_ticket(None, Some("u-alice")).unwrap();
        mgr.exchange_bootstrap_ticket(&t1, Some("dev-a".into()), None, None)
            .unwrap();
        let t2 = mgr.create_bootstrap_ticket(None, None).unwrap(); // unbound re-pair
        mgr.exchange_bootstrap_ticket(&t2, Some("dev-a".into()), None, None)
            .unwrap();
        assert_eq!(store.device_user("dev-a").unwrap().as_deref(), Some("u-alice"));
    }

    #[test]
    fn create_ticket_returns_valid_format() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();
        assert!(ticket.starts_with("aleph-bt-"));
        assert_eq!(ticket.len(), "aleph-bt-".len() + 36);
    }

    #[test]
    fn exchange_ticket_issues_device_token() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();

        let result = mgr
            .exchange_bootstrap_ticket(
                &ticket,
                Some("dev-1".to_string()),
                Some("Test Panel".to_string()),
                None,
            )
            .unwrap();

        assert_eq!(result.device_id, "dev-1");
        assert!(result.device_token.starts_with("aleph-dt-"));

        // Ticket cannot be reused.
        assert!(mgr
            .exchange_bootstrap_ticket(&ticket, None, None, None)
            .is_err());

        // Device token validates.
        let row = mgr.validate_device_token(&result.device_token).unwrap();
        assert!(row.is_some());
        let row = row.unwrap();
        assert_eq!(row.device_id, "dev-1");
        assert_eq!(row.role, "operator");
    }

    #[test]
    fn expired_ticket_cannot_be_exchanged() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(Some(-1), None).unwrap();
        assert!(mgr
            .exchange_bootstrap_ticket(&ticket, None, None, None)
            .is_err());
    }

    #[test]
    fn revoked_device_token_invalidated() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();
        let result = mgr
            .exchange_bootstrap_ticket(&ticket, Some("dev-x".to_string()), None, None)
            .unwrap();

        mgr.revoke_device("dev-x").unwrap();
        assert!(mgr
            .validate_device_token(&result.device_token)
            .unwrap()
            .is_none());
    }

    /// Seed a cluster node directly in the store (role=node), bypassing the
    /// bootstrap-ticket flow which only ever mints `device_type = "panel"`.
    fn seed_node(mgr: &DeviceTokenManager, device_id: &str) {
        mgr.store
            .upsert_device(&DeviceUpsertData {
                device_id,
                device_name: "a-node",
                device_type: Some("node"),
                public_key: b"",
                fingerprint: device_id,
                role: "node",
                scopes: &["*".to_string()],
                user_id: None,
            })
            .unwrap();
    }

    #[test]
    fn list_panel_devices_excludes_cluster_nodes() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();
        mgr.exchange_bootstrap_ticket(&ticket, Some("panel-1".to_string()), None, None)
            .unwrap();
        seed_node(&mgr, "node-1");

        let panels = mgr.list_panel_devices().unwrap();
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].device_id, "panel-1");
    }

    #[test]
    fn revoke_panel_device_refuses_cluster_node() {
        let mgr = manager();
        seed_node(&mgr, "node-1");
        // A node id is not a panel device: revoke is a no-op and leaves it active.
        assert!(!mgr.revoke_panel_device("node-1").unwrap());
        assert!(mgr
            .store
            .list_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == "node-1"));
    }

    #[test]
    fn revoke_all_panel_devices_spares_nodes_and_kills_tokens() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();
        let paired = mgr
            .exchange_bootstrap_ticket(&ticket, Some("panel-1".to_string()), None, None)
            .unwrap();
        seed_node(&mgr, "node-1");

        let revoked = mgr.revoke_all_panel_devices().unwrap();
        assert_eq!(revoked, 1);
        // Panel device token no longer validates.
        assert!(mgr
            .validate_device_token(&paired.device_token)
            .unwrap()
            .is_none());
        // Cluster node survives.
        assert!(mgr
            .store
            .list_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == "node-1"));
    }

    /// Regression: pairing must refuse a `device_id` that already names a
    /// non-Panel device. The `device_id` is client-asserted and the `devices`
    /// table is one namespace shared with cluster nodes, while
    /// `upsert_device`'s ON CONFLICT never rewrites `device_type` — so without
    /// this guard the exchange minted an operator token onto a `node`-typed row:
    /// invisible to `list_panel_devices`, untouched by `revoke_all_panel_devices`,
    /// and therefore surviving `gateway.token.rotate` (the "revoke everything"
    /// hammer). Same shape as the revoked-device-resurrects bug, one namespace over.
    #[test]
    fn pairing_refuses_to_adopt_a_cluster_node_device_id() {
        let mgr = manager();
        seed_node(&mgr, "node-1");

        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();
        let err = mgr
            .exchange_bootstrap_ticket(&ticket, Some("node-1".to_string()), None, None)
            .expect_err("pairing onto a cluster node id must fail closed");
        assert!(
            matches!(err, DeviceTokenError::DeviceIdConflict(ref id) if id == "node-1"),
            "expected DeviceIdConflict, got {err:?}"
        );

        // The node row is untouched: still a node, still listed by the cluster
        // path, still absent from the Panel roster.
        let row = mgr.store.get_device("node-1").unwrap().unwrap();
        assert_eq!(row.role, "node");
        assert!(mgr.list_panel_devices().unwrap().is_empty());

        // The rejection must not burn the operator's one-time ticket.
        let ok = mgr
            .exchange_bootstrap_ticket(&ticket, Some("panel-1".to_string()), None, None)
            .expect("ticket survives a rejected pairing");
        assert_eq!(ok.device_id, "panel-1");
    }

    /// A node adopted by `cluster::admit_node`'s unknown-id backfill has a NULL
    /// `device_type` (not `"node"`), so the guard must key on "is a panel",
    /// never on "is a node".
    #[test]
    fn pairing_refuses_untyped_device_rows() {
        let mgr = manager();
        mgr.store
            .upsert_device(&DeviceUpsertData {
                device_id: "legacy-1",
                device_name: "backfilled",
                device_type: None,
                public_key: b"",
                fingerprint: "legacy-1",
                role: "node",
                scopes: &["node".to_string()],
                user_id: None,
            })
            .unwrap();

        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();
        assert!(matches!(
            mgr.exchange_bootstrap_ticket(&ticket, Some("legacy-1".to_string()), None, None),
            Err(DeviceTokenError::DeviceIdConflict(_))
        ));
    }

    /// Regression: re-pairing a previously-revoked device must restore it to the
    /// listable + revocable roster. Before the `upsert_device` fix the row kept
    /// its `revoked_at` stamp, so the re-paired device held a working operator
    /// token yet was invisible to `list_panel_devices` and untouchable by
    /// `revoke_all_panel_devices` — it even survived a shared-token rotation.
    #[test]
    fn repairing_revoked_device_is_listable_and_revocable_again() {
        let mgr = manager();

        // Pair, then revoke.
        let t1 = mgr.create_bootstrap_ticket(None, None).unwrap();
        mgr.exchange_bootstrap_ticket(&t1, Some("dev-x".to_string()), None, None)
            .unwrap();
        assert!(mgr.revoke_device("dev-x").unwrap());
        assert!(!mgr
            .list_panel_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == "dev-x"));

        // Re-pair the same device_id with a fresh ticket.
        let t2 = mgr.create_bootstrap_ticket(None, None).unwrap();
        let repaired = mgr
            .exchange_bootstrap_ticket(&t2, Some("dev-x".to_string()), None, None)
            .unwrap();

        // New token works AND the device is back on the roster.
        assert!(mgr
            .validate_device_token(&repaired.device_token)
            .unwrap()
            .is_some());
        assert!(mgr
            .list_panel_devices()
            .unwrap()
            .iter()
            .any(|d| d.device_id == "dev-x"));

        // It is revocable again (was un-revocable while the row stayed hidden).
        assert!(mgr.revoke_panel_device("dev-x").unwrap());
        assert!(mgr
            .validate_device_token(&repaired.device_token)
            .unwrap()
            .is_none());
    }
}
