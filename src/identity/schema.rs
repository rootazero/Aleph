//! `SQLite` schema for agent identities and the signed operation ledger.
//!
//! The consts live here (the domain module) and are applied by
//! [`SecurityStore::migrate`](crate::gateway::security::store::SecurityStore) —
//! the same split `security_audit_log` already uses
//! ([`crate::security::audit::AUDIT_LOG_SCHEMA`]): the domain owns the shape,
//! the store owns the file.
//!
//! Both tables live in `security.db` rather than a new file, so the ledger
//! inherits `open_sqlite_safe` (WAL, `busy_timeout`, cross-process safety —
//! Spec C) and the existing boot wiring without adding a store.

/// Keys are keyed by **fingerprint**, not by agent id, because rotation must
/// keep the retired key verifiable: records signed before a rotation can only
/// be checked against the key that signed them. `retired_at` marks a key as no
/// longer signing; it never removes it.
///
/// `agent_identities` carries the **chain anchor** (`head_seq` / `head_hash`).
/// Keeping the expected head in a different row than the chain is what lets
/// [`verify`](super::verify) detect tail truncation — the failure mode a plain
/// hash chain is structurally blind to, because a truncated chain is still
/// internally consistent.
///
/// `agent_ledger_health` is a single row counting appends this installation
/// **failed** to write. It is durable rather than a process counter because a
/// record that was never written is invisible to every chain check, and the
/// surface that most needs to know — the offline `aleph-server identity verify`,
/// used precisely when the writing daemon is not trusted — runs in a different
/// process and would otherwise always read zero. A daemon restart would lose it
/// too. Every read of the verdict reports it alongside, so a clean `ok` is
/// never mistaken for "complete".
pub const IDENTITY_SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS agent_keys (
    fingerprint TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL,
    public_key  BLOB NOT NULL,
    created_at  INTEGER NOT NULL,
    retired_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_agent_keys_agent ON agent_keys(agent_id);

CREATE TABLE IF NOT EXISTS agent_identities (
    agent_id           TEXT PRIMARY KEY,
    active_fingerprint TEXT NOT NULL,
    created_at         INTEGER NOT NULL,
    revoked_at         INTEGER,
    head_seq           INTEGER NOT NULL DEFAULT 0,
    head_hash          BLOB
);

CREATE TABLE IF NOT EXISTS agent_ledger (
    agent_id   TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    prev_hash  BLOB,
    hash       BLOB NOT NULL,
    signature  BLOB NOT NULL,
    signer_fp  TEXT NOT NULL,
    action     TEXT NOT NULL,
    target     TEXT NOT NULL,
    outcome    TEXT NOT NULL,
    args_fp    TEXT,
    detail     TEXT NOT NULL,
    at_ms      INTEGER NOT NULL,
    -- The person the row is attributable to (`users.user_id`), NULL when the
    -- action had no human behind it (cron, heartbeat, A2A, a chain's own
    -- opening record) and on every row written before this column existed.
    -- Added in schema v16; see `LEDGER_ADD_PRINCIPAL_SQL` for the ALTER an
    -- already-created table needs (re-running this CREATE cannot reach it).
    principal  TEXT,
    PRIMARY KEY (agent_id, seq)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_ledger_hash ON agent_ledger(agent_id, hash);
CREATE INDEX IF NOT EXISTS idx_agent_ledger_at ON agent_ledger(at_ms);

CREATE TABLE IF NOT EXISTS agent_ledger_health (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    lost_appends INTEGER NOT NULL DEFAULT 0,
    last_lost_ms INTEGER
);
";

/// Reach an `agent_ledger` that already exists.
///
/// Lives beside the `CREATE` rather than inside `SecurityStore::migrate` for
/// the same reason [`IDENTITY_SCHEMA`] does: the domain owns the shape, the
/// store owns the file. Mirrors `security::audit::AUDIT_LOG_ADD_ACTOR_SQL`.
pub const LEDGER_ADD_PRINCIPAL_SQL: &str = "ALTER TABLE agent_ledger ADD COLUMN principal TEXT";

/// Columns selected by every ledger read, in the order
/// [`row_to_record`](super::record::row_to_record) expects.
pub const LEDGER_COLUMNS: &str =
    "agent_id, seq, prev_hash, hash, signature, signer_fp, action, target, outcome, args_fp, \
     detail, at_ms, principal";
