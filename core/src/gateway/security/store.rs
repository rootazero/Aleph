// core/src/gateway/security/store.rs

//! Unified SQLite storage for security data.
//!
//! Manages devices, tokens, pairing requests, and approved senders.

use rusqlite::{params, Connection, Result as SqliteResult};
use std::path::Path;
use crate::sync_primitives::Mutex;
use tracing::{debug, info};

/// Schema version for migrations
const SCHEMA_VERSION: i32 = 2;

/// Unified security storage backed by SQLite
pub struct SecurityStore {
    conn: Mutex<Connection>,
}

impl SecurityStore {
    /// Open or create a security store at the specified path
    pub fn open(path: impl AsRef<Path>) -> SqliteResult<Self> {
        let conn = Connection::open(path)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory store (for testing)
    pub fn in_memory() -> SqliteResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Get current schema version
    fn get_schema_version(&self) -> SqliteResult<i32> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
    }

    /// Set schema version
    fn set_schema_version(&self, version: i32) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(&format!("PRAGMA user_version = {}", version), [])?;
        Ok(())
    }

    /// Run database migrations
    fn migrate(&self) -> SqliteResult<()> {
        let version = self.get_schema_version()?;

        if version < SCHEMA_VERSION {
            info!(
                from = version,
                to = SCHEMA_VERSION,
                "Migrating security schema"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

            // Enable foreign key constraints
            conn.execute("PRAGMA foreign_keys = ON", [])?;

            // Drop old tables (force re-pairing)
            conn.execute_batch(
                r#"
                DROP TABLE IF EXISTS approved_devices;
                DROP TABLE IF EXISTS pairing_requests;
                DROP TABLE IF EXISTS approved_senders;
                DROP TABLE IF EXISTS tokens;
                "#,
            )?;

            // Create new schema
            conn.execute_batch(SCHEMA_V2)?;

            drop(conn);
            self.set_schema_version(SCHEMA_VERSION)?;

            info!("Security schema migration complete");
        }

        debug!("Security store initialized (schema v{})", SCHEMA_VERSION);
        Ok(())
    }

    // ========== Device Operations ==========

    /// Insert or update a device
    pub fn upsert_device(&self, data: &DeviceUpsertData<'_>) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let scopes_json = serde_json::to_string(data.scopes).unwrap_or_else(|e| {
            tracing::warn!("Failed to serialize device scopes: {}", e);
            "[]".to_string()
        });

        // ON CONFLICT behavior: device_name is updated, but approved_at is preserved
        // from the original INSERT. This means re-pairing a device keeps its original
        // approval timestamp, while updating the name and refreshing last_seen_at.
        conn.execute(
            r#"INSERT INTO devices
               (device_id, device_name, device_type, public_key, fingerprint, role, scopes, created_at, approved_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
               ON CONFLICT(device_id) DO UPDATE SET
                 device_name = excluded.device_name,
                 last_seen_at = ?8"#,
            params![data.device_id, data.device_name, data.device_type, data.public_key, data.fingerprint, data.role, scopes_json, now],
        )?;
        Ok(())
    }

    /// Get device by ID
    pub fn get_device(&self, device_id: &str) -> SqliteResult<Option<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE device_id = ?1",
        )?;

        let result = stmt.query_row(params![device_id], DeviceRow::from_row);

        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get device by fingerprint
    pub fn get_device_by_fingerprint(&self, fingerprint: &str) -> SqliteResult<Option<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE fingerprint = ?1 AND revoked_at IS NULL",
        )?;

        let result = stmt.query_row(params![fingerprint], DeviceRow::from_row);

        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check if device is approved (not revoked)
    pub fn is_device_approved(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM devices WHERE device_id = ?1 AND revoked_at IS NULL",
            params![device_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// List all active devices
    pub fn list_devices(&self) -> SqliteResult<Vec<DeviceRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE revoked_at IS NULL ORDER BY approved_at DESC",
        )?;

        let devices = stmt
            .query_map([], DeviceRow::from_row)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(devices)
    }

    /// Update device last_seen_at
    pub fn touch_device(&self, device_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        conn.execute(
            "UPDATE devices SET last_seen_at = ?1 WHERE device_id = ?2",
            params![now, device_id],
        )?;
        Ok(())
    }

    /// Revoke a device
    pub fn revoke_device(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "UPDATE devices SET revoked_at = ?1 WHERE device_id = ?2 AND revoked_at IS NULL",
            params![now, device_id],
        )?;
        Ok(rows > 0)
    }

    // ========== Token Operations ==========

    /// Insert a new token
    pub fn insert_token(
        &self,
        token_id: &str,
        device_id: &str,
        token_hash: &str,
        role: &str,
        scopes: &[String],
        expires_at: i64,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let scopes_json = serde_json::to_string(scopes).unwrap_or_else(|e| {
            tracing::warn!("Failed to serialize token scopes: {}", e);
            "[]".to_string()
        });

        conn.execute(
            r#"INSERT INTO tokens (token_id, device_id, token_hash, role, scopes, issued_at, expires_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            params![token_id, device_id, token_hash, role, scopes_json, now, expires_at],
        )?;
        Ok(())
    }

    /// Get token by hash
    pub fn get_token_by_hash(&self, token_hash: &str) -> SqliteResult<Option<TokenRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();

        let mut stmt = conn.prepare(
            "SELECT token_id, device_id, token_hash, role, scopes, issued_at, expires_at,
                    last_used_at, rotated_at, revoked_at
             FROM tokens
             WHERE token_hash = ?1 AND revoked_at IS NULL AND expires_at > ?2",
        )?;

        let result = stmt.query_row(params![token_hash, now], TokenRow::from_row);

        match result {
            Ok(token) => Ok(Some(token)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Update token last_used_at
    pub fn touch_token(&self, token_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        conn.execute(
            "UPDATE tokens SET last_used_at = ?1 WHERE token_id = ?2",
            params![now, token_id],
        )?;
        Ok(())
    }

    /// Revoke a token
    pub fn revoke_token(&self, token_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE token_id = ?2 AND revoked_at IS NULL",
            params![now, token_id],
        )?;
        Ok(rows > 0)
    }

    /// Revoke all tokens for a device
    pub fn revoke_device_tokens(&self, device_id: &str) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE device_id = ?2 AND revoked_at IS NULL",
            params![now, device_id],
        )?;
        Ok(rows as u64)
    }

    /// Delete expired tokens
    pub fn delete_expired_tokens(&self) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "DELETE FROM tokens WHERE expires_at < ?1 OR revoked_at IS NOT NULL",
            params![now],
        )?;
        Ok(rows as u64)
    }

    // ========== Pairing Request Operations ==========

    /// Insert a pairing request
    pub fn insert_pairing_request(&self, data: &PairingRequestData<'_>) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();

        conn.execute(
            r#"INSERT INTO pairing_requests
               (request_id, code, pairing_type, device_name, device_type, public_key,
                channel, sender_id, remote_addr, created_at, expires_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
            params![
                data.request_id, data.code, data.pairing_type, data.device_name,
                data.device_type, data.public_key, data.channel, data.sender_id,
                data.remote_addr, now, data.expires_at
            ],
        )?;
        Ok(())
    }

    /// Get pairing request by code
    pub fn get_pairing_request(&self, code: &str) -> SqliteResult<Option<PairingRequestRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();

        let mut stmt = conn.prepare(
            "SELECT request_id, code, pairing_type, device_name, device_type, public_key,
                    channel, sender_id, remote_addr, metadata, created_at, expires_at
             FROM pairing_requests
             WHERE code = ?1 AND expires_at > ?2",
        )?;

        let result = stmt.query_row(params![code, now], PairingRequestRow::from_row);

        match result {
            Ok(req) => Ok(Some(req)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete a pairing request
    pub fn delete_pairing_request(&self, code: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn.execute("DELETE FROM pairing_requests WHERE code = ?1", params![code])?;
        Ok(rows > 0)
    }

    /// List pending pairing requests
    pub fn list_pairing_requests(&self) -> SqliteResult<Vec<PairingRequestRow>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();

        let mut stmt = conn.prepare(
            "SELECT request_id, code, pairing_type, device_name, device_type, public_key,
                    channel, sender_id, remote_addr, metadata, created_at, expires_at
             FROM pairing_requests
             WHERE expires_at > ?1
             ORDER BY created_at DESC",
        )?;

        let requests = stmt
            .query_map(params![now], PairingRequestRow::from_row)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(requests)
    }

    /// Count pending pairing requests
    pub fn count_pairing_requests(&self) -> SqliteResult<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pairing_requests WHERE expires_at > ?1",
            params![now],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Delete expired pairing requests
    pub fn delete_expired_pairing_requests(&self) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let rows = conn.execute("DELETE FROM pairing_requests WHERE expires_at < ?1", params![now])?;
        Ok(rows as u64)
    }

    // ========== Approved Senders Operations ==========

    /// Approve a channel sender
    pub fn approve_sender(&self, channel: &str, sender_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();

        conn.execute(
            r#"INSERT INTO approved_senders (channel, sender_id, approved_at)
               VALUES (?1, ?2, ?3)
               ON CONFLICT(channel, sender_id) DO UPDATE SET
                 revoked_at = NULL, approved_at = excluded.approved_at"#,
            params![channel, sender_id, now],
        )?;
        Ok(())
    }

    /// Check if sender is approved
    pub fn is_sender_approved(&self, channel: &str, sender_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM approved_senders WHERE channel = ?1 AND sender_id = ?2 AND revoked_at IS NULL",
            params![channel, sender_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Revoke a sender
    pub fn revoke_sender(&self, channel: &str, sender_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "UPDATE approved_senders SET revoked_at = ?1 WHERE channel = ?2 AND sender_id = ?3 AND revoked_at IS NULL",
            params![now, channel, sender_id],
        )?;
        Ok(rows > 0)
    }

    /// List approved senders for a channel
    pub fn list_senders(&self, channel: &str) -> SqliteResult<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT sender_id, approved_at FROM approved_senders WHERE channel = ?1 AND revoked_at IS NULL",
        )?;

        let senders = stmt
            .query_map(params![channel], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(senders)
    }
}

