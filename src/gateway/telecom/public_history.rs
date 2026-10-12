//! Encrypted, bounded, isolated history of completed public telecom turns
//! (public context addendum §4.3; execution contract §3.7).
//!
//! # What this is
//!
//! A tiny SQLite-backed store that keeps **completed** public turns (caller
//! text + the reply that was produced for it) so a later public run for the
//! same binding and caller principal can see them. It is deliberately *not* the
//! owner session service, not `session_events`, not memory/profile/global history and not a
//! journal: it is a separate database with its own dedicated key, handed in by
//! the caller (no key is derived from login/shared-token credentials and none
//! is persisted beside the database).
//!
//! # Guarantees
//!
//! - **Typed key.** A session is addressed only by [`PublicHistorySession`],
//!   derived from the typed [`TelecomProvenance`] (`region` + `binding_ref` +
//!   caller digest). The channel (SMS / voice alias) and
//!   `connection_generation` are *not* part of the key: aliases share one
//!   principal and a lease reconnect must not orphan history. The owner session
//!   key never appears here. The key is stored as a SHA-256 digest, never as
//!   the raw binding reference or raw phone number.
//! - **Encrypted at rest.** Plaintext lives only inside an AES-256-GCM payload
//!   ([`SecretsCrypto`]); the payload repeats the session digest and both
//!   timestamps and they are re-checked on read, so a ciphertext cannot be
//!   moved to another row or session without being refused.
//! - **Bounded.** Per turn-row TTL (default and ceiling 24h; `0` disables
//!   persistence), at most 32 turns and 64 KiB of text per session, at most
//!   8 MiB of ciphertext in the whole store. Only complete turns are evicted
//!   (oldest first, same session only). If the global cap cannot be met the
//!   append is **refused** — it never evicts another session's history and
//!   never spills anywhere else.
//! - **Fail closed.** A row that fails to decrypt (wrong key, tampering,
//!   truncation) or whose payload disagrees with its row makes the whole
//!   [`PublicHistoryStore::load`] fail with [`PublicHistoryError::Corrupt`]
//!   without deleting unexpired rows; there is no plaintext fallback, partial
//!   result or automatic reset. Restoring the original key can recover valid
//!   history. Ordinary TTL pruning still deletes expired rows before reading.
//!
//! # Non-guarantees
//!
//! SQLite WAL / backups may retain deleted ciphertext; logical deletion is not
//! physical erasure. Ciphertext length leaks approximate turn size. The key is
//! the caller's responsibility; production installs a store only when the
//! dedicated startup configuration and key pass validation (see
//! [`install_global`]).

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::{params, Connection, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::public_context::TelecomProvenance;
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::secrets::crypto::SecretsCrypto;
use crate::secrets::types::SecretError;
use crate::sync_primitives::Arc;
use crate::utils::sqlite_open::open_sqlite_safe;

/// Ceiling (and default) of a turn's retention.
pub const PUBLIC_HISTORY_MAX_TTL: Duration = Duration::hours(24);
/// Ceiling (and default) of retained complete turns per session.
pub const PUBLIC_HISTORY_MAX_TURNS: usize = 32;
/// Ceiling (and default) of retained plaintext bytes per session.
pub const PUBLIC_HISTORY_MAX_SESSION_BYTES: usize = 64 * 1024;
/// Ceiling (and default) of total encrypted payload bytes in the store.
pub const PUBLIC_HISTORY_MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;

const PAYLOAD_VERSION: u32 = 1;

/// Resource limits. Constructed only through [`Default`] (the frozen numbers)
/// or [`Self::with_limits`], which can lower but never exceed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicHistoryConfig {
    ttl: Duration,
    max_turns: usize,
    max_session_bytes: usize,
    max_total_bytes: usize,
}

impl Default for PublicHistoryConfig {
    fn default() -> Self {
        Self {
            ttl: PUBLIC_HISTORY_MAX_TTL,
            max_turns: PUBLIC_HISTORY_MAX_TURNS,
            max_session_bytes: PUBLIC_HISTORY_MAX_SESSION_BYTES,
            max_total_bytes: PUBLIC_HISTORY_MAX_TOTAL_BYTES,
        }
    }
}

