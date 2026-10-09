//! Synchronous, encrypted durable storage for Relay-to-Core inbound SMS.
//!
//! This is a persistence seam only. It does not broadcast, deliver to an Agent,
//! open a Relay connection, or claim exactly-once delivery. SQLite WAL/NORMAL
//! durability and logical deletion do not guarantee physical secure erasure
//! from WAL files or backups.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use aleph_protocol::telecom_relay::{SmsEncoding, SmsInbound, TelecomEnvelope, TelecomKind};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::secrets::crypto::SecretsCrypto;
use crate::secrets::types::SecretError;
use crate::utils::sqlite_open::open_sqlite_safe;

use super::protocol::{accept_event, AcceptContext, Acceptance, DedupLedger, TelecomError};

const MAX_CONTENT_TTL: Duration = Duration::hours(24);
const DEFAULT_DEDUP_HORIZON: Duration = Duration::days(7);

/// Explicit resource limits for one inbound inbox.
#[derive(Debug, Clone, Copy)]
pub struct SmsInboxConfig {
    pub max_entries: usize,
    pub max_bytes: usize,
    pub dedup_horizon: Duration,
}

impl SmsInboxConfig {
    /// Construct a bounded configuration. Limits are required, not guessed by
    /// the store, and zero limits are rejected fail-closed.
    pub fn new(max_entries: usize, max_bytes: usize) -> Result<Self, SmsInboxError> {
        if max_entries == 0 || max_bytes == 0 {
            return Err(SmsInboxError::InvalidConfig(
                "max_entries and max_bytes must be non-zero".into(),
            ));
        }
        Ok(Self {
            max_entries,
            max_bytes,
            dedup_horizon: DEFAULT_DEDUP_HORIZON,
        })
    }

    pub fn with_dedup_horizon(mut self, horizon: Duration) -> Result<Self, SmsInboxError> {
        if horizon <= Duration::zero() {
            return Err(SmsInboxError::InvalidConfig(
                "dedup_horizon must be positive".into(),
            ));
        }
        self.dedup_horizon = horizon;
        Ok(self)
    }
}