// ========== Parameter Structs ==========

/// Data for inserting or creating a pairing request.
pub struct PairingRequestData<'a> {
    pub request_id: &'a str,
    pub code: &'a str,
    pub pairing_type: &'a str,
    pub device_name: Option<&'a str>,
    pub device_type: Option<&'a str>,
    pub public_key: Option<&'a [u8]>,
    pub channel: Option<&'a str>,
    pub sender_id: Option<&'a str>,
    pub remote_addr: Option<&'a str>,
    pub expires_at: i64,
}

/// Data for inserting or updating a device.
pub struct DeviceUpsertData<'a> {
    pub device_id: &'a str,
    pub device_name: &'a str,
    pub device_type: Option<&'a str>,
    pub public_key: &'a [u8],
    pub fingerprint: &'a str,
    pub role: &'a str,
    pub scopes: &'a [String],
}

// ========== Row Types ==========

/// Device row from database
#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub device_id: String,
    pub device_name: String,
    pub device_type: Option<String>,
    pub public_key: Vec<u8>,
    pub fingerprint: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub approved_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl DeviceRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let scopes_json: String = row.get(6)?;
        let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_else(|e| {
            tracing::warn!("Failed to deserialize device scopes: {}", e);
            Vec::new()
        });

        Ok(Self {
            device_id: row.get(0)?,
            device_name: row.get(1)?,
            device_type: row.get(2)?,
            public_key: row.get(3)?,
            fingerprint: row.get(4)?,
            role: row.get(5)?,
            scopes,
            created_at: row.get(7)?,
            approved_at: row.get(8)?,
            last_seen_at: row.get(9)?,
            revoked_at: row.get(10)?,
        })
    }
}

