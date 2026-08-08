// core/src/gateway/security/store.rs

//! Unified `SQLite` storage for security data.
//!
//! Under LAN-trust this store is the persistence layer for the secret vault
//! master key (shared-token chain), cluster-node device records, remote Panel
//! device-auth tokens + bootstrap tickets (see `tokens.rs` / `devices.rs` /
//! `bootstrap_tickets.rs`), channel sender policies, the security audit log,
//! and agent signing identities + their signed operation ledger (see
//! `identity.rs`; the domain logic lives in [`crate::identity`]).
//!
//! Legacy tables: the migration chain below still creates `sessions` and
//! `pairing_requests` (SCHEMA_V2/V3; v9/v10 rebuild `pairing_requests` for the
//! 'browser'/'node' CHECK variants), but no code on HEAD reads or writes those
//! two anymore — they are kept solely so existing databases upgrade cleanly.
//! Do not wire new features to them. The `tokens` table is **not** dead: the
//! device-auth token store in `tokens.rs` actively reads and writes it (the
//! remote Panel device-token flow reuses it).

use crate::sync_primitives::Mutex;
use rusqlite::{Connection, Result as SqliteResult};
use std::path::Path;
use tracing::{debug, info};

mod bootstrap_tickets;
mod devices;
mod identity;
mod tokens;
mod types;
mod users;

#[cfg(test)]
mod tests;

pub use bootstrap_tickets::{BootstrapTicketError, ConsumedBootstrapTicket};
pub use types::*;
pub use users::{UserRecord, UserRole, UserStatus, OWNER_USER_ID};

/// Schema version for migrations
const SCHEMA_VERSION: i32 = 15;

/// Unified security storage backed by `SQLite`
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
        conn.pragma_update(None, "user_version", version)?;
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

        if version < 11 {
            info!(
                from = version,
                to = 11,
                "Migrating security schema to v11 (bootstrap tickets for panel pairing)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(SCHEMA_V11)?;
            drop(conn);
            self.set_schema_version(11)?;
        }

        if version < 12 {
            info!(
                from = version,
                to = 12,
                "Migrating security schema to v12 (agent identities + signed ledger)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(crate::identity::IDENTITY_SCHEMA)?;
            drop(conn);
            self.set_schema_version(12)?;
        }

        if version < 13 {
            info!(
                from = version,
                to = 13,
                "Migrating security schema to v13 (durable ledger loss counter)"
            );

            // The whole identity batch is `CREATE ... IF NOT EXISTS`, so
            // re-running it adds only the new table and keeps one definition
            // of the shape rather than a second, drifting copy here.
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(crate::identity::IDENTITY_SCHEMA)?;
            drop(conn);
            self.set_schema_version(13)?;
        }

        if version < 14 {
            info!("Migrating security store to v14 (users + identity linking)");
            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute_batch(users::USERS_SCHEMA)?;
            // ALTER TABLE ADD COLUMN is not idempotent in SQLite — the version gate
            // guarantees single execution.
            conn.execute("ALTER TABLE devices ADD COLUMN user_id TEXT", [])?;
            conn.execute("ALTER TABLE bootstrap_tickets ADD COLUMN user_id TEXT", [])?;
            drop(conn);
            self.set_schema_version(14)?;
        }

        if version < 15 {
            info!(
                from = version,
                to = 15,
                "Migrating security schema to v15 (audit actor column)"
            );

            let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            // The version gate alone is NOT enough here, unlike v14's two
            // ALTERs: a store created from scratch runs every arm in order and
            // v7 already built the table from `AUDIT_LOG_SCHEMA`, which now
            // carries `actor_user` — so the ALTER would hit `duplicate column
            // name` on every FIRST boot. Probe like v6 does.
            let has_column: bool = conn
                .prepare("SELECT actor_user FROM security_audit_log LIMIT 0")
                .is_ok();
            if !has_column {
                conn.execute(crate::security::audit::AUDIT_LOG_ADD_ACTOR_SQL, [])?;
            }
            drop(conn);
            self.set_schema_version(15)?;
        }
        // After all versioned migrations (runs on every open, idempotent):
        self.ensure_bootstrap_owner()?;

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
                entry.actor_user.as_deref(),
                entry.detail.as_str(),
            ],
        )?;
        Ok(())
    }

    /// Delete audit entries older than `retention_secs`. Returns the number of
    /// rows removed. Called periodically by the audit drain task so the
    /// `security_audit_log` table does not grow without bound.
    pub fn purge_audit_entries(&self, retention_secs: i64) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            crate::security::audit::AUDIT_CLEANUP_SQL,
            rusqlite::params![retention_secs],
        )
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

-- Accessorless since the dead-twin CUT: nothing in the tree reads or writes
-- this table. The live approved-sender authority is `gateway::pairing_store`
-- (a different database), consulted by `inbound_router::check_permission`.
-- The DDL stays only so the v2 migration keeps producing the schema every
-- deployed database already has; do not add accessors here.
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

/// Schema v4 SQL — add HMAC secret column to `shared_token` for persistence across restarts
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

/// Schema v8 SQL — channel policies persistence.
///
/// Accessorless since the dead-twin CUT: the per-channel dm/group policy that
/// is actually enforced comes from `[channels.*]` config bridged into
/// `gateway::channel_policy` + `inbound_router::check_permission`; this table
/// never had a runtime writer. DDL retained for schema continuity only.
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
/// `SQLite` can `ADD COLUMN` freely, but it cannot edit a `CHECK` constraint
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
/// Same rationale as v9: `SQLite` cannot edit a `CHECK` constraint in place, so
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

/// Schema v11 SQL — bootstrap tickets for remote Panel pairing.
///
/// One-time, short-lived codes exchanged for a per-device token during the
/// WebSocket `connect` handshake. Keeps long-lived shared tokens out of URLs.
const SCHEMA_V11: &str = r#"
CREATE TABLE IF NOT EXISTS bootstrap_tickets (
    code                    TEXT PRIMARY KEY,
    created_at              INTEGER NOT NULL,
    expires_at              INTEGER NOT NULL,
    consumed_at             INTEGER,
    consumed_by_device_id   TEXT
);
CREATE INDEX IF NOT EXISTS idx_bootstrap_expires ON bootstrap_tickets(expires_at);
CREATE INDEX IF NOT EXISTS idx_bootstrap_consumed ON bootstrap_tickets(consumed_at);
"#;