/// Reasons intake cannot produce an ACK-eligible local result.
#[derive(Debug, thiserror::Error)]
pub enum SmsInboxError {
    #[error("invalid SMS inbox configuration: {0}")]
    InvalidConfig(String),
    #[error("dedicated SMS inbox key is missing or unusable")]
    MissingKey,
    #[error("only sms_inbound envelopes may enter the SMS inbox")]
    WrongKind,
    #[error("inbound SMS has invalid or future timestamps: {0}")]
    InvalidTimestamp(String),
    #[error("SMS inbox capacity exceeded ({entries} entries, {bytes} bytes)")]
    Capacity { entries: usize, bytes: usize },
    #[error(transparent)]
    Protocol(#[from] TelecomError),
    #[error("encrypted SMS payload is corrupt or metadata does not match its row")]
    CorruptPayload,
    #[error(transparent)]
    Crypto(#[from] SecretError),
    #[error(transparent)]
    Storage(#[from] rusqlite::Error),
}

/// Result of a durable intake attempt. Persisted and Duplicate are the only
/// outcomes that are eligible for a later local ACK; neither is a Relay ACK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmsInboxAdmission {
    Persisted(i64),
    Duplicate(i64),
    Stale,
    Expired,
}

/// Decrypted pending SMS returned by [`SmsInbox::pending`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsInboxItem {
    pub id: i64,
    pub binding_id: String,
    pub provider_account_id: String,
    pub provider_message_id: String,
    pub event_id: String,
    pub connection_generation: u64,
    pub service_address: String,
    pub peer_address: String,
    pub encoding: SmsEncoding,
    pub segment_count: u32,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub body: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedSmsPayload {
    protocol_version: u32,
    binding_id: String,
    provider_account_id: String,
    provider_message_id: String,
    event_id: String,
    connection_generation: u64,
    service_address: String,
    peer_address: String,
    encoding: SmsEncoding,
    segment_count: u32,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    body: String,
}

/// File-backed, mutex-serialized synchronous inbox.
pub struct SmsInbox {
    conn: Mutex<Connection>,
    crypto: SecretsCrypto,
    config: SmsInboxConfig,
}

impl SmsInbox {
    /// Open a durable inbox using a caller-supplied dedicated inbox crypto
    /// engine. The caller must not derive this key from login/shared-token
    /// credentials and must not persist it beside the database.
    pub fn open(
        path: &Path,
        crypto: SecretsCrypto,
        config: SmsInboxConfig,
    ) -> Result<Self, SmsInboxError> {
        if !crypto.is_usable() {
            return Err(SmsInboxError::MissingKey);
        }
        Self::init(open_sqlite_safe(path)?, crypto, config)
    }

    /// Convenience constructor that rejects an absent or empty dedicated key.
    pub fn open_with_key(
        path: &Path,
        key: impl Into<String>,
        config: SmsInboxConfig,
    ) -> Result<Self, SmsInboxError> {
        let key = key.into();
        if key.is_empty() {
            return Err(SmsInboxError::MissingKey);
        }
        Self::open(path, SecretsCrypto::new(key), config)
    }

    /// Ephemeral store intended for unit tests; production callers should use
    /// [`Self::open`] with a file path.
    pub fn open_in_memory(
        crypto: SecretsCrypto,
        config: SmsInboxConfig,
    ) -> Result<Self, SmsInboxError> {
        if !crypto.is_usable() {
            return Err(SmsInboxError::MissingKey);
        }
        Self::init(Connection::open_in_memory()?, crypto, config)
    }

    fn init(
        conn: Connection,
        crypto: SecretsCrypto,
        config: SmsInboxConfig,
    ) -> Result<Self, SmsInboxError> {
        if config.max_entries == 0
            || config.max_bytes == 0
            || config.dedup_horizon <= Duration::zero()
        {
            return Err(SmsInboxError::InvalidConfig(
                "inbox limits and dedup horizon must be positive".into(),
            ));
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sms_inbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                protocol_version INTEGER NOT NULL,
                binding_id TEXT NOT NULL,
                provider_account_id TEXT NOT NULL,
                provider_message_id TEXT NOT NULL,
                event_id TEXT NOT NULL,
                connection_generation INTEGER NOT NULL,
                service_address TEXT NOT NULL,
                peer_address TEXT NOT NULL,
                encoding TEXT NOT NULL,
                segment_count INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                content_bytes INTEGER NOT NULL DEFAULT 0,
                ciphertext BLOB,
                nonce BLOB,
                salt BLOB,
                consumed_at INTEGER,
                UNIQUE(protocol_version, binding_id, provider_account_id, provider_message_id)
            );
            CREATE INDEX IF NOT EXISTS idx_sms_inbox_pending
                ON sms_inbox(created_at, id);",
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

    /// Atomically admit one valid inbound SMS and its durable dedup identity.
    pub fn accept(
        &self,
        context: &AcceptContext<'_>,
        envelope: &TelecomEnvelope,
    ) -> Result<SmsInboxAdmission, SmsInboxError> {
        let inbound = match &envelope.kind {
            TelecomKind::SmsInbound(inbound) => inbound,
            _ => return Err(SmsInboxError::WrongKind),
        };
        validate_inbound_shape(context.now, envelope, inbound)?;

        // Run the existing shared validator. The temporary ledger is only a
        // vehicle for its version, binding, generation, stale and expiry
        // semantics; durable dedup is decided in the SQLite transaction below.
        let mut validator_ledger = DedupLedger::new(self.config.dedup_horizon);
        let acceptance = accept_event(&mut validator_ledger, context, envelope)?;
        let effective_expires_at = envelope.expires_at.min(
            envelope
                .created_at
                .checked_add_signed(MAX_CONTENT_TTL)
                .ok_or_else(|| {
                    SmsInboxError::InvalidTimestamp("created_at plus content TTL overflowed".into())
                })?,
        );
        let acceptance = match acceptance {
            Acceptance::Fresh(key) if context.now >= effective_expires_at => {
                Acceptance::Expired(key)
            }
            acceptance => acceptance,
        };

        let mut conn = self.guard();
        let tx = conn.transaction()?;
        prune_transaction(&tx, context.now, self.config.dedup_horizon)?;

        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM sms_inbox
                 WHERE protocol_version = ?1 AND binding_id = ?2
                   AND provider_account_id = ?3 AND provider_message_id = ?4",
                params![
                    envelope.protocol_version,
                    envelope.binding_id,
                    inbound.provider_account_id,
                    inbound.provider_message_id
                ],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            tx.commit()?;
            return Ok(SmsInboxAdmission::Duplicate(id));
        }

        match acceptance {
            Acceptance::Stale(_) => {
                tx.commit()?;
                return Ok(SmsInboxAdmission::Stale);
            }
            Acceptance::Expired(_) => {
                insert_tombstone(&tx, envelope, inbound, effective_expires_at)?;
                tx.commit()?;
                return Ok(SmsInboxAdmission::Expired);
            }
            Acceptance::Fresh(_) => {}
            Acceptance::Duplicate(_) => unreachable!("temporary ledger starts empty"),
        }

        let stored_created_at = millis_to_datetime(envelope.created_at.timestamp_millis())?;
        let stored_expires_at = millis_to_datetime(effective_expires_at.timestamp_millis())?;
        let payload = EncryptedSmsPayload::from_envelope(
            envelope,
            inbound,
            stored_created_at,
            stored_expires_at,
        );
        let payload_json = serde_json::to_string(&payload)
            .map_err(|err| SmsInboxError::InvalidConfig(format!("payload serialization: {err}")))?;
        let encrypted = self.crypto.encrypt(&payload_json)?;

        let (entry_count, byte_count): (i64, i64) = tx.query_row(
            "SELECT COUNT(*), COALESCE(SUM(content_bytes), 0) FROM sms_inbox
             WHERE ciphertext IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let entry_count =
            usize::try_from(entry_count).map_err(|_| SmsInboxError::CorruptPayload)?;
        let byte_count = usize::try_from(byte_count).map_err(|_| SmsInboxError::CorruptPayload)?;
        if entry_count >= self.config.max_entries
            || encrypted.ciphertext.len() > self.config.max_bytes.saturating_sub(byte_count)
        {
            return Err(SmsInboxError::Capacity {
                entries: entry_count,
                bytes: byte_count,
            });
        }

        tx.execute(
            "INSERT INTO sms_inbox (
                protocol_version, binding_id, provider_account_id, provider_message_id,
                event_id, connection_generation, service_address, peer_address,
                encoding, segment_count, created_at, expires_at, content_bytes,
                ciphertext, nonce, salt
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                envelope.protocol_version,
                envelope.binding_id,
                inbound.provider_account_id,
                inbound.provider_message_id,
                envelope.event_id,
                envelope.connection_generation,
                inbound.service_address,
                inbound.peer_address,
                serde_json::to_string(&inbound.encoding)
                    .map_err(|err| SmsInboxError::InvalidConfig(err.to_string()))?,
                inbound.segment_count,
                envelope.created_at.timestamp_millis(),
                effective_expires_at.timestamp_millis(),
                i64::try_from(encrypted.ciphertext.len()).map_err(|_| SmsInboxError::Capacity {
                    entries: entry_count,
                    bytes: byte_count,
                })?,
                encrypted.ciphertext,
                encrypted.nonce.as_slice(),
                encrypted.salt.as_slice(),
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(SmsInboxAdmission::Persisted(id))
    }

    /// Retrieve pending content without consuming it. Expired content is
    /// logically deleted while its dedup metadata remains within the horizon.
    pub fn pending(&self, now: DateTime<Utc>) -> Result<Vec<SmsInboxItem>, SmsInboxError> {
        let mut conn = self.guard();
        let tx = conn.transaction()?;
        prune_transaction(&tx, now, self.config.dedup_horizon)?;
        tx.commit()?;

        let mut stmt = conn.prepare(
            "SELECT id, protocol_version, binding_id, provider_account_id,
                    provider_message_id, event_id, connection_generation,
                    service_address, peer_address, encoding, segment_count,
                    created_at, expires_at, ciphertext, nonce, salt
             FROM sms_inbox WHERE ciphertext IS NOT NULL ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredRow {
                id: row.get(0)?,
                protocol_version: row.get(1)?,
                binding_id: row.get(2)?,
                provider_account_id: row.get(3)?,
                provider_message_id: row.get(4)?,
                event_id: row.get(5)?,
                connection_generation: row.get(6)?,
                service_address: row.get(7)?,
                peer_address: row.get(8)?,
                encoding: row.get(9)?,
                segment_count: row.get(10)?,
                created_at: row.get(11)?,
                expires_at: row.get(12)?,
                ciphertext: row.get(13)?,
                nonce: row.get(14)?,
                salt: row.get(15)?,
            })
        })?;
        let stored: Vec<StoredRow> = rows.collect::<rusqlite::Result<_>>()?;
        stored.iter().map(|row| self.decrypt_row(row)).collect()
    }

    /// Confirmed consumption only. Broadcast/send success is not connected to
    /// this method and therefore cannot silently remove durable content.
    pub fn consume(&self, id: i64, now: DateTime<Utc>) -> Result<bool, SmsInboxError> {
        let mut conn = self.guard();
        let tx = conn.transaction()?;
        prune_transaction(&tx, now, self.config.dedup_horizon)?;
        let changed = tx.execute(
            "UPDATE sms_inbox
             SET ciphertext = NULL, nonce = NULL, salt = NULL,
                 content_bytes = 0, consumed_at = ?1
             WHERE id = ?2 AND ciphertext IS NOT NULL",
            params![now.timestamp_millis(), id],
        )?;
        tx.commit()?;
        Ok(changed == 1)
    }

    pub fn pending_count(&self, now: DateTime<Utc>) -> Result<usize, SmsInboxError> {
        let mut conn = self.guard();
        let tx = conn.transaction()?;
        prune_transaction(&tx, now, self.config.dedup_horizon)?;
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM sms_inbox WHERE ciphertext IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        tx.commit()?;
        usize::try_from(count).map_err(|_| SmsInboxError::CorruptPayload)
    }

    fn decrypt_row(&self, row: &StoredRow) -> Result<SmsInboxItem, SmsInboxError> {
        let ciphertext = row
            .ciphertext
            .as_deref()
            .ok_or(SmsInboxError::CorruptPayload)?;
        let nonce_slice = row.nonce.as_deref().ok_or(SmsInboxError::CorruptPayload)?;
        let salt_slice = row.salt.as_deref().ok_or(SmsInboxError::CorruptPayload)?;
        let nonce: [u8; 12] = nonce_slice
            .try_into()
            .map_err(|_| SmsInboxError::CorruptPayload)?;
        let salt: [u8; 32] = salt_slice
            .try_into()
            .map_err(|_| SmsInboxError::CorruptPayload)?;
        let plaintext = self.crypto.decrypt(ciphertext, &nonce, &salt)?;
        let payload: EncryptedSmsPayload =
            serde_json::from_str(&plaintext).map_err(|_| SmsInboxError::CorruptPayload)?;
        let created_at = millis_to_datetime(row.created_at)?;
        let expires_at = millis_to_datetime(row.expires_at)?;
        if payload.protocol_version != row.protocol_version
            || payload.binding_id != row.binding_id
            || payload.provider_account_id != row.provider_account_id
            || payload.provider_message_id != row.provider_message_id
            || payload.event_id != row.event_id
            || payload.connection_generation != row.connection_generation
            || payload.service_address != row.service_address
            || payload.peer_address != row.peer_address
            || serde_json::to_string(&payload.encoding)
                .map_err(|_| SmsInboxError::CorruptPayload)?
                != row.encoding
            || payload.segment_count != row.segment_count
            || payload.created_at != created_at
            || payload.expires_at != expires_at
        {
            return Err(SmsInboxError::CorruptPayload);
        }
        Ok(SmsInboxItem {
            id: row.id,
            binding_id: row.binding_id.clone(),
            provider_account_id: row.provider_account_id.clone(),
            provider_message_id: row.provider_message_id.clone(),
            event_id: row.event_id.clone(),
            connection_generation: row.connection_generation,
            service_address: row.service_address.clone(),
            peer_address: row.peer_address.clone(),
            encoding: payload.encoding,
            segment_count: row.segment_count,
            created_at,
            expires_at,
            body: payload.body,
        })
    }
}

#[derive(Debug)]
struct StoredRow {
    id: i64,
    protocol_version: u32,
    binding_id: String,
    provider_account_id: String,
    provider_message_id: String,
    event_id: String,
    connection_generation: u64,
    service_address: String,
    peer_address: String,
    encoding: String,
    segment_count: u32,
    created_at: i64,
    expires_at: i64,
    ciphertext: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
    salt: Option<Vec<u8>>,
}

impl EncryptedSmsPayload {
    fn from_envelope(
        envelope: &TelecomEnvelope,
        inbound: &SmsInbound,
        created_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            protocol_version: envelope.protocol_version,
            binding_id: envelope.binding_id.clone(),
            provider_account_id: inbound.provider_account_id.clone(),
            provider_message_id: inbound.provider_message_id.clone(),
            event_id: envelope.event_id.clone(),
            connection_generation: envelope.connection_generation,
            service_address: inbound.service_address.clone(),
            peer_address: inbound.peer_address.clone(),
            encoding: inbound.encoding,
            segment_count: inbound.segment_count,
            created_at,
            expires_at,
            body: inbound.body.clone(),
        }
    }
}

fn validate_inbound_shape(
    now: DateTime<Utc>,
    envelope: &TelecomEnvelope,
    inbound: &SmsInbound,
) -> Result<(), SmsInboxError> {
    if envelope.created_at > now {
        return Err(SmsInboxError::InvalidTimestamp(
            "created_at is in the future".into(),
        ));
    }
    if envelope.expires_at <= envelope.created_at {
        return Err(SmsInboxError::InvalidTimestamp(
            "expires_at must be after created_at".into(),
        ));
    }
    if inbound.provider_account_id.is_empty() || inbound.provider_message_id.is_empty() {
        return Err(SmsInboxError::InvalidTimestamp(
            "provider account and message ids must be non-empty".into(),
        ));
    }
    Ok(())
}

fn insert_tombstone(
    tx: &Transaction<'_>,
    envelope: &TelecomEnvelope,
    inbound: &SmsInbound,
    effective_expires_at: DateTime<Utc>,
) -> Result<(), SmsInboxError> {
    let encoding = serde_json::to_string(&inbound.encoding)
        .map_err(|err| SmsInboxError::InvalidConfig(err.to_string()))?;
    tx.execute(
        "INSERT INTO sms_inbox (
            protocol_version, binding_id, provider_account_id, provider_message_id,
            event_id, connection_generation, service_address, peer_address,
            encoding, segment_count, created_at, expires_at, content_bytes,
            ciphertext, nonce, salt
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0,
                   NULL, NULL, NULL)",
        params![
            envelope.protocol_version,
            envelope.binding_id,
            inbound.provider_account_id,
            inbound.provider_message_id,
            envelope.event_id,
            envelope.connection_generation,
            inbound.service_address,
            inbound.peer_address,
            encoding,
            inbound.segment_count,
            envelope.created_at.timestamp_millis(),
            effective_expires_at.timestamp_millis(),
        ],
    )?;
    Ok(())
}