/// Token row from database
#[derive(Debug, Clone)]
pub struct TokenRow {
    pub token_id: String,
    pub device_id: String,
    pub token_hash: String,
    pub role: String,
    pub scopes: Vec<String>,
    pub issued_at: i64,
    pub expires_at: i64,
    pub last_used_at: Option<i64>,
    pub rotated_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl TokenRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let scopes_json: String = row.get(4)?;
        let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_else(|e| {
            tracing::warn!("Failed to deserialize token scopes: {}", e);
            Vec::new()
        });

        Ok(Self {
            token_id: row.get(0)?,
            device_id: row.get(1)?,
            token_hash: row.get(2)?,
            role: row.get(3)?,
            scopes,
            issued_at: row.get(5)?,
            expires_at: row.get(6)?,
            last_used_at: row.get(7)?,
            rotated_at: row.get(8)?,
            revoked_at: row.get(9)?,
        })
    }
}

/// Pairing request row from database
#[derive(Debug, Clone)]
pub struct PairingRequestRow {
    pub request_id: String,
    pub code: String,
    pub pairing_type: String,
    pub device_name: Option<String>,
    pub device_type: Option<String>,
    pub public_key: Option<Vec<u8>>,
    pub channel: Option<String>,
    pub sender_id: Option<String>,
    pub remote_addr: Option<String>,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

impl PairingRequestRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            request_id: row.get(0)?,
            code: row.get(1)?,
            pairing_type: row.get(2)?,
            device_name: row.get(3)?,
            device_type: row.get(4)?,
            public_key: row.get(5)?,
            channel: row.get(6)?,
            sender_id: row.get(7)?,
            remote_addr: row.get(8)?,
            metadata: row.get(9)?,
            created_at: row.get(10)?,
            expires_at: row.get(11)?,
        })
    }

    /// Calculate remaining seconds until expiry
    pub fn remaining_secs(&self) -> u64 {
        let now = current_timestamp_ms();
        if self.expires_at > now {
            ((self.expires_at - now) / 1000) as u64
        } else {
            0
        }
    }
}

