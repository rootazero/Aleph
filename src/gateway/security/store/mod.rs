// core/src/gateway/security/store.rs

//! Unified SQLite storage for security data.
//!
//! Manages devices, tokens, pairing requests, and approved senders.

use crate::sync_primitives::Mutex;
use rusqlite::{Connection, Result as SqliteResult};
use std::path::Path;
use tracing::{debug, info};

mod devices;
mod pairing;
mod senders;
mod sessions;
mod tokens;
mod types;

#[cfg(test)]
mod tests;

pub use types::*;

/// Schema version for migrations
const SCHEMA_VERSION: i32 = 10;

/// Unified security storage backed by SQLite
pub struct SecurityStore {
    pub(crate) conn: Mutex<Connection>,
}

impl SecurityStore {
    /// Open or create a security store at the specified path
    pub fn open(path: impl AsRef<Path>) -> SqliteResult<Self> {
        let conn = crate::utils::sqlite_open::open_sqlite_safe(path.as_ref())?;
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
    pub(crate) fn get_schema_version(&self) -> SqliteResult<i32> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
    }

    /// Set schema version
    pub(crate) fn set_schema_version(&self, version: i32) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(&format!("PRAGMA user_version = {version}"), [])?;
        Ok(())
    }

    /// Run database migrations
    pub(crate) fn migrate(&self) -> SqliteResult<()> {
        let version = self.get_schema_version()?;

        if version < 2 {
            info!(from = version, to = 2, "Migrating security schema to v2");

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
        }

        if version < 3 {
            info!(from = version, to = 3, "Migrating security schema to v3");

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(SCHEMA_V3)?;
            drop(conn);
        }

        if version < 4 {
            info!(
                from = version,
                to = 4,
                "Migrating security schema to v4 (persistent HMAC secret)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(SCHEMA_V4)?;
            drop(conn);
        }

        if version < 5 {
            info!(
                from = version,
                to = 5,
                "Migrating security schema to v5 (token manager secret persistence)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(SCHEMA_V5)?;
            drop(conn);
        }

        if version < 6 {
            info!(
                from = version,
                to = 6,
                "Migrating security schema to v6 (plaintext token persistence)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            // Idempotent: only add column if it doesn't already exist
            let has_column: bool = conn
                .prepare("SELECT plaintext_token FROM shared_token LIMIT 0")
                .is_ok();
            if !has_column {
                conn.execute_batch(SCHEMA_V6)?;
            }
            drop(conn);
        }

        if version < 7 {
            info!(
                from = version,
                to = 7,
                "Migrating security schema to v7 (security audit log)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(crate::security::audit::AUDIT_LOG_SCHEMA)?;
            drop(conn);
        }

        if version < 8 {
            info!(
                from = version,
                to = 8,
                "Migrating security schema to v8 (channel policies)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(SCHEMA_V8)?;
            drop(conn);
            self.set_schema_version(8)?;
        }

        if version < 9 {
            info!(
                from = version,
                to = 9,
                "Migrating security schema to v9 (browser pairing variant)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(SCHEMA_V9)?;
            drop(conn);
            self.set_schema_version(9)?;
        }

        if version < 10 {
            info!(
                from = version,
                to = 10,
                "Migrating security schema to v10 (cluster-node pairing variant)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(SCHEMA_V10)?;
            drop(conn);
            self.set_schema_version(10)?;
        }

        // Final safety: ensure version is at latest
        self.set_schema_version(SCHEMA_VERSION)?;

        info!("Security schema migration complete");
        debug!("Security store initialized (schema v{})", SCHEMA_VERSION);
        Ok(())
    }

    /// Insert a single security audit entry. Used by the audit drain task.
    pub fn insert_audit_entry(
        &self,
        entry: &crate::security::audit::AuditEntry,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            crate::security::audit::AUDIT_INSERT_SQL,
            rusqlite::params![
                entry.event_type.to_string(),
                entry.severity.to_string(),
                entry.source_ip.as_deref(),
                entry.session_id.as_deref(),
                entry.detail.as_str(),
            ],
        )?;
        Ok(())
    }
}

/// Get current timestamp in milliseconds
pub(crate) fn current_timestamp_ms() -> i64 {
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
    CHECK (pairing_type IN ('device', 'channel', 'browser'))
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

/// Schema v3 SQL — incremental additions for shared token and sessions
const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS shared_token (
    token_hash  TEXT PRIMARY KEY,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    session_id    TEXT PRIMARY KEY,
    token_hash    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL,
    last_used_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);
"#;

/// Schema v4 SQL — add HMAC secret column to shared_token for persistence across restarts
const SCHEMA_V4: &str = r#"
ALTER TABLE shared_token ADD COLUMN hmac_secret BLOB;
"#;

const SCHEMA_V5: &str = r#"
CREATE TABLE IF NOT EXISTS token_manager_secret (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    hmac_secret BLOB NOT NULL,
    created_at  INTEGER NOT NULL
);
"#;

/// Schema v6 SQL — store plaintext token in DB for recovery when file is lost
const SCHEMA_V6: &str = r#"
ALTER TABLE shared_token ADD COLUMN plaintext_token TEXT;
"#;

/// Schema v8 SQL — channel policies persistence
const SCHEMA_V8: &str = r#"
CREATE TABLE IF NOT EXISTS channel_policies (
    channel_id  TEXT NOT NULL,
    policy_type TEXT NOT NULL,
    policy      TEXT NOT NULL,
    allowlist   TEXT,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (channel_id, policy_type)
);
CREATE INDEX IF NOT EXISTS idx_channel_policies_channel ON channel_policies(channel_id);
"#;

/// Schema v9 SQL — browser pairing variant.
///
/// SQLite can `ADD COLUMN` freely, but it cannot edit a `CHECK` constraint
/// in place. The v2 table was created with `CHECK (pairing_type IN
/// ('device', 'channel'))` — to admit the new `'browser'` value we must
/// rebuild the table. Pending pairings are short-lived (5min default) so
/// nuking the table is the safe move; legacy rows are never useful after
/// a daemon restart anyway.
const SCHEMA_V9: &str = r#"
DROP TABLE IF EXISTS pairing_requests;
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
    origin_label    TEXT,
    user_agent      TEXT,
    peer_ip         TEXT,
    CHECK (pairing_type IN ('device', 'channel', 'browser'))
);
CREATE INDEX IF NOT EXISTS idx_pairing_code ON pairing_requests(code);
CREATE INDEX IF NOT EXISTS idx_pairing_expires ON pairing_requests(expires_at);
"#;

/// Schema v10 SQL — cluster-node pairing variant.
///
/// Same rationale as v9: SQLite cannot edit a `CHECK` constraint in place, so
/// to admit the new `'node'` value we rebuild the table. Pending pairings are
/// short-lived (5min default) and never useful after a daemon restart, so
/// dropping the table is the safe move.
const SCHEMA_V10: &str = r#"
DROP TABLE IF EXISTS pairing_requests;
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
    origin_label    TEXT,
    user_agent      TEXT,
    peer_ip         TEXT,
    CHECK (pairing_type IN ('device', 'channel', 'browser', 'node'))
);
CREATE INDEX IF NOT EXISTS idx_pairing_code ON pairing_requests(code);
CREATE INDEX IF NOT EXISTS idx_pairing_expires ON pairing_requests(expires_at);
"#;