fn prune_transaction(
    tx: &Transaction<'_>,
    now: DateTime<Utc>,
    horizon: Duration,
) -> Result<(), SmsInboxError> {
    let now_ms = now.timestamp_millis();
    tx.execute(
        "UPDATE sms_inbox
         SET ciphertext = NULL, nonce = NULL, salt = NULL, content_bytes = 0
         WHERE ciphertext IS NOT NULL AND expires_at <= ?1",
        params![now_ms],
    )?;
    let cutoff = now.checked_sub_signed(horizon).ok_or_else(|| {
        SmsInboxError::InvalidConfig("dedup horizon overflows timestamp range".into())
    })?;
    tx.execute(
        "DELETE FROM sms_inbox WHERE created_at < ?1",
        params![cutoff.timestamp_millis()],
    )?;
    Ok(())
}

fn millis_to_datetime(ms: i64) -> Result<DateTime<Utc>, SmsInboxError> {
    DateTime::<Utc>::from_timestamp_millis(ms).ok_or(SmsInboxError::CorruptPayload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::crypto::SecretsCrypto;
    use aleph_protocol::telecom_relay::{
        CallTurn, CallTurnRole, OutcomeReport, PlaybackEvent, PlaybackState, PlayedExtent,
        SmsInbound, TelecomOutcome, TranscriptState, TELECOM_PROTOCOL_VERSION,
    };
    use chrono::TimeZone;

    fn envelope(
        account: &str,
        message_id: &str,
        body: &str,
        now: DateTime<Utc>,
    ) -> TelecomEnvelope {
        TelecomEnvelope {
            protocol_version: TELECOM_PROTOCOL_VERSION,
            binding_id: "binding-a".into(),
            connection_generation: 7,
            event_id: format!("event-{message_id}"),
            kind: TelecomKind::SmsInbound(SmsInbound {
                provider_account_id: account.into(),
                provider_message_id: message_id.into(),
                service_address: "+15550001".into(),
                peer_address: "+15550002".into(),
                body: body.into(),
                encoding: SmsEncoding::Gsm7,
                segment_count: 1,
            }),
            created_at: now,
            expires_at: now + Duration::hours(1),
        }
    }

    fn inbox(max_entries: usize, max_bytes: usize) -> SmsInbox {
        SmsInbox::open_in_memory(
            SecretsCrypto::new("dedicated-inbox-key"),
            SmsInboxConfig::new(max_entries, max_bytes).unwrap(),
        )
        .unwrap()
    }

    fn context(now: DateTime<Utc>) -> AcceptContext<'static> {
        AcceptContext {
            authenticated_binding: "binding-a",
            active_generation: 7,
            now,
        }
    }

    #[test]
    fn real_inbox_persists_an_encrypted_pending_sms() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let inbox = inbox(4, 1024);
        let env = envelope("account-a", "message-1", "durable secret body", now);
        let result = inbox.accept(&context(now), &env).unwrap();
        assert!(matches!(result, SmsInboxAdmission::Persisted(_)));
        let pending = inbox.pending(now).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].body, "durable secret body");
        assert_eq!(pending[0].provider_message_id, "message-1");
    }

    #[test]
    fn database_stores_ciphertext_not_plaintext_and_config_rejects_missing_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sms-inbox.db");
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let body = "body-must-not-appear-in-sqlite";
        let config = SmsInboxConfig::new(4, 4096).unwrap();
        assert!(matches!(
            SmsInbox::open_with_key(&path, "", config),
            Err(SmsInboxError::MissingKey)
        ));
        assert!(matches!(
            SmsInbox::open_in_memory(SecretsCrypto::new(""), config),
            Err(SmsInboxError::MissingKey)
        ));
        assert!(matches!(
            SmsInboxConfig::new(0, 4096),
            Err(SmsInboxError::InvalidConfig(_))
        ));
        let store = SmsInbox::open_with_key(&path, "dedicated-inbox-key", config).unwrap();
        store
            .accept(
                &context(now),
                &envelope("account-a", "ciphertext", body, now),
            )
            .unwrap();
        drop(store);
        let raw = crate::utils::sqlite_open::open_sqlite_safe(&path).unwrap();
        let ciphertext: Vec<u8> = raw
            .query_row("SELECT ciphertext FROM sms_inbox", [], |row| row.get(0))
            .unwrap();
        assert_ne!(ciphertext, body.as_bytes());
        let db_bytes = std::fs::read(&path).unwrap();
        assert!(!db_bytes
            .windows(body.len())
            .any(|window| window == body.as_bytes()));
    }

    #[test]
    fn duplicate_and_identical_bodies_use_structured_identity() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let inbox = inbox(4, 4096);
        let first = envelope("account-a", "message-1", "same", now);
        let mut retry = first.clone();
        retry.event_id = "retry-frame".into();
        let second = envelope("account-a", "message-2", "same", now);
        let cross_account = envelope("account-b", "message-1", "same", now);
        assert!(matches!(
            inbox.accept(&context(now), &first),
            Ok(SmsInboxAdmission::Persisted(_))
        ));
        let first_id = match inbox.accept(&context(now), &retry).unwrap() {
            SmsInboxAdmission::Duplicate(id) => id,
            other => panic!("expected duplicate, got {other:?}"),
        };
        assert!(first_id > 0);
        assert!(matches!(
            inbox.accept(&context(now), &second),
            Ok(SmsInboxAdmission::Persisted(_))
        ));
        assert!(matches!(
            inbox.accept(&context(now), &cross_account),
            Ok(SmsInboxAdmission::Persisted(_))
        ));
        assert_eq!(inbox.pending(now).unwrap().len(), 3);
    }

    #[test]
    fn pending_survives_file_reopen_and_explicit_consume_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sms-inbox.db");
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let env = envelope("account-a", "message-1", "restart body", now);
        let id = {
            let store = SmsInbox::open_with_key(
                &path,
                "dedicated-inbox-key",
                SmsInboxConfig::new(4, 4096).unwrap(),
            )
            .unwrap();
            match store.accept(&context(now), &env).unwrap() {
                SmsInboxAdmission::Persisted(id) => id,
                other => panic!("{other:?}"),
            }
        };
        let reopened = SmsInbox::open_with_key(
            &path,
            "dedicated-inbox-key",
            SmsInboxConfig::new(4, 4096).unwrap(),
        )
        .unwrap();
        assert_eq!(reopened.pending(now).unwrap()[0].body, "restart body");
        assert!(!reopened.consume(id + 1, now).unwrap());
        assert!(reopened.consume(id, now).unwrap());
        assert!(reopened.pending(now).unwrap().is_empty());
        assert!(matches!(
            reopened.accept(&context(now), &env),
            Ok(SmsInboxAdmission::Duplicate(_))
        ));
    }

    #[test]
    fn wrong_key_and_tampering_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sms-inbox.db");
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let env = envelope("account-a", "message-1", "secret", now);
        let store =
            SmsInbox::open_with_key(&path, "correct", SmsInboxConfig::new(4, 4096).unwrap())
                .unwrap();
        store.accept(&context(now), &env).unwrap();
        drop(store);
        let wrong =
            SmsInbox::open_with_key(&path, "wrong", SmsInboxConfig::new(4, 4096).unwrap()).unwrap();
        assert!(matches!(wrong.pending(now), Err(SmsInboxError::Crypto(_))));
        drop(wrong);
        let raw = crate::utils::sqlite_open::open_sqlite_safe(&path).unwrap();
        raw.execute(
            "UPDATE sms_inbox SET ciphertext = zeroblob(length(ciphertext))",
            [],
        )
        .unwrap();
        drop(raw);
        let tampered =
            SmsInbox::open_with_key(&path, "correct", SmsInboxConfig::new(4, 4096).unwrap())
                .unwrap();
        assert!(tampered.pending(now).is_err());
    }

    #[test]
    fn invalid_direction_binding_generation_stale_expiry_and_future_are_rejected() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let store = inbox(10, 8192);
        let mut command = envelope("account-a", "command", "x", now);
        command.kind = TelecomKind::SmsCommand(aleph_protocol::telecom_relay::SmsCommand {
            operation_id: "op".into(),
            approval_reference: None,
            recipient_binding_id: "binding-a".into(),
            expires_at: now + Duration::hours(1),
            body: "x".into(),
            encoding: SmsEncoding::Gsm7,
            segment_count: 1,
        });
        assert!(matches!(
            store.accept(&context(now), &command),
            Err(SmsInboxError::WrongKind)
        ));
        let mut call = envelope("account-a", "call", "x", now);
        call.kind = TelecomKind::CallTurn(CallTurn {
            provider_account_id: "account-a".into(),
            phone_call_id: "call".into(),
            provider_call_id: "provider-call".into(),
            turn_id: "turn".into(),
            role: CallTurnRole::UserTranscript,
            transcript_state: TranscriptState::Committed,
            output_epoch: 1,
            output_seq: 1,
            text: "x".into(),
        });
        assert!(matches!(
            store.accept(&context(now), &call),
            Err(SmsInboxError::WrongKind)
        ));
        let mut playback = envelope("account-a", "playback", "x", now);
        playback.kind = TelecomKind::PlaybackEvent(PlaybackEvent {
            provider_account_id: "account-a".into(),
            phone_call_id: "call".into(),
            turn_id: "turn".into(),
            output_epoch: 1,
            output_seq: 1,
            state: PlaybackState::Played,
            extent: PlayedExtent::Exact { chars: 1 },
        });
        assert!(matches!(
            store.accept(&context(now), &playback),
            Err(SmsInboxError::WrongKind)
        ));
        let mut receipt = envelope("account-a", "receipt", "x", now);
        receipt.kind = TelecomKind::Outcome(OutcomeReport {
            provider_account_id: "account-a".into(),
            provider_event_id: "provider-event".into(),
            operation_id: "operation".into(),
            provider_message_id: None,
            provider_call_id: None,
            outcome: TelecomOutcome::Delivered,
            error_code: None,
        });
        assert!(matches!(
            store.accept(&context(now), &receipt),
            Err(SmsInboxError::WrongKind)
        ));
        let mut bad_version = envelope("account-a", "version", "x", now);
        bad_version.protocol_version = TELECOM_PROTOCOL_VERSION + 1;
        assert!(store.accept(&context(now), &bad_version).is_err());
        let mut wrong_binding = envelope("account-a", "binding", "x", now);
        wrong_binding.binding_id = "other".into();
        assert!(store.accept(&context(now), &wrong_binding).is_err());
        let mut stale = envelope("account-a", "stale", "x", now);
        stale.connection_generation = 6;
        assert!(matches!(
            store.accept(&context(now), &stale),
            Err(SmsInboxError::Protocol(_))
        ));
        let mut future = envelope("account-a", "future", "x", now);
        future.connection_generation = 8;
        assert!(matches!(
            store.accept(&context(now), &future),
            Err(SmsInboxError::Protocol(_))
        ));
        let old = envelope("account-a", "old", "x", now - Duration::days(8));
        assert!(matches!(
            store.accept(&context(now), &old),
            Ok(SmsInboxAdmission::Stale)
        ));
        let expired = envelope("account-a", "expired", "x", now - Duration::hours(2));
        assert!(matches!(
            store.accept(&context(now), &expired),
            Ok(SmsInboxAdmission::Expired)
        ));
        let mut future_time = envelope("account-a", "future-time", "x", now);
        future_time.created_at = now + Duration::seconds(1);
        assert!(matches!(
            store.accept(&context(now), &future_time),
            Err(SmsInboxError::InvalidTimestamp(_))
        ));
        assert_eq!(store.pending_count(now).unwrap(), 0);
    }

    #[test]
    fn expired_events_leave_tombstones_and_redeliver_as_duplicates() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let store = inbox(4, 4096);
        let mut expired = envelope(
            "account-a",
            "expired-tombstone",
            "original body",
            now - Duration::hours(2),
        );
        expired.expires_at = now - Duration::hours(1);
        assert!(matches!(
            store.accept(&context(now), &expired),
            Ok(SmsInboxAdmission::Expired)
        ));
        assert_eq!(store.pending_count(now).unwrap(), 0);

        let mut redelivery = expired.clone();
        redelivery.event_id = "different-event".into();
        if let TelecomKind::SmsInbound(inbound) = &mut redelivery.kind {
            inbound.body = "different body".into();
        }
        redelivery.expires_at = now + Duration::hours(1);
        assert!(matches!(
            store.accept(&context(now), &redelivery),
            Ok(SmsInboxAdmission::Duplicate(_))
        ));

        let mut wrong_identity = redelivery.clone();
        wrong_identity.binding_id = "wrong-binding".into();
        assert!(matches!(
            store.accept(&context(now), &wrong_identity),
            Err(SmsInboxError::Protocol(
                TelecomError::BindingMismatch { .. }
            ))
        ));

        let raw = store.conn.lock().unwrap();
        let (rows, content_bytes, ciphertext, nonce, salt): (
            i64,
            i64,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
        ) = raw
            .query_row(
                "SELECT COUNT(*), content_bytes, ciphertext, nonce, salt FROM sms_inbox",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(content_bytes, 0);
        assert!(ciphertext.is_none() && nonce.is_none() && salt.is_none());
    }

    #[test]
    fn expired_tombstone_insert_rolls_back_on_storage_failure() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let store = inbox(4, 4096);
        let conn = store.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TRIGGER sms_inbox_fail_expired AFTER INSERT ON sms_inbox
             BEGIN SELECT RAISE(ABORT, 'test expired insertion failure'); END;",
        )
        .unwrap();
        drop(conn);

        let mut expired = envelope("account-a", "expired-rollback", "body", now);
        expired.expires_at = now - Duration::seconds(1);
        assert!(store.accept(&context(now), &expired).is_err());
        let raw_count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sms_inbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(raw_count, 0);
    }

    #[test]
    fn content_ttl_expires_old_event_without_pending_content() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let store = inbox(4, 4096);
        let mut old = envelope(
            "account-a",
            "content-ttl",
            "old body",
            now - Duration::hours(25),
        );
        old.expires_at = now + Duration::hours(1);
        assert!(matches!(
            store.accept(&context(now), &old),
            Ok(SmsInboxAdmission::Expired)
        ));
        assert_eq!(store.pending_count(now).unwrap(), 0);

        let mut redelivery = old.clone();
        redelivery.event_id = "ttl-retry".into();
        redelivery.expires_at = now + Duration::hours(2);
        assert!(matches!(
            store.accept(&context(now), &redelivery),
            Ok(SmsInboxAdmission::Duplicate(_))
        ));
    }

    #[test]
    fn dedup_horizon_keeps_exact_boundary_and_prunes_afterward() {
        let first_seen = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let horizon = Duration::hours(1);
        let config = SmsInboxConfig::new(4, 4096)
            .unwrap()
            .with_dedup_horizon(horizon)
            .unwrap();
        let store =
            SmsInbox::open_in_memory(SecretsCrypto::new("dedicated-inbox-key"), config).unwrap();
        let mut env = envelope("account-a", "horizon", "body", first_seen);
        env.expires_at = first_seen + Duration::hours(2);
        assert!(matches!(
            store.accept(&context(first_seen), &env),
            Ok(SmsInboxAdmission::Persisted(_))
        ));

        let exact = first_seen + horizon;
        assert!(matches!(
            store.accept(&context(exact), &env),
            Ok(SmsInboxAdmission::Duplicate(_))
        ));
        let after = exact + Duration::milliseconds(1);
        assert!(matches!(
            store.accept(&context(after), &env),
            Ok(SmsInboxAdmission::Stale)
        ));
        let raw_count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sms_inbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(raw_count, 0);
    }

    #[test]
    fn ttl_prune_capacity_and_transaction_rollback_are_fail_closed() {
        let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let store = inbox(1, 1024);
        let first = envelope("account-a", "one", "first", now);
        let second = envelope("account-a", "two", "second", now);
        store.accept(&context(now), &first).unwrap();
        assert!(matches!(
            store.accept(&context(now), &second),
            Err(SmsInboxError::Capacity { .. })
        ));
        assert_eq!(store.pending_count(now).unwrap(), 1);
        assert!(store.consume(1, now).unwrap());
        assert!(matches!(
            store.accept(&context(now), &second),
            Ok(SmsInboxAdmission::Persisted(_))
        ));
        assert_eq!(store.pending(now + Duration::hours(2)).unwrap().len(), 0);

        let rollback = inbox(4, 4096);
        let conn = rollback.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TRIGGER sms_inbox_fail AFTER INSERT ON sms_inbox
             BEGIN SELECT RAISE(ABORT, 'test insertion failure'); END;",
        )
        .unwrap();
        drop(conn);
        let third = envelope("account-a", "rollback", "rollback body", now);
        assert!(rollback.accept(&context(now), &third).is_err());
        let raw_count: i64 = rollback
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sms_inbox", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            raw_count, 0,
            "dedup and ciphertext insertion must roll back together"
        );
    }
}