/// Get current timestamp in milliseconds
fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Schema v2 SQL
const SCHEMA_V2: &str = r#"
CREATE TABLE devices (
    device_id       TEXT PRIMARY KEY,
    device_name     TEXT NOT NULL,
    device_type     TEXT,
    public_key      BLOB NOT NULL,
    fingerprint     TEXT NOT NULL UNIQUE,
    role            TEXT NOT NULL DEFAULT 'operator',
    scopes          TEXT NOT NULL DEFAULT '["*"]',
    created_at      INTEGER NOT NULL,
    approved_at     INTEGER NOT NULL,
    last_seen_at    INTEGER,
    revoked_at      INTEGER
);

CREATE TABLE tokens (
    token_id        TEXT PRIMARY KEY,
    device_id       TEXT NOT NULL,
    token_hash      TEXT NOT NULL,
    role            TEXT NOT NULL,
    scopes          TEXT NOT NULL,
    issued_at       INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    last_used_at    INTEGER,
    rotated_at      INTEGER,
    revoked_at      INTEGER,
    FOREIGN KEY (device_id) REFERENCES devices(device_id)
);

CREATE INDEX idx_tokens_device ON tokens(device_id);
CREATE INDEX idx_tokens_expires ON tokens(expires_at);
CREATE INDEX idx_tokens_hash ON tokens(token_hash);

CREATE TABLE pairing_requests (
    request_id      TEXT PRIMARY KEY,
    code            TEXT NOT NULL UNIQUE,
    pairing_type    TEXT NOT NULL,
    device_name     TEXT,
    device_type     TEXT,
    public_key      BLOB,
    channel         TEXT,
    sender_id       TEXT,
    remote_addr     TEXT,
    metadata        TEXT,
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    CHECK (pairing_type IN ('device', 'channel'))
);

