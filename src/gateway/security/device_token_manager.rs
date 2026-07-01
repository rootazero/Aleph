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
    BootstrapTicketError, ConsumedBootstrapTicket, DeviceTokenRow, DeviceUpsertData, SecurityStore,
};
use crate::sync_primitives::Arc;

/// Default bootstrap ticket lifetime: 5 minutes.
const DEFAULT_BOOTSTRAP_TTL_MS: i64 = 5 * 60 * 1000;

/// Default device token lifetime: 10 years (effectively indefinite for MVP).
const DEFAULT_DEVICE_TOKEN_TTL_MS: i64 = 10 * 365 * 24 * 60 * 60 * 1000;

/// Device-token manager errors.
#[derive(Debug, thiserror::Error)]
pub enum DeviceTokenError {
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Invalid or expired bootstrap ticket")]
    InvalidBootstrapTicket,
    #[error("Device token not found or revoked")]
    InvalidDeviceToken,
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
    /// `ttl_ms` defaults to 5 minutes when `None`.
    pub fn create_bootstrap_ticket(&self, ttl_ms: Option<i64>) -> Result<String, DeviceTokenError> {
        let code = format!("aleph-bt-{}", Uuid::new_v4());
        let ttl = ttl_ms.unwrap_or(DEFAULT_BOOTSTRAP_TTL_MS);
        // Negative TTL is allowed for tests that need an immediately-expired ticket.
        let ttl = if ttl < 0 { ttl } else { ttl.max(60_000) };
        self.store.create_bootstrap_ticket(&code, ttl)?;
        Ok(code)
    }

    /// Exchange a bootstrap ticket for a per-device token.
    ///
    /// If `device_id` is not provided, a random UUID is assigned. The device
    /// record is created with operator role and wildcard scope for the MVP.
    pub fn exchange_bootstrap_ticket(
        &self,
        ticket: &str,
        device_id: Option<String>,
        device_name: Option<String>,
        _public_key: Option<Vec<u8>>,
    ) -> Result<BootstrapExchangeResult, DeviceTokenError> {
        let device_id = device_id.unwrap_or_else(|| format!("device-{}", Uuid::new_v4()));
        let device_name = device_name.unwrap_or_else(|| "Remote Panel".to_string());

        // Atomically consume the ticket before issuing the device token.
        let _consumed: ConsumedBootstrapTicket =
            self.store.consume_bootstrap_ticket(ticket, Some(&device_id))?;

        // Create or update the device record. For the MVP every paired remote
        // device gets the operator role and wildcard scope.
        self.store.upsert_device(&DeviceUpsertData {
            device_id: &device_id,
            device_name: &device_name,
            device_type: Some("panel"),
            public_key: b"",
            fingerprint: &device_id,
            role: "operator",
            scopes: &["*".to_string()],
        })?;

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

    fn manager() -> DeviceTokenManager {
        DeviceTokenManager::new(Arc::new(SecurityStore::in_memory().unwrap()))
    }

    #[test]
    fn create_ticket_returns_valid_format() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(None).unwrap();
        assert!(ticket.starts_with("aleph-bt-"));
        assert_eq!(ticket.len(), "aleph-bt-".len() + 36);
    }

    #[test]
    fn exchange_ticket_issues_device_token() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(None).unwrap();

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
        let ticket = mgr.create_bootstrap_ticket(Some(-1)).unwrap();
        assert!(mgr
            .exchange_bootstrap_ticket(&ticket, None, None, None)
            .is_err());
    }

    #[test]
    fn revoked_device_token_invalidated() {
        let mgr = manager();
        let ticket = mgr.create_bootstrap_ticket(None).unwrap();
        let result = mgr
            .exchange_bootstrap_ticket(&ticket, Some("dev-x".to_string()), None, None)
            .unwrap();

        mgr.revoke_device("dev-x").unwrap();
        assert!(mgr.validate_device_token(&result.device_token).unwrap().is_none());
    }
}