impl PublicHistoryConfig {
    /// Lower the frozen limits. `ttl == 0` disables persistence of cross-turn
    /// history; any other limit must be non-zero and within its ceiling.
    pub fn with_limits(
        ttl: Duration,
        max_turns: usize,
        max_session_bytes: usize,
        max_total_bytes: usize,
    ) -> Result<Self, PublicHistoryError> {
        let bad = |what: &str| Err(PublicHistoryError::InvalidConfig(what.to_string()));
        if ttl < Duration::zero() || ttl > PUBLIC_HISTORY_MAX_TTL {
            return bad("ttl must be within 0..=24h");
        }
        if max_turns == 0 || max_turns > PUBLIC_HISTORY_MAX_TURNS {
            return bad("max_turns must be within 1..=32");
        }
        if max_session_bytes == 0 || max_session_bytes > PUBLIC_HISTORY_MAX_SESSION_BYTES {
            return bad("max_session_bytes must be within 1..=64KiB");
        }
        if max_total_bytes == 0 || max_total_bytes > PUBLIC_HISTORY_MAX_TOTAL_BYTES {
            return bad("max_total_bytes must be within 1..=8MiB");
        }
        Ok(Self {
            ttl,
            max_turns,
            max_session_bytes,
            max_total_bytes,
        })
    }
}

/// Typed address of one public history. Built only from admitted-ingress
/// provenance; carries a digest, never the raw binding reference or caller
/// phone number.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PublicHistorySession(String);

impl PublicHistorySession {
    pub fn from_provenance(p: &TelecomProvenance) -> Self {
        let mut h = Sha256::new();
        h.update(b"aleph-telecom-public-history-v2\0");
        h.update(p.region().as_bytes());
        h.update([0u8]);
        h.update(p.binding_ref().as_bytes());
        h.update([0u8]);
        h.update(p.caller_digest().as_str().as_bytes());
        Self(hex::encode(h.finalize()))
    }
}

impl std::fmt::Debug for PublicHistorySession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PublicHistorySession(<redacted>)")
    }
}

/// One completed public turn: the caller's text and the reply produced for it.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicHistoryTurn {
    pub user: String,
    pub assistant: String,
}

impl PublicHistoryTurn {
    fn text_bytes(&self) -> usize {
        self.user.len().saturating_add(self.assistant.len())
    }
}

impl std::fmt::Debug for PublicHistoryTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PublicHistoryTurn")
            .field("user", &"<redacted>")
            .field("assistant", &"<redacted>")
            .finish()
    }
}

/// Result of a successful [`PublicHistoryStore::append`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicHistoryAppend {
    /// The turn was stored; `evicted` older complete turns of the same
    /// session were dropped to stay within the per-session limits.
    Stored { evicted: usize },
    /// Persistence is disabled (`ttl == 0`); nothing was written.
    Disabled,
}