CREATE INDEX idx_pairing_code ON pairing_requests(code);
CREATE INDEX idx_pairing_expires ON pairing_requests(expires_at);

CREATE TABLE approved_senders (
    channel         TEXT NOT NULL,
    sender_id       TEXT NOT NULL,
    approved_at     INTEGER NOT NULL,
    revoked_at      INTEGER,
    PRIMARY KEY (channel, sender_id)
);
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_migration() {
        let store = SecurityStore::in_memory().unwrap();
        assert_eq!(store.get_schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn test_device_crud() {
        let store = SecurityStore::in_memory().unwrap();

        // Insert device
        store
            .upsert_device(&DeviceUpsertData {
                device_id: "dev-1",
                device_name: "Test Device",
                device_type: Some("macos"),
                public_key: &[1u8; 32],
                fingerprint: "abc123",
                role: "operator",
                scopes: &["*".to_string()],
            })
            .unwrap();

        // Get device
        let device = store.get_device("dev-1").unwrap().unwrap();
        assert_eq!(device.device_name, "Test Device");
        assert_eq!(device.fingerprint, "abc123");

        // List devices
        let devices = store.list_devices().unwrap();
        assert_eq!(devices.len(), 1);

        // Revoke device
        assert!(store.revoke_device("dev-1").unwrap());
        assert!(!store.is_device_approved("dev-1").unwrap());
    }

    #[test]
    fn test_token_crud() {
        let store = SecurityStore::in_memory().unwrap();

        // Need a device first
        store
            .upsert_device(&DeviceUpsertData {
                device_id: "dev-1",
                device_name: "Test",
                device_type: None,
                public_key: &[1u8; 32],
                fingerprint: "fp",
                role: "operator",
                scopes: &[],
            })
            .unwrap();

        let expires = current_timestamp_ms() + 3600000; // 1 hour

        // Insert token
        store
            .insert_token("tok-1", "dev-1", "hash123", "operator", &["*".to_string()], expires)
            .unwrap();

        // Get token
        let token = store.get_token_by_hash("hash123").unwrap().unwrap();
        assert_eq!(token.device_id, "dev-1");

        // Revoke token
        assert!(store.revoke_token("tok-1").unwrap());
        assert!(store.get_token_by_hash("hash123").unwrap().is_none());
    }

    #[test]
    fn test_pairing_request_crud() {
        let store = SecurityStore::in_memory().unwrap();

        let expires = current_timestamp_ms() + 300000; // 5 minutes

        // Insert device pairing request
        store
            .insert_pairing_request(&PairingRequestData {
                request_id: "req-1",
                code: "A3B7K9M2",
                pairing_type: "device",
                device_name: Some("iPhone"),
                device_type: Some("ios"),
                public_key: Some(&[1u8; 32]),
                channel: None,
                sender_id: None,
                remote_addr: Some("192.168.1.1"),
                expires_at: expires,
            })
            .unwrap();

        // Get by code
        let req = store.get_pairing_request("A3B7K9M2").unwrap().unwrap();
        assert_eq!(req.device_name, Some("iPhone".to_string()));
        assert!(req.remaining_secs() > 0);

        // Delete
        assert!(store.delete_pairing_request("A3B7K9M2").unwrap());
        assert!(store.get_pairing_request("A3B7K9M2").unwrap().is_none());
    }

    #[test]
    fn test_sender_approval() {
        let store = SecurityStore::in_memory().unwrap();

        // Approve sender
        store.approve_sender("telegram", "user123").unwrap();
        assert!(store.is_sender_approved("telegram", "user123").unwrap());

        // List senders
        let senders = store.list_senders("telegram").unwrap();
        assert_eq!(senders.len(), 1);

        // Revoke
        assert!(store.revoke_sender("telegram", "user123").unwrap());
        assert!(!store.is_sender_approved("telegram", "user123").unwrap());
    }
}
