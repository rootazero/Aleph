//! Pairing Store
//!
//! Manages pairing requests for unknown senders.
//! Stores pending pairing codes and approved senders.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// A pending pairing request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Channel type (e.g., "imessage", "telegram")
    pub channel: String,
    /// Sender identifier
    pub sender_id: String,
    /// Pairing code (6 alphanumeric characters)
    pub code: String,
    /// When the request was created
    pub created_at: DateTime<Utc>,
    /// Additional metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Error type for pairing operations
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Pairing request not found")]
    NotFound,

    #[error("Invalid pairing code")]
    InvalidCode,

    #[error("Pairing request expired")]
    Expired,
}

/// Trait for pairing request storage
#[async_trait]
pub trait PairingStore: Send + Sync {
    /// Create or get existing pairing request for a sender
    /// Returns (code, `was_created`)
    async fn upsert(
        &self,
        channel: &str,
        sender_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<(String, bool), PairingError>;

    /// Approve a pairing request by code, adding sender to allowlist
    async fn approve(&self, channel: &str, code: &str) -> Result<PairingRequest, PairingError>;

    /// Reject/delete a pairing request
    async fn reject(&self, channel: &str, code: &str) -> Result<(), PairingError>;

    /// List pending pairing requests
    async fn list_pending(
        &self,
        channel: Option<&str>,
    ) -> Result<Vec<PairingRequest>, PairingError>;

    /// Check if a sender is in the approved list
    async fn is_approved(&self, channel: &str, sender_id: &str) -> Result<bool, PairingError>;

    /// Get approved senders for a channel
    async fn list_approved(&self, channel: &str) -> Result<Vec<String>, PairingError>;

    /// Remove a sender from the approved list
    async fn revoke(&self, channel: &str, sender_id: &str) -> Result<(), PairingError>;
}

/// Default pairing-code TTL (24h), mirroring the documented
/// `[routing] pairing_code_expiry_secs` default (`0` = never expire).
const DEFAULT_PAIRING_EXPIRY_SECS: u64 = 86_400;

/// SQLite-based pairing store
pub struct SqlitePairingStore {
    conn: Arc<Mutex<Connection>>,
    /// Max age (seconds) a pairing code stays approvable; `0` disables expiry.
    expiry_secs: u64,
}

impl SqlitePairingStore {
    /// Create a new `SQLite` pairing store
    pub fn new(db_path: impl AsRef<Path>) -> Result<Self, PairingError> {
        let conn = crate::utils::sqlite_open::open_sqlite_safe(db_path.as_ref())?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            expiry_secs: DEFAULT_PAIRING_EXPIRY_SECS,
        })
    }

    /// Create an in-memory pairing store (for testing)
    pub fn in_memory() -> Result<Self, PairingError> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            expiry_secs: DEFAULT_PAIRING_EXPIRY_SECS,
        })
    }

    /// Override the pairing-code expiry window (seconds). `0` disables expiry.
    ///
    /// Test-only knob today: production always uses
    /// [`DEFAULT_PAIRING_EXPIRY_SECS`]. This used to point at a
    /// `RoutingConfig::pairing_code_expiry_secs` setting that nothing read and
    /// that has since been removed — if an operator-facing TTL is wanted, it
    /// needs a real config field wired to this method, not a doc comment.
    #[must_use]
    pub fn with_expiry_secs(mut self, expiry_secs: u64) -> Self {
        self.expiry_secs = expiry_secs;
        self
    }

    fn init_schema(conn: &Connection) -> Result<(), PairingError> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS pairing_requests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                sender_id TEXT NOT NULL,
                code TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                metadata TEXT,
                UNIQUE(channel, sender_id)
            );

            CREATE TABLE IF NOT EXISTS approved_senders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel TEXT NOT NULL,
                sender_id TEXT NOT NULL,
                approved_at TEXT NOT NULL,
                UNIQUE(channel, sender_id)
            );

            CREATE INDEX IF NOT EXISTS idx_pairing_channel ON pairing_requests(channel);
            CREATE INDEX IF NOT EXISTS idx_pairing_code ON pairing_requests(code);
            CREATE INDEX IF NOT EXISTS idx_approved_channel ON approved_senders(channel);
            "#,
        )?;
        Ok(())
    }

    /// Generate a random 6-character alphanumeric code
    /// Uses only unambiguous characters (excludes 0, O, 1, I)
    fn generate_code() -> String {
        use rand::RngExt;
        const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        let mut rng = rand::rng();
        (0..6)
            .map(|_| {
                let idx = rng.random_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect()
    }
}

#[async_trait]
impl PairingStore for SqlitePairingStore {
    async fn upsert(
        &self,
        channel: &str,
        sender_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<(String, bool), PairingError> {
        let conn = self.conn.lock().await;

        // Check for existing request
        let existing: Option<String> = conn
            .query_row(
                "SELECT code FROM pairing_requests WHERE channel = ?1 AND sender_id = ?2",
                params![channel, sender_id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(code) = existing {
            debug!(
                "Found existing pairing request for {}:{}",
                channel, sender_id
            );
            return Ok((code, false));
        }

        // Create new request
        let code = Self::generate_code();
        let now = Utc::now().to_rfc3339();
        let metadata_json = serde_json::to_string(&metadata).unwrap_or_default();

        conn.execute(
            "INSERT INTO pairing_requests (channel, sender_id, code, created_at, metadata) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![channel, sender_id, code, now, metadata_json],
        )?;

        info!(
            "Created pairing request for {}:{} with code {}",
            channel, sender_id, code
        );
        Ok((code, true))
    }

    async fn approve(&self, channel: &str, code: &str) -> Result<PairingRequest, PairingError> {
        let conn = self.conn.lock().await;

        // Find the request
        let request: Option<(String, String, String, String)> = conn
            .query_row(
                "SELECT sender_id, code, created_at, metadata FROM pairing_requests WHERE channel = ?1 AND code = ?2",
                params![channel, code],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;

        let (sender_id, code, created_at, metadata_json) = request.ok_or(PairingError::NotFound)?;

        // Reject (and consume) an expired code before approving it: a leaked or
        // stale pairing code must not stay approvable forever. `expiry_secs == 0`
        // disables expiry, per the `[routing] pairing_code_expiry_secs` "0 = never"
        // contract.
        let created_at = DateTime::parse_from_rfc3339(&created_at)
            .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc));
        if self.expiry_secs > 0 {
            let ttl =
                chrono::Duration::seconds(i64::try_from(self.expiry_secs).unwrap_or(i64::MAX));
            if Utc::now().signed_duration_since(created_at) > ttl {
                // Consume the stale row so a later retry can't approve it either.
                conn.execute(
                    "DELETE FROM pairing_requests WHERE channel = ?1 AND code = ?2",
                    params![channel, code],
                )?;
                info!("Rejected expired pairing code for channel {}", channel);
                return Err(PairingError::Expired);
            }
        }

        // Approve + consume atomically: a crash between the INSERT and the
        // DELETE would otherwise leave the request pending while the sender is
        // already approved (an orphaned, re-approvable row).
        let now = Utc::now().to_rfc3339();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO approved_senders (channel, sender_id, approved_at) VALUES (?1, ?2, ?3)",
            params![channel, sender_id, now],
        )?;
        tx.execute(
            "DELETE FROM pairing_requests WHERE channel = ?1 AND code = ?2",
            params![channel, code],
        )?;
        tx.commit()?;

        let metadata: HashMap<String, String> =
            serde_json::from_str(&metadata_json).unwrap_or_default();

        info!("Approved pairing for {}:{}", channel, sender_id);

        Ok(PairingRequest {
            channel: channel.to_string(),
            sender_id,
            code,
            created_at,
            metadata,
        })
    }

    async fn reject(&self, channel: &str, code: &str) -> Result<(), PairingError> {
        let conn = self.conn.lock().await;
        let deleted = conn.execute(
            "DELETE FROM pairing_requests WHERE channel = ?1 AND code = ?2",
            params![channel, code],
        )?;

        if deleted == 0 {
            return Err(PairingError::NotFound);
        }

        info!("Rejected pairing request with code {}", code);
        Ok(())
    }

    async fn list_pending(
        &self,
        channel: Option<&str>,
    ) -> Result<Vec<PairingRequest>, PairingError> {
        let conn = self.conn.lock().await;

        let mut requests = Vec::new();

        if let Some(ch) = channel {
            let mut stmt = conn.prepare(
                "SELECT channel, sender_id, code, created_at, metadata FROM pairing_requests WHERE channel = ?1 ORDER BY created_at DESC"
            )?;
            let rows = stmt.query_map(params![ch], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;

            for row in rows {
                let (channel, sender_id, code, created_at, metadata_json) = row?;
                let metadata: HashMap<String, String> =
                    serde_json::from_str(&metadata_json).unwrap_or_default();
                let created_at = DateTime::parse_from_rfc3339(&created_at)
                    .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc));

                requests.push(PairingRequest {
                    channel,
                    sender_id,
                    code,
                    created_at,
                    metadata,
                });
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT channel, sender_id, code, created_at, metadata FROM pairing_requests ORDER BY created_at DESC"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;

            for row in rows {
                let (channel, sender_id, code, created_at, metadata_json) = row?;
                let metadata: HashMap<String, String> =
                    serde_json::from_str(&metadata_json).unwrap_or_default();
                let created_at = DateTime::parse_from_rfc3339(&created_at)
                    .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc));

                requests.push(PairingRequest {
                    channel,
                    sender_id,
                    code,
                    created_at,
                    metadata,
                });
            }
        }

        Ok(requests)
    }

    async fn is_approved(&self, channel: &str, sender_id: &str) -> Result<bool, PairingError> {
        let conn = self.conn.lock().await;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM approved_senders WHERE channel = ?1 AND sender_id = ?2",
                params![channel, sender_id],
                |row| row.get(0),
            )
            .optional()?;

        Ok(exists.is_some())
    }

    async fn list_approved(&self, channel: &str) -> Result<Vec<String>, PairingError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT sender_id FROM approved_senders WHERE channel = ?1 ORDER BY approved_at DESC",
        )?;
        let rows = stmt.query_map(params![channel], |row| row.get(0))?;

        let mut senders = Vec::new();
        for row in rows {
            senders.push(row?);
        }
        Ok(senders)
    }

    async fn revoke(&self, channel: &str, sender_id: &str) -> Result<(), PairingError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM approved_senders WHERE channel = ?1 AND sender_id = ?2",
            params![channel, sender_id],
        )?;
        info!("Revoked approval for {}:{}", channel, sender_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_upsert_creates_new_request() {
        let store = SqlitePairingStore::in_memory().unwrap();
        let (code, created) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        assert!(created);
        assert_eq!(code.len(), 6);
    }

    #[tokio::test]
    async fn test_upsert_returns_existing() {
        let store = SqlitePairingStore::in_memory().unwrap();

        let (code1, created1) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();
        let (code2, created2) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        assert!(created1);
        assert!(!created2);
        assert_eq!(code1, code2);
    }

    #[tokio::test]
    async fn test_approve_adds_to_approved() {
        let store = SqlitePairingStore::in_memory().unwrap();

        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        let request = store.approve("imessage", &code).await.unwrap();
        assert_eq!(request.sender_id, "+15551234567");

        // Should be approved now
        assert!(store.is_approved("imessage", "+15551234567").await.unwrap());

        // Pairing request should be deleted
        let pending = store.list_pending(Some("imessage")).await.unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_reject_deletes_request() {
        let store = SqlitePairingStore::in_memory().unwrap();

        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        store.reject("imessage", &code).await.unwrap();

        let pending = store.list_pending(Some("imessage")).await.unwrap();
        assert!(pending.is_empty());

        // Should NOT be approved
        assert!(!store.is_approved("imessage", "+15551234567").await.unwrap());
    }

    #[tokio::test]
    async fn test_list_pending() {
        let store = SqlitePairingStore::in_memory().unwrap();

        store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();
        store
            .upsert("imessage", "+15559876543", HashMap::new())
            .await
            .unwrap();
        store
            .upsert("telegram", "user123", HashMap::new())
            .await
            .unwrap();

        let all = store.list_pending(None).await.unwrap();
        assert_eq!(all.len(), 3);

        let imessage_only = store.list_pending(Some("imessage")).await.unwrap();
        assert_eq!(imessage_only.len(), 2);
    }

    #[tokio::test]
    async fn test_revoke() {
        let store = SqlitePairingStore::in_memory().unwrap();

        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();
        store.approve("imessage", &code).await.unwrap();

        assert!(store.is_approved("imessage", "+15551234567").await.unwrap());

        store.revoke("imessage", "+15551234567").await.unwrap();

        assert!(!store.is_approved("imessage", "+15551234567").await.unwrap());
    }

    #[tokio::test]
    async fn test_approve_rejects_expired_code() {
        let store = SqlitePairingStore::in_memory()
            .unwrap()
            .with_expiry_secs(3600);
        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        // Backdate the request beyond the TTL.
        {
            let conn = store.conn.lock().await;
            let past = (Utc::now() - chrono::Duration::seconds(7200)).to_rfc3339();
            conn.execute(
                "UPDATE pairing_requests SET created_at = ?1 WHERE code = ?2",
                rusqlite::params![past, code],
            )
            .unwrap();
        }

        // Approval is rejected as expired...
        assert!(matches!(
            store.approve("imessage", &code).await,
            Err(PairingError::Expired)
        ));
        // ...the stale row is consumed so a later retry can't approve it...
        assert!(store
            .list_pending(Some("imessage"))
            .await
            .unwrap()
            .is_empty());
        // ...and the sender was NOT approved.
        assert!(!store.is_approved("imessage", "+15551234567").await.unwrap());
    }

    #[tokio::test]
    async fn test_approve_zero_expiry_never_expires() {
        let store = SqlitePairingStore::in_memory().unwrap().with_expiry_secs(0);
        let (code, _) = store
            .upsert("imessage", "+15551234567", HashMap::new())
            .await
            .unwrap();

        // Even a very old code approves when expiry is disabled (0 = never).
        {
            let conn = store.conn.lock().await;
            let past = (Utc::now() - chrono::Duration::seconds(999_999)).to_rfc3339();
            conn.execute(
                "UPDATE pairing_requests SET created_at = ?1 WHERE code = ?2",
                rusqlite::params![past, code],
            )
            .unwrap();
        }

        assert!(store.approve("imessage", &code).await.is_ok());
        assert!(store.is_approved("imessage", "+15551234567").await.unwrap());
    }
}