#[derive(Debug, thiserror::Error)]
pub enum PublicHistoryError {
    #[error("invalid public history configuration: {0}")]
    InvalidConfig(String),
    #[error("dedicated public history key is missing or unusable")]
    MissingKey,
    #[error("one public turn exceeds the per-session history limit")]
    TurnTooLarge,
    #[error("public history store is at its encrypted capacity")]
    Capacity,
    #[error("public history is corrupt, undecryptable or does not match its row")]
    Corrupt,
    #[error(transparent)]
    Crypto(#[from] SecretError),
    #[error(transparent)]
    Storage(#[from] rusqlite::Error),
}

#[derive(Serialize, Deserialize)]
struct Payload {
    version: u32,
    session: String,
    created_at_ms: i64,
    expires_at_ms: i64,
    user: String,
    assistant: String,
}

/// File-backed, mutex-serialized encrypted history store.
pub struct PublicHistoryStore {
    conn: Mutex<Connection>,
    crypto: SecretsCrypto,
    config: PublicHistoryConfig,
}

impl PublicHistoryStore {
    /// Open a durable store with a caller-supplied dedicated key.
    pub fn open(
        path: &Path,
        crypto: SecretsCrypto,
        config: PublicHistoryConfig,
    ) -> Result<Self, PublicHistoryError> {
        if !crypto.is_usable() {
            return Err(PublicHistoryError::MissingKey);
        }
        Self::init(open_sqlite_safe(path)?, crypto, config)
    }

    /// Ephemeral store for tests.
    pub fn open_in_memory(
        crypto: SecretsCrypto,
        config: PublicHistoryConfig,
    ) -> Result<Self, PublicHistoryError> {
        if !crypto.is_usable() {
            return Err(PublicHistoryError::MissingKey);
        }
        Self::init(Connection::open_in_memory()?, crypto, config)
    }

    fn init(
        conn: Connection,
        crypto: SecretsCrypto,
        config: PublicHistoryConfig,
    ) -> Result<Self, PublicHistoryError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS telecom_public_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                plain_bytes INTEGER NOT NULL,
                ciphertext BLOB NOT NULL,
                nonce BLOB NOT NULL,
                salt BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_telecom_public_history_session
                ON telecom_public_history(session, id);",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            crypto,
            config,
        })
    }

    fn guard(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Completed, unexpired turns of `session`, oldest first. Any undecryptable
    /// or inconsistent row fails the whole call without a partial result or
    /// deletion of unexpired rows. Ordinary TTL pruning still applies.
    pub fn load(
        &self,
        session: &PublicHistorySession,
        now: DateTime<Utc>,
    ) -> Result<Vec<PublicHistoryTurn>, PublicHistoryError> {
        let conn = self.guard();
        prune_expired(&conn, now)?;
        let rows = {
            let mut stmt = conn.prepare(
                "SELECT created_at, expires_at, plain_bytes, ciphertext, nonce, salt
                 FROM telecom_public_history WHERE session = ?1 ORDER BY id ASC",
            )?;
            let mapped = stmt.query_map(params![session.0], |r| {
                Ok(RawRow {
                    created_at: r.get(0)?,
                    expires_at: r.get(1)?,
                    plain_bytes: r.get(2)?,
                    ciphertext: r.get(3)?,
                    nonce: r.get(4)?,
                    salt: r.get(5)?,
                })
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };
        let mut turns = Vec::with_capacity(rows.len());
        for row in &rows {
            // Fail closed without erasing history: a wrong operator key must
            // not reset the session. `?` discards any earlier readable turns.
            turns.push(self.decrypt_row(session, row)?);
        }
        Ok(turns)
    }

    /// Store one completed turn finishing at `now`.
    pub fn append(
        &self,
        session: &PublicHistorySession,
        turn: &PublicHistoryTurn,
        now: DateTime<Utc>,
    ) -> Result<PublicHistoryAppend, PublicHistoryError> {
        if self.config.ttl.is_zero() {
            return Ok(PublicHistoryAppend::Disabled);
        }
        let plain_bytes = turn.text_bytes();
        if plain_bytes > self.config.max_session_bytes {
            return Err(PublicHistoryError::TurnTooLarge);
        }
        let expires_at = now
            .checked_add_signed(self.config.ttl)
            .ok_or(PublicHistoryError::Corrupt)?;
        let payload = Payload {
            version: PAYLOAD_VERSION,
            session: session.0.clone(),
            created_at_ms: now.timestamp_millis(),
            expires_at_ms: expires_at.timestamp_millis(),
            user: turn.user.clone(),
            assistant: turn.assistant.clone(),
        };
        let json = serde_json::to_string(&payload).map_err(|_| PublicHistoryError::Corrupt)?;
        let encrypted = self.crypto.encrypt(&json)?;
        let new_cipher_len = encrypted.ciphertext.len();

        let mut conn = self.guard();
        prune_expired(&conn, now)?;
        let tx = conn.transaction()?;
        let evict = plan_eviction(&tx, &session.0, plain_bytes, &self.config)?;
        let total: i64 = tx.query_row(
            "SELECT COALESCE(SUM(LENGTH(ciphertext)), 0) FROM telecom_public_history",
            [],
            |r| r.get(0),
        )?;
        let total = usize::try_from(total).map_err(|_| PublicHistoryError::Corrupt)?;
        let after = total
            .saturating_sub(evict.cipher_bytes)
            .saturating_add(new_cipher_len);
        if after > self.config.max_total_bytes {
            // Dropping `tx` rolls the planned evictions back: a refused append
            // never costs the session its existing history.
            return Err(PublicHistoryError::Capacity);
        }
        for id in &evict.ids {
            tx.execute(
                "DELETE FROM telecom_public_history WHERE id = ?1",
                params![id],
            )?;
        }
        tx.execute(
            "INSERT INTO telecom_public_history
                (session, created_at, expires_at, plain_bytes, ciphertext, nonce, salt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session.0,
                payload.created_at_ms,
                payload.expires_at_ms,
                i64::try_from(plain_bytes).map_err(|_| PublicHistoryError::Corrupt)?,
                encrypted.ciphertext,
                encrypted.nonce.as_slice(),
                encrypted.salt.as_slice(),
            ],
        )?;
        tx.commit()?;
        Ok(PublicHistoryAppend::Stored {
            evicted: evict.ids.len(),
        })
    }

    fn decrypt_row(
        &self,
        session: &PublicHistorySession,
        row: &RawRow,
    ) -> Result<PublicHistoryTurn, PublicHistoryError> {
        let nonce: [u8; 12] = row
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| PublicHistoryError::Corrupt)?;
        let salt: [u8; 32] = row
            .salt
            .as_slice()
            .try_into()
            .map_err(|_| PublicHistoryError::Corrupt)?;
        // Crypto errors (wrong key, tamper, bad UTF-8) are collapsed into
        // `Corrupt` so callers cannot distinguish them or fall back.
        let plaintext = self
            .crypto
            .decrypt(&row.ciphertext, &nonce, &salt)
            .map_err(|_| PublicHistoryError::Corrupt)?;
        let payload: Payload =
            serde_json::from_str(&plaintext).map_err(|_| PublicHistoryError::Corrupt)?;
        let turn = PublicHistoryTurn {
            user: payload.user,
            assistant: payload.assistant,
        };
        if payload.version != PAYLOAD_VERSION
            || payload.session != session.0
            || payload.created_at_ms != row.created_at
            || payload.expires_at_ms != row.expires_at
            || i64::try_from(turn.text_bytes()).ok() != Some(row.plain_bytes)
            || ms_to_datetime(row.created_at).is_none()
        {
            return Err(PublicHistoryError::Corrupt);
        }
        Ok(turn)
    }
}

struct RawRow {
    created_at: i64,
    expires_at: i64,
    plain_bytes: i64,
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
    salt: Vec<u8>,
}

struct Eviction {
    ids: Vec<i64>,
    cipher_bytes: usize,
}

/// Oldest complete turns of `session` that must go so one more turn of
/// `new_plain` bytes fits the per-session turn and byte limits.
fn plan_eviction(
    tx: &Transaction<'_>,
    session: &str,
    new_plain: usize,
    cfg: &PublicHistoryConfig,
) -> Result<Eviction, PublicHistoryError> {
    let mut stmt = tx.prepare(
        "SELECT id, plain_bytes, LENGTH(ciphertext) FROM telecom_public_history
         WHERE session = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt
        .query_map(params![session], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let to_usize = |v: i64| usize::try_from(v).map_err(|_| PublicHistoryError::Corrupt);
    let mut turns = rows.len();
    let mut bytes = 0usize;
    for (_, plain, _) in &rows {
        bytes = bytes.saturating_add(to_usize(*plain)?);
    }
    let mut ev = Eviction {
        ids: Vec::new(),
        cipher_bytes: 0,
    };
    for (id, plain, cipher) in &rows {
        if turns < cfg.max_turns && bytes.saturating_add(new_plain) <= cfg.max_session_bytes {
            break;
        }
        ev.ids.push(*id);
        ev.cipher_bytes = ev.cipher_bytes.saturating_add(to_usize(*cipher)?);
        turns -= 1;
        bytes = bytes.saturating_sub(to_usize(*plain)?);
    }
    Ok(ev)
}

fn prune_expired(conn: &Connection, now: DateTime<Utc>) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM telecom_public_history WHERE expires_at <= ?1",
        params![now.timestamp_millis()],
    )?;
    Ok(())
}

fn ms_to_datetime(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

static INSTALLED: CapabilitySlot<Arc<PublicHistoryStore>> = CapabilitySlot::new(
    "gateway/telecom/public_history",
    MissingSemantics::ConsumerDecides,
);

/// Install the process-wide store used by the public SMS driver. Returns the
/// store back if one is already installed.
pub fn install_global(store: Arc<PublicHistoryStore>) -> Result<(), Arc<PublicHistoryStore>> {
    if INSTALLED.install(store.clone()) {
        Ok(())
    } else {
        Err(store)
    }
}

/// Record that startup reached the history slot but deliberately did not
/// install it. The reason is static because it is operator-facing diagnostics.
pub fn decline_global(because: &'static str) {
    INSTALLED.decline(because);
}

/// The installed store, if any.
pub fn installed() -> Option<&'static Arc<PublicHistoryStore>> {
    INSTALLED.get()
}

/// Capability census accessor; the store value itself is not exposed here.
pub(crate) const fn installed_slot() -> &'static dyn SlotStatus {
    &INSTALLED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::telecom::persisted_marker::{MarkerRestore, PersistedTelecomMarker};
    use crate::gateway::telecom::public_context::test_support::{
        sample_public_context, sample_public_context_for_peer,
    };

    const KEY: &str = "dedicated-public-history-key";

    #[test]
    fn capability_slot_missing_semantics_is_consumer_decides() {
        assert!(matches!(
            INSTALLED.missing(),
            MissingSemantics::ConsumerDecides
        ));
        let slot: &dyn SlotStatus = &INSTALLED;
        assert_eq!(slot.id(), "gateway/telecom/public_history");
    }

    fn session(binding: &str) -> PublicHistorySession {
        PublicHistorySession::from_provenance(sample_public_context(binding).provenance())
    }

    fn session_for_peer(binding: &str, peer_address: &str) -> PublicHistorySession {
        PublicHistorySession::from_provenance(
            sample_public_context_for_peer(binding, peer_address).provenance(),
        )
    }

    fn voice_session_for_same_caller(
        binding: &str,
        sms_peer_address: &str,
    ) -> PublicHistorySession {
        let sms = sample_public_context_for_peer(binding, sms_peer_address);
        let marker: PersistedTelecomMarker = serde_json::from_value(serde_json::json!({
            "version": 2,
            "state": "public",
            "channel": "voice",
            "region": sms.provenance().region(),
            "binding_ref": sms.provenance().binding_ref(),
            "caller_digest": sms.provenance().caller_digest().as_str(),
            "connection_generation": 99,
        }))
        .unwrap();
        match marker.restore() {
            MarkerRestore::Public(ctx) => PublicHistorySession::from_provenance(ctx.provenance()),
            other => panic!("expected restored voice public context, got {other:?}"),
        }
    }

    fn turn(user: &str, assistant: &str) -> PublicHistoryTurn {
        PublicHistoryTurn {
            user: user.into(),
            assistant: assistant.into(),
        }
    }

    fn t0() -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000, 0).unwrap()
    }

    fn store(cfg: PublicHistoryConfig) -> PublicHistoryStore {
        PublicHistoryStore::open_in_memory(SecretsCrypto::new(KEY), cfg).unwrap()
    }

    fn count(s: &PublicHistoryStore) -> i64 {
        s.guard()
            .query_row("SELECT COUNT(*) FROM telecom_public_history", [], |r| {
                r.get(0)
            })
            .unwrap()
    }

    // Include row identity, metadata and all encrypted bytes so a failed read
    // cannot silently delete, replace or rewrite retained history.
    type StoredRowSnapshot = (i64, String, i64, i64, i64, Vec<u8>, Vec<u8>, Vec<u8>);

    fn encrypted_rows(s: &PublicHistoryStore) -> Vec<StoredRowSnapshot> {
        let conn = s.guard();
        let mut stmt = conn
            .prepare(
                "SELECT id, session, created_at, expires_at, plain_bytes, ciphertext, nonce, salt
                 FROM telecom_public_history ORDER BY id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    #[test]
    fn round_trips_in_order_through_api_only() {
        let s = store(PublicHistoryConfig::default());
        let a = session("bind-a");
        s.append(&a, &turn("q1 lilac-heron-91", "a1"), t0())
            .unwrap();
        s.append(
            &a,
            &turn("q2", "a2 amber-quokka-17"),
            t0() + Duration::seconds(5),
        )
        .unwrap();
        let got = s.load(&a, t0() + Duration::seconds(10)).unwrap();
        assert_eq!(
            got,
            vec![
                turn("q1 lilac-heron-91", "a1"),
                turn("q2", "a2 amber-quokka-17")
            ]
        );
    }

    #[test]
    fn raw_storage_never_contains_plaintext_raw_binding_or_raw_phone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("public-history.db");
        let s = PublicHistoryStore::open(
            &path,
            SecretsCrypto::new(KEY),
            PublicHistoryConfig::default(),
        )
        .unwrap();
        let a = session("binding-raw-ref-zzz");
        let (u, r) = ("caller-canary-violet-otter-42", "reply-canary-teal-lynx-77");
        s.append(&a, &turn(u, r), t0()).unwrap();
        drop(s);
        let bytes = std::fs::read(&path).unwrap();
        for needle in [u, r, "binding-raw-ref-zzz", "test-region", "+15550000001"] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "plaintext {needle} found in raw db"
            );
        }
        // Reopen with the same key: durable and still readable via the API.
        let s = PublicHistoryStore::open(
            &path,
            SecretsCrypto::new(KEY),
            PublicHistoryConfig::default(),
        )
        .unwrap();
        assert_eq!(s.load(&a, t0()).unwrap(), vec![turn(u, r)]);
    }

    #[test]
    fn ttl_expires_each_turn_independently_and_physically_deletes() {
        let s = store(PublicHistoryConfig::default());
        let a = session("bind-a");
        s.append(&a, &turn("old", "old"), t0()).unwrap();
        s.append(&a, &turn("new", "new"), t0() + Duration::hours(10))
            .unwrap();
        // Exact expiry boundary of the first turn: gone; second still there.
        let at = t0() + PUBLIC_HISTORY_MAX_TTL;
        assert_eq!(s.load(&a, at).unwrap(), vec![turn("new", "new")]);
        assert_eq!(count(&s), 1, "expired row is deleted, not just hidden");
        let later = t0() + Duration::hours(10) + PUBLIC_HISTORY_MAX_TTL;
        assert!(s.load(&a, later).unwrap().is_empty());
        assert_eq!(count(&s), 0);
    }

    #[test]
    fn zero_ttl_disables_persistence() {
        let cfg = PublicHistoryConfig::with_limits(Duration::zero(), 32, 65536, 1 << 20).unwrap();
        let s = store(cfg);
        let a = session("bind-a");
        assert_eq!(
            s.append(&a, &turn("q", "a"), t0()).unwrap(),
            PublicHistoryAppend::Disabled
        );
        assert_eq!(count(&s), 0);
        assert!(s.load(&a, t0()).unwrap().is_empty());
    }

    #[test]
    fn config_cannot_exceed_frozen_limits_or_be_zero() {
        let ok = |t, n, b, m| PublicHistoryConfig::with_limits(t, n, b, m).is_ok();
        assert!(ok(PUBLIC_HISTORY_MAX_TTL, 32, 64 * 1024, 8 << 20));
        assert!(!ok(PUBLIC_HISTORY_MAX_TTL + Duration::seconds(1), 32, 1, 1));
        assert!(!ok(Duration::seconds(-1), 32, 1, 1));
        assert!(!ok(Duration::hours(1), 33, 1, 1));
        assert!(!ok(Duration::hours(1), 0, 1, 1));
        assert!(!ok(Duration::hours(1), 1, 64 * 1024 + 1, 1));
        assert!(!ok(Duration::hours(1), 1, 0, 1));
        assert!(!ok(Duration::hours(1), 1, 1, (8 << 20) + 1));
        assert!(!ok(Duration::hours(1), 1, 1, 0));
        let d = PublicHistoryConfig::default();
        assert_eq!(
            (d.ttl, d.max_turns, d.max_session_bytes, d.max_total_bytes),
            (Duration::hours(24), 32, 65536, 8 * 1024 * 1024)
        );
    }

    #[test]
    fn thirty_two_turn_limit_evicts_oldest_complete_turns_only() {
        let s = store(PublicHistoryConfig::default());
        let a = session("bind-a");
        for i in 0..34 {
            s.append(
                &a,
                &turn(&format!("u{i}"), &format!("r{i}")),
                t0() + Duration::seconds(i),
            )
            .unwrap();
        }
        let got = s.load(&a, t0() + Duration::seconds(60)).unwrap();
        assert_eq!(got.len(), 32);
        assert_eq!(got.first().unwrap(), &turn("u2", "r2"));
        assert_eq!(got.last().unwrap(), &turn("u33", "r33"));
    }

    #[test]
    fn sixty_four_kib_session_limit_evicts_and_oversized_turn_is_refused() {
        let s = store(PublicHistoryConfig::default());
        let a = session("bind-a");
        let half = "x".repeat(16 * 1024);
        for i in 0..4 {
            s.append(
                &a,
                &turn(&half, &format!("{i}")),
                t0() + Duration::seconds(i),
            )
            .unwrap();
        }
        // Each turn is 16KiB + 1 byte, so four exceed 64KiB: the oldest was
        // evicted when the fourth arrived and three remain.
        let got = s.load(&a, t0() + Duration::seconds(10)).unwrap();
        let bytes: usize = got.iter().map(|t| t.text_bytes()).sum();
        assert!(bytes <= PUBLIC_HISTORY_MAX_SESSION_BYTES);
        assert_eq!(got.len(), 3);
        assert_eq!(got.last().unwrap().assistant, "3");
        // A single turn larger than the session budget is refused outright
        // and does not disturb stored history.
        let huge = "y".repeat(PUBLIC_HISTORY_MAX_SESSION_BYTES);
        assert!(matches!(
            s.append(&a, &turn(&huge, "z"), t0() + Duration::seconds(11)),
            Err(PublicHistoryError::TurnTooLarge)
        ));
        assert_eq!(s.load(&a, t0() + Duration::seconds(12)).unwrap().len(), 3);
    }

    #[test]
    fn total_cap_refuses_without_evicting_anything() {
        // Tiny global cap: one small turn fits, a second session's does not.
        let probe = store(PublicHistoryConfig::default());
        probe.append(&session("p"), &turn("u", "r"), t0()).unwrap();
        let one: i64 = probe
            .guard()
            .query_row(
                "SELECT LENGTH(ciphertext) FROM telecom_public_history",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let cap = usize::try_from(one).unwrap() + 8;
        let s =
            store(PublicHistoryConfig::with_limits(Duration::hours(1), 32, 65536, cap).unwrap());
        let (a, b) = (session("bind-a"), session("bind-b"));
        s.append(&a, &turn("u", "r"), t0()).unwrap();
        assert!(matches!(
            s.append(&b, &turn("u", "r"), t0()),
            Err(PublicHistoryError::Capacity)
        ));
        assert_eq!(s.load(&a, t0()).unwrap(), vec![turn("u", "r")]);
        assert!(s.load(&b, t0()).unwrap().is_empty());
        // Same-session over-cap append is also refused and keeps old history
        // (the planned eviction is rolled back).
        let cfg = PublicHistoryConfig::with_limits(Duration::hours(1), 1, 65536, cap).unwrap();
        let s2 = store(cfg);
        s2.append(&a, &turn("u", "r"), t0()).unwrap();
        assert!(matches!(
            s2.append(
                &a,
                &turn("uuuuuuuuuuuuuuuuuuuuuuuu", "rrrrrrrrrrrrrrrr"),
                t0()
            ),
            Err(PublicHistoryError::Capacity)
        ));
        assert_eq!(s2.load(&a, t0()).unwrap(), vec![turn("u", "r")]);
        // Expired rows free capacity first.
        assert!(s
            .append(&b, &turn("u", "r"), t0() + Duration::hours(2))
            .is_ok());
    }

    #[test]
    fn bindings_and_callers_are_isolated_but_aliases_share_completed_history() {
        let s = store(PublicHistoryConfig::default());
        let a = session_for_peer("bind-a", "+15550000001");
        let same_binding_other_caller = session_for_peer("bind-a", "+15550000002");
        let other_binding_same_caller = session_for_peer("bind-b", "+15550000001");
        let voice_alias_same_caller = voice_session_for_same_caller("bind-a", "+15550000001");

        s.append(&a, &turn("only-a", "a"), t0()).unwrap();
        assert!(s.load(&same_binding_other_caller, t0()).unwrap().is_empty());
        assert!(s.load(&other_binding_same_caller, t0()).unwrap().is_empty());
        assert_eq!(s.load(&voice_alias_same_caller, t0()).unwrap().len(), 1);
        assert_eq!(a, voice_alias_same_caller);
        assert_ne!(a, same_binding_other_caller);
        assert_ne!(a, other_binding_same_caller);
        assert!(!format!("{a:?}").contains(&a.0));
    }

    #[test]
    fn wrong_key_repeated_reads_retain_ciphertext_and_original_key_restores_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.db");
        let open = |k: &str| {
            PublicHistoryStore::open(&path, SecretsCrypto::new(k), PublicHistoryConfig::default())
                .unwrap()
        };
        let a = session("bind-a");
        let original = vec![turn("secret-q-umber-newt-03", "secret-a"), turn("q2", "a2")];
        let s = open(KEY);
        for (i, turn) in original.iter().enumerate() {
            s.append(&a, turn, t0() + Duration::seconds(i as i64))
                .unwrap();
        }
        let before = encrypted_rows(&s);
        assert_eq!(count(&s), 2);
        drop(s);

        let wrong = open("another-key");
        for seconds in [2, 3] {
            assert!(matches!(
                wrong.load(&a, t0() + Duration::seconds(seconds)),
                Err(PublicHistoryError::Corrupt)
            ));
            assert_eq!(count(&wrong), 2, "wrong key must not reset history");
            assert_eq!(encrypted_rows(&wrong), before);
        }
        drop(wrong);

        let restored = open(KEY);
        assert_eq!(
            restored.load(&a, t0() + Duration::seconds(4)).unwrap(),
            original
        );
        assert_eq!(count(&restored), 2);
        assert_eq!(encrypted_rows(&restored), before);
    }

    #[test]
    fn corrupt_row_repeated_reads_retain_ciphertext_without_partial_turns() {
        let s = store(PublicHistoryConfig::default());
        let a = session("bind-a");
        s.append(&a, &turn("readable-first", "complete-reply"), t0())
            .unwrap();
        s.append(&a, &turn("corrupt-second", "reply"), t0())
            .unwrap();
        // Corrupt the second row so an earlier readable turn cannot leak as
        // a partial result. Keep the corrupt row for explicit operator repair.
        s.guard()
            .execute(
                "UPDATE telecom_public_history SET ciphertext = zeroblob(length(ciphertext))
                 WHERE id = (SELECT MAX(id) FROM telecom_public_history)",
                [],
            )
            .unwrap();
        let before = encrypted_rows(&s);
        for seconds in [1, 2] {
            assert!(matches!(
                s.load(&a, t0() + Duration::seconds(seconds)),
                Err(PublicHistoryError::Corrupt)
            ));
            assert_eq!(count(&s), 2);
            assert_eq!(encrypted_rows(&s), before);
        }
    }

    #[test]
    fn plaintext_payload_is_refused_without_fallback_or_deletion() {
        let s = store(PublicHistoryConfig::default());
        let a = session("bind-a");
        s.append(&a, &turn("q", "a"), t0()).unwrap();
        // A well-formed plaintext payload is not an encrypted row, even if
        // its session and timestamps agree with the database metadata.
        let plaintext = serde_json::to_vec(&Payload {
            version: PAYLOAD_VERSION,
            session: a.0.clone(),
            created_at_ms: t0().timestamp_millis(),
            expires_at_ms: (t0() + PUBLIC_HISTORY_MAX_TTL).timestamp_millis(),
            user: "q".into(),
            assistant: "a".into(),
        })
        .unwrap();
        s.guard()
            .execute(
                "UPDATE telecom_public_history SET ciphertext = ?1",
                params![plaintext],
            )
            .unwrap();
        let before = encrypted_rows(&s);
        for seconds in [1, 2] {
            assert!(matches!(
                s.load(&a, t0() + Duration::seconds(seconds)),
                Err(PublicHistoryError::Corrupt)
            ));
            assert_eq!(count(&s), 1);
            assert_eq!(encrypted_rows(&s), before);
        }
    }

    #[test]
    fn wrong_key_failure_still_prunes_expired_rows_only() {
        let s = store(PublicHistoryConfig::default());
        let a = session("bind-a");
        s.append(&a, &turn("expired", "old-reply"), t0()).unwrap();
        let retained = turn("unexpired", "new-reply");
        s.append(&a, &retained, t0() + Duration::hours(1)).unwrap();
        let before = encrypted_rows(&s);
        assert_eq!(before.len(), 2);
        // Reuse the same database with a mistaken key; expired-row pruning
        // is independent of decryption and must not remove the live row.
        let conn = s.conn.into_inner().unwrap();
        let wrong = PublicHistoryStore::init(
            conn,
            SecretsCrypto::new("another-key"),
            PublicHistoryConfig::default(),
        )
        .unwrap();
        let at = t0() + PUBLIC_HISTORY_MAX_TTL;
        for _ in 0..2 {
            assert!(matches!(
                wrong.load(&a, at),
                Err(PublicHistoryError::Corrupt)
            ));
            assert_eq!(count(&wrong), 1);
            assert_eq!(encrypted_rows(&wrong), before[1..]);
        }
        let restored = PublicHistoryStore::init(
            wrong.conn.into_inner().unwrap(),
            SecretsCrypto::new(KEY),
            PublicHistoryConfig::default(),
        )
        .unwrap();
        assert_eq!(restored.load(&a, at).unwrap(), vec![retained]);
        assert!(restored
            .load(&a, at + Duration::hours(1))
            .unwrap()
            .is_empty());
        assert_eq!(count(&restored), 0);
    }

    #[test]
    fn ciphertext_moved_across_session_or_metadata_is_refused() {
        let s = store(PublicHistoryConfig::default());
        let (a, b) = (session("bind-a"), session("bind-b"));
        s.append(&a, &turn("a-private", "a"), t0()).unwrap();
        // Re-label a's row as b's: AEAD payload still names a.
        s.guard()
            .execute(
                "UPDATE telecom_public_history SET session = ?1",
                params![b.0],
            )
            .unwrap();
        let before = encrypted_rows(&s);
        for _ in 0..2 {
            assert!(matches!(s.load(&b, t0()), Err(PublicHistoryError::Corrupt)));
            assert_eq!(count(&s), 1);
            assert_eq!(encrypted_rows(&s), before);
        }

        // Each metadata mismatch gets an independent fixture: retained corrupt
        // rows from an earlier case must not mask the next validation check.
        for sql in [
            "UPDATE telecom_public_history SET plain_bytes = plain_bytes + 1",
            "UPDATE telecom_public_history SET expires_at = expires_at + 3600000",
        ] {
            let s = store(PublicHistoryConfig::default());
            s.append(&a, &turn("q", "a"), t0()).unwrap();
            s.guard().execute(sql, []).unwrap();
            let before = encrypted_rows(&s);
            for _ in 0..2 {
                assert!(matches!(s.load(&a, t0()), Err(PublicHistoryError::Corrupt)));
                assert_eq!(count(&s), 1);
                assert_eq!(encrypted_rows(&s), before);
            }
        }
    }

    #[test]
    fn missing_key_is_refused_and_debug_is_redacted() {
        assert!(matches!(
            PublicHistoryStore::open_in_memory(
                SecretsCrypto::new(""),
                PublicHistoryConfig::default()
            ),
            Err(PublicHistoryError::MissingKey)
        ));
        let t = turn("canary-user-text", "canary-reply-text");
        assert!(!format!("{t:?}").contains("canary"));
    }
}
