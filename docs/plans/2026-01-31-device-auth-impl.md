# Device Authentication Protocol Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement secure device authentication with Ed25519 signatures, HMAC-signed tokens, and 8-character Base32 pairing codes.

**Architecture:** Unified SecurityManager coordinating TokenManager, PairingManager, and DeviceRegistry, all backed by SQLite storage.

**Tech Stack:** Rust, ed25519-dalek, hmac, sha2, lru, base32, SQLite

---

## Task 1: Add Dependencies

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add crypto dependencies**

Add these to `[dependencies]` section (after line 114, `hex = "0.4"`):

```toml
# Ed25519 signatures for device authentication
ed25519-dalek = { version = "2.1", features = ["rand_core"] }
# Base32 encoding for pairing codes
base32 = "0.5"
```

Note: `hmac`, `sha2`, and `lru` are already present in the file.

**Step 2: Verify compilation**

Run: `cargo build -p aethecore --features gateway`
Expected: Compiles successfully

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "deps: add ed25519-dalek and base32 for device auth"
```

---

## Task 2: Create crypto.rs - Cryptographic Utilities

**Files:**
- Create: `core/src/gateway/security/crypto.rs`
- Test: inline `#[cfg(test)]` module

**Step 1: Write tests first**

```rust
// core/src/gateway/security/crypto.rs

//! Cryptographic utilities for device authentication.
//!
//! Provides Ed25519 key generation/verification and HMAC-SHA256 token signing.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

/// Errors from cryptographic operations
#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("Invalid public key: {0}")]
    InvalidPublicKey(String),
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("HMAC verification failed")]
    HmacVerificationFailed,
}

/// Device fingerprint - first 16 hex characters of SHA256(public_key)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceFingerprint(pub String);

impl DeviceFingerprint {
    /// Create fingerprint from public key bytes
    pub fn from_public_key(public_key: &[u8]) -> Self {
        use sha2::Digest;
        let hash = Sha256::digest(public_key);
        let hex = hex::encode(hash);
        Self(hex[..16].to_string())
    }
}

impl std::fmt::Display for DeviceFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Generate a new Ed25519 keypair
///
/// Returns (signing_key_bytes, verifying_key_bytes)
pub fn generate_keypair() -> ([u8; 32], [u8; 32]) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    (signing_key.to_bytes(), verifying_key.to_bytes())
}

/// Sign a message with Ed25519
pub fn sign_message(signing_key_bytes: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let signing_key = SigningKey::from_bytes(signing_key_bytes);
    let signature = signing_key.sign(message);
    signature.to_bytes()
}

/// Verify an Ed25519 signature
pub fn verify_signature(
    public_key_bytes: &[u8],
    message: &[u8],
    signature_bytes: &[u8],
) -> Result<(), CryptoError> {
    let public_key: [u8; 32] = public_key_bytes
        .try_into()
        .map_err(|_| CryptoError::InvalidPublicKey("Invalid length".into()))?;

    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .map_err(|e| CryptoError::InvalidPublicKey(e.to_string()))?;

    let signature: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| CryptoError::InvalidSignature)?;

    let signature = Signature::from_bytes(&signature);

    verifying_key
        .verify(message, &signature)
        .map_err(|_| CryptoError::InvalidSignature)
}

/// Sign a token with HMAC-SHA256
pub fn hmac_sign(secret: &[u8], token: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(token.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Verify an HMAC-SHA256 signature
pub fn hmac_verify(secret: &[u8], token: &str, signature: &str) -> Result<(), CryptoError> {
    let expected = hmac_sign(secret, token);
    // Use constant-time comparison
    if subtle::ConstantTimeEq::ct_eq(expected.as_bytes(), signature.as_bytes()).into() {
        Ok(())
    } else {
        Err(CryptoError::HmacVerificationFailed)
    }
}

/// Generate a random 32-byte secret
pub fn generate_secret() -> [u8; 32] {
    let mut secret = [0u8; 32];
    use rand::RngCore;
    OsRng.fill_bytes(&mut secret);
    secret
}

/// Pairing code constants
pub const PAIRING_CODE_LENGTH: usize = 8;
pub const PAIRING_CODE_CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generate an 8-character Base32 pairing code (excluding confusing chars)
pub fn generate_pairing_code() -> String {
    use rand::Rng;
    let mut rng = OsRng;
    (0..PAIRING_CODE_LENGTH)
        .map(|_| {
            let idx = rng.gen_range(0..PAIRING_CODE_CHARSET.len());
            PAIRING_CODE_CHARSET[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_generation() {
        let (signing, verifying) = generate_keypair();
        assert_eq!(signing.len(), 32);
        assert_eq!(verifying.len(), 32);
    }

    #[test]
    fn test_sign_and_verify() {
        let (signing, verifying) = generate_keypair();
        let message = b"hello world";
        let signature = sign_message(&signing, message);

        assert!(verify_signature(&verifying, message, &signature).is_ok());
    }

    #[test]
    fn test_verify_wrong_message() {
        let (signing, verifying) = generate_keypair();
        let signature = sign_message(&signing, b"hello");

        assert!(verify_signature(&verifying, b"world", &signature).is_err());
    }

    #[test]
    fn test_fingerprint() {
        let (_, verifying) = generate_keypair();
        let fp = DeviceFingerprint::from_public_key(&verifying);
        assert_eq!(fp.0.len(), 16);
        // Should be hex characters
        assert!(fp.0.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hmac_sign_verify() {
        let secret = generate_secret();
        let token = "test-token-123";
        let signature = hmac_sign(&secret, token);

        assert!(hmac_verify(&secret, token, &signature).is_ok());
        assert!(hmac_verify(&secret, token, "wrong").is_err());
        assert!(hmac_verify(&secret, "wrong-token", &signature).is_err());
    }

    #[test]
    fn test_pairing_code_format() {
        for _ in 0..100 {
            let code = generate_pairing_code();
            assert_eq!(code.len(), 8);
            // Should only contain allowed characters
            assert!(code.chars().all(|c| {
                PAIRING_CODE_CHARSET.contains(&(c as u8))
            }));
            // Should not contain confusing characters
            assert!(!code.contains('0'));
            assert!(!code.contains('1'));
            assert!(!code.contains('I'));
            assert!(!code.contains('O'));
        }
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p aethecore crypto --features gateway`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/gateway/security/crypto.rs
git commit -m "feat(security): add crypto utilities for Ed25519 and HMAC"
```

---

## Task 3: Create store.rs - SecurityStore SQLite Layer

**Files:**
- Create: `core/src/gateway/security/store.rs`

**Step 1: Write SecurityStore with schema migration**

```rust
// core/src/gateway/security/store.rs

//! Unified SQLite storage for security data.
//!
//! Manages devices, tokens, pairing requests, and approved senders.

use rusqlite::{params, Connection, Result as SqliteResult};
use std::path::Path;
use std::sync::Mutex;
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
        let conn = self.conn.lock().unwrap();
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
    }

    /// Set schema version
    fn set_schema_version(&self, version: i32) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
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

            let conn = self.conn.lock().unwrap();

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
    pub fn upsert_device(
        &self,
        device_id: &str,
        device_name: &str,
        device_type: Option<&str>,
        public_key: &[u8],
        fingerprint: &str,
        role: &str,
        scopes: &[String],
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let scopes_json = serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_string());

        conn.execute(
            r#"INSERT INTO devices
               (device_id, device_name, device_type, public_key, fingerprint, role, scopes, created_at, approved_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
               ON CONFLICT(device_id) DO UPDATE SET
                 device_name = excluded.device_name,
                 last_seen_at = ?8"#,
            params![device_id, device_name, device_type, public_key, fingerprint, role, scopes_json, now],
        )?;
        Ok(())
    }

    /// Get device by ID
    pub fn get_device(&self, device_id: &str) -> SqliteResult<Option<DeviceRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE device_id = ?1",
        )?;

        let result = stmt.query_row(params![device_id], |row| DeviceRow::from_row(row));

        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get device by fingerprint
    pub fn get_device_by_fingerprint(&self, fingerprint: &str) -> SqliteResult<Option<DeviceRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE fingerprint = ?1 AND revoked_at IS NULL",
        )?;

        let result = stmt.query_row(params![fingerprint], |row| DeviceRow::from_row(row));

        match result {
            Ok(device) => Ok(Some(device)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check if device is approved (not revoked)
    pub fn is_device_approved(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM devices WHERE device_id = ?1 AND revoked_at IS NULL",
            params![device_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// List all active devices
    pub fn list_devices(&self) -> SqliteResult<Vec<DeviceRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, device_type, public_key, fingerprint, role, scopes,
                    created_at, approved_at, last_seen_at, revoked_at
             FROM devices WHERE revoked_at IS NULL ORDER BY approved_at DESC",
        )?;

        let devices = stmt
            .query_map([], |row| DeviceRow::from_row(row))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(devices)
    }

    /// Update device last_seen_at
    pub fn touch_device(&self, device_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        conn.execute(
            "UPDATE devices SET last_seen_at = ?1 WHERE device_id = ?2",
            params![now, device_id],
        )?;
        Ok(())
    }

    /// Revoke a device
    pub fn revoke_device(&self, device_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let scopes_json = serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_string());

        conn.execute(
            r#"INSERT INTO tokens (token_id, device_id, token_hash, role, scopes, issued_at, expires_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            params![token_id, device_id, token_hash, role, scopes_json, now, expires_at],
        )?;
        Ok(())
    }

    /// Get token by hash
    pub fn get_token_by_hash(&self, token_hash: &str) -> SqliteResult<Option<TokenRow>> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();

        let mut stmt = conn.prepare(
            "SELECT token_id, device_id, token_hash, role, scopes, issued_at, expires_at,
                    last_used_at, rotated_at, revoked_at
             FROM tokens
             WHERE token_hash = ?1 AND revoked_at IS NULL AND expires_at > ?2",
        )?;

        let result = stmt.query_row(params![token_hash, now], |row| TokenRow::from_row(row));

        match result {
            Ok(token) => Ok(Some(token)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Update token last_used_at
    pub fn touch_token(&self, token_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        conn.execute(
            "UPDATE tokens SET last_used_at = ?1 WHERE token_id = ?2",
            params![now, token_id],
        )?;
        Ok(())
    }

    /// Revoke a token
    pub fn revoke_token(&self, token_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE token_id = ?2 AND revoked_at IS NULL",
            params![now, token_id],
        )?;
        Ok(rows > 0)
    }

    /// Revoke all tokens for a device
    pub fn revoke_device_tokens(&self, device_id: &str) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "UPDATE tokens SET revoked_at = ?1 WHERE device_id = ?2 AND revoked_at IS NULL",
            params![now, device_id],
        )?;
        Ok(rows as u64)
    }

    /// Delete expired tokens
    pub fn delete_expired_tokens(&self) -> SqliteResult<u64> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "DELETE FROM tokens WHERE expires_at < ?1 OR revoked_at IS NOT NULL",
            params![now],
        )?;
        Ok(rows as u64)
    }

    // ========== Pairing Request Operations ==========

    /// Insert a pairing request
    pub fn insert_pairing_request(
        &self,
        request_id: &str,
        code: &str,
        pairing_type: &str,
        device_name: Option<&str>,
        device_type: Option<&str>,
        public_key: Option<&[u8]>,
        channel: Option<&str>,
        sender_id: Option<&str>,
        remote_addr: Option<&str>,
        expires_at: i64,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();

        conn.execute(
            r#"INSERT INTO pairing_requests
               (request_id, code, pairing_type, device_name, device_type, public_key,
                channel, sender_id, remote_addr, created_at, expires_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
            params![
                request_id, code, pairing_type, device_name, device_type, public_key,
                channel, sender_id, remote_addr, now, expires_at
            ],
        )?;
        Ok(())
    }

    /// Get pairing request by code
    pub fn get_pairing_request(&self, code: &str) -> SqliteResult<Option<PairingRequestRow>> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();

        let mut stmt = conn.prepare(
            "SELECT request_id, code, pairing_type, device_name, device_type, public_key,
                    channel, sender_id, remote_addr, metadata, created_at, expires_at
             FROM pairing_requests
             WHERE code = ?1 AND expires_at > ?2",
        )?;

        let result = stmt.query_row(params![code, now], |row| PairingRequestRow::from_row(row));

        match result {
            Ok(req) => Ok(Some(req)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete a pairing request
    pub fn delete_pairing_request(&self, code: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute("DELETE FROM pairing_requests WHERE code = ?1", params![code])?;
        Ok(rows > 0)
    }

    /// List pending pairing requests
    pub fn list_pairing_requests(&self) -> SqliteResult<Vec<PairingRequestRow>> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();

        let mut stmt = conn.prepare(
            "SELECT request_id, code, pairing_type, device_name, device_type, public_key,
                    channel, sender_id, remote_addr, metadata, created_at, expires_at
             FROM pairing_requests
             WHERE expires_at > ?1
             ORDER BY created_at DESC",
        )?;

        let requests = stmt
            .query_map(params![now], |row| PairingRequestRow::from_row(row))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(requests)
    }

    /// Count pending pairing requests
    pub fn count_pairing_requests(&self) -> SqliteResult<usize> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let rows = conn.execute("DELETE FROM pairing_requests WHERE expires_at < ?1", params![now])?;
        Ok(rows as u64)
    }

    // ========== Approved Senders Operations ==========

    /// Approve a channel sender
    pub fn approve_sender(&self, channel: &str, sender_id: &str) -> SqliteResult<()> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM approved_senders WHERE channel = ?1 AND sender_id = ?2 AND revoked_at IS NULL",
            params![channel, sender_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Revoke a sender
    pub fn revoke_sender(&self, channel: &str, sender_id: &str) -> SqliteResult<bool> {
        let conn = self.conn.lock().unwrap();
        let now = current_timestamp_ms();
        let rows = conn.execute(
            "UPDATE approved_senders SET revoked_at = ?1 WHERE channel = ?2 AND sender_id = ?3 AND revoked_at IS NULL",
            params![now, channel, sender_id],
        )?;
        Ok(rows > 0)
    }

    /// List approved senders for a channel
    pub fn list_senders(&self, channel: &str) -> SqliteResult<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
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
        let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_default();

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
        let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_default();

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
        .unwrap()
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
            .upsert_device(
                "dev-1",
                "Test Device",
                Some("macos"),
                &[1u8; 32],
                "abc123",
                "operator",
                &["*".to_string()],
            )
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
            .upsert_device("dev-1", "Test", None, &[1u8; 32], "fp", "operator", &[])
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
            .insert_pairing_request(
                "req-1",
                "A3B7K9M2",
                "device",
                Some("iPhone"),
                Some("ios"),
                Some(&[1u8; 32]),
                None,
                None,
                Some("192.168.1.1"),
                expires,
            )
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
```

**Step 2: Run tests**

Run: `cargo test -p aethecore store --features gateway`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/gateway/security/store.rs
git commit -m "feat(security): add SecurityStore with SQLite persistence"
```

---

## Task 4: Create device.rs - Device Types

**Files:**
- Create: `core/src/gateway/security/device.rs`

**Step 1: Write device types**

```rust
// core/src/gateway/security/device.rs

//! Device types and registry for device authentication.

use serde::{Deserialize, Serialize};

use super::crypto::DeviceFingerprint;
use super::store::DeviceRow;

/// Device type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    MacOS,
    IOS,
    Android,
    CLI,
    Web,
}

impl DeviceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceType::MacOS => "macos",
            DeviceType::IOS => "ios",
            DeviceType::Android => "android",
            DeviceType::CLI => "cli",
            DeviceType::Web => "web",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "macos" => Some(DeviceType::MacOS),
            "ios" => Some(DeviceType::IOS),
            "android" => Some(DeviceType::Android),
            "cli" => Some(DeviceType::CLI),
            "web" => Some(DeviceType::Web),
            _ => None,
        }
    }
}

/// Device role - determines permissions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceRole {
    /// Full control (CLI, macOS App, Web UI)
    Operator,
    /// Limited execution (iOS/Android nodes)
    Node,
}

impl DeviceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeviceRole::Operator => "operator",
            DeviceRole::Node => "node",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "operator" => Some(DeviceRole::Operator),
            "node" => Some(DeviceRole::Node),
            _ => None,
        }
    }
}

impl Default for DeviceRole {
    fn default() -> Self {
        DeviceRole::Operator
    }
}

/// An approved device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub device_id: String,
    pub device_name: String,
    pub device_type: Option<DeviceType>,
    pub public_key: Vec<u8>,
    pub fingerprint: DeviceFingerprint,
    pub role: DeviceRole,
    pub scopes: Vec<String>,
    pub created_at: i64,
    pub approved_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl Device {
    /// Check if device is active (not revoked)
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Check if device has a specific scope
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(&"*".to_string()) || self.scopes.iter().any(|s| s == scope)
    }
}

impl From<DeviceRow> for Device {
    fn from(row: DeviceRow) -> Self {
        Device {
            device_id: row.device_id,
            device_name: row.device_name,
            device_type: row.device_type.and_then(|s| DeviceType::from_str(&s)),
            public_key: row.public_key,
            fingerprint: DeviceFingerprint(row.fingerprint),
            role: DeviceRole::from_str(&row.role).unwrap_or_default(),
            scopes: row.scopes,
            created_at: row.created_at,
            approved_at: row.approved_at,
            last_seen_at: row.last_seen_at,
            revoked_at: row.revoked_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_type_conversion() {
        assert_eq!(DeviceType::MacOS.as_str(), "macos");
        assert_eq!(DeviceType::from_str("macos"), Some(DeviceType::MacOS));
        assert_eq!(DeviceType::from_str("MACOS"), Some(DeviceType::MacOS));
        assert_eq!(DeviceType::from_str("unknown"), None);
    }

    #[test]
    fn test_device_role_conversion() {
        assert_eq!(DeviceRole::Operator.as_str(), "operator");
        assert_eq!(DeviceRole::from_str("operator"), Some(DeviceRole::Operator));
        assert_eq!(DeviceRole::from_str("NODE"), Some(DeviceRole::Node));
    }

    #[test]
    fn test_device_has_scope() {
        let device = Device {
            device_id: "test".into(),
            device_name: "Test".into(),
            device_type: None,
            public_key: vec![],
            fingerprint: DeviceFingerprint("abc".into()),
            role: DeviceRole::Operator,
            scopes: vec!["read".into(), "write".into()],
            created_at: 0,
            approved_at: 0,
            last_seen_at: None,
            revoked_at: None,
        };

        assert!(device.has_scope("read"));
        assert!(device.has_scope("write"));
        assert!(!device.has_scope("admin"));
    }

    #[test]
    fn test_device_wildcard_scope() {
        let device = Device {
            device_id: "test".into(),
            device_name: "Test".into(),
            device_type: None,
            public_key: vec![],
            fingerprint: DeviceFingerprint("abc".into()),
            role: DeviceRole::Operator,
            scopes: vec!["*".into()],
            created_at: 0,
            approved_at: 0,
            last_seen_at: None,
            revoked_at: None,
        };

        assert!(device.has_scope("anything"));
        assert!(device.has_scope("admin"));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p aethecore device --features gateway`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/gateway/security/device.rs
git commit -m "feat(security): add Device types and DeviceRole enum"
```

---

## Task 5: Update mod.rs - Module Exports

**Files:**
- Modify: `core/src/gateway/security/mod.rs`

**Step 1: Update module exports**

Replace the entire file:

```rust
// core/src/gateway/security/mod.rs

//! Security Module
//!
//! Provides authentication and authorization for Gateway connections.
//!
//! ## Architecture
//!
//! ```text
//! SecurityManager (unified entry point)
//!   ├── TokenManager (HMAC-signed tokens)
//!   ├── PairingManager (8-char Base32 codes)
//!   └── DeviceRegistry (Ed25519 public keys)
//!          │
//!          ▼
//!     SecurityStore (SQLite)
//! ```

pub mod crypto;
pub mod device;
pub mod pairing;
pub mod store;
pub mod token;

// Re-export commonly used types
pub use crypto::{
    generate_keypair, generate_pairing_code, generate_secret, hmac_sign, hmac_verify,
    sign_message, verify_signature, CryptoError, DeviceFingerprint, PAIRING_CODE_CHARSET,
    PAIRING_CODE_LENGTH,
};
pub use device::{Device, DeviceRole, DeviceType};
pub use pairing::PairingManager;
pub use store::{DeviceRow, PairingRequestRow, SecurityStore, TokenRow};
pub use token::TokenManager;
```

**Step 2: Verify compilation**

Run: `cargo build -p aethecore --features gateway`
Expected: Compiles (may have warnings about unused imports, that's OK)

**Step 3: Commit**

```bash
git add core/src/gateway/security/mod.rs
git commit -m "feat(security): update module exports for new security types"
```

---

## Task 6: Rewrite token.rs - HMAC-Signed Tokens

**Files:**
- Modify: `core/src/gateway/security/token.rs`

**Step 1: Rewrite token.rs with HMAC signing**

Replace the entire file:

```rust
// core/src/gateway/security/token.rs

//! HMAC-Signed Token Management
//!
//! Tokens are signed with HMAC-SHA256 and stored in SQLite.
//! The original token value is never stored - only the hash.

use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

use super::crypto::{generate_secret, hmac_sign, hmac_verify};
use super::device::DeviceRole;
use super::store::SecurityStore;

/// Default token expiry (24 hours in milliseconds)
const DEFAULT_TOKEN_EXPIRY_MS: i64 = 24 * 60 * 60 * 1000;

/// Token-related errors
#[derive(Debug, Error)]
pub enum TokenError {
    #[error("Invalid token")]
    InvalidToken,
    #[error("Token expired")]
    TokenExpired,
    #[error("Token revoked")]
    TokenRevoked,
    #[error("Signature verification failed")]
    SignatureInvalid,
    #[error("Database error: {0}")]
    DatabaseError(String),
}

/// A signed token with its signature
#[derive(Debug, Clone)]
pub struct SignedToken {
    pub token: String,
    pub signature: String,
    pub token_id: String,
    pub expires_at: i64,
}

/// Token validation result
#[derive(Debug, Clone)]
pub struct TokenValidation {
    pub token_id: String,
    pub device_id: String,
    pub role: DeviceRole,
    pub scopes: Vec<String>,
    pub remaining_ms: i64,
}

/// Token manager with HMAC signing
pub struct TokenManager {
    store: Arc<SecurityStore>,
    secret: [u8; 32],
    default_expiry_ms: i64,
}

impl TokenManager {
    /// Create a new token manager
    pub fn new(store: Arc<SecurityStore>) -> Self {
        Self {
            store,
            secret: generate_secret(),
            default_expiry_ms: DEFAULT_TOKEN_EXPIRY_MS,
        }
    }

    /// Create with a specific secret (for testing or persistence)
    pub fn with_secret(store: Arc<SecurityStore>, secret: [u8; 32]) -> Self {
        Self {
            store,
            secret,
            default_expiry_ms: DEFAULT_TOKEN_EXPIRY_MS,
        }
    }

    /// Create with custom expiry
    pub fn with_expiry(store: Arc<SecurityStore>, expiry_ms: i64) -> Self {
        Self {
            store,
            secret: generate_secret(),
            default_expiry_ms: expiry_ms,
        }
    }

    /// Issue a new signed token for a device
    pub fn issue_token(
        &self,
        device_id: &str,
        role: DeviceRole,
        scopes: Vec<String>,
    ) -> Result<SignedToken, TokenError> {
        self.issue_token_with_expiry(device_id, role, scopes, self.default_expiry_ms)
    }

    /// Issue a token with custom expiry
    pub fn issue_token_with_expiry(
        &self,
        device_id: &str,
        role: DeviceRole,
        scopes: Vec<String>,
        expiry_ms: i64,
    ) -> Result<SignedToken, TokenError> {
        let token_id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().to_string();
        let signature = hmac_sign(&self.secret, &token);
        let token_hash = hmac_sign(&self.secret, &token); // Store hash, not token

        let now = current_timestamp_ms();
        let expires_at = now + expiry_ms;

        self.store
            .insert_token(&token_id, device_id, &token_hash, role.as_str(), &scopes, expires_at)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))?;

        Ok(SignedToken {
            token,
            signature,
            token_id,
            expires_at,
        })
    }

    /// Validate a token and its signature
    pub fn validate_token(&self, token: &str, signature: &str) -> Result<TokenValidation, TokenError> {
        // Verify HMAC signature
        hmac_verify(&self.secret, token, signature).map_err(|_| TokenError::SignatureInvalid)?;

        // Compute hash to look up in database
        let token_hash = hmac_sign(&self.secret, token);

        // Look up token in database
        let token_row = self
            .store
            .get_token_by_hash(&token_hash)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))?
            .ok_or(TokenError::InvalidToken)?;

        // Check if revoked (shouldn't happen as query filters, but be safe)
        if token_row.revoked_at.is_some() {
            return Err(TokenError::TokenRevoked);
        }

        // Check expiry
        let now = current_timestamp_ms();
        if token_row.expires_at <= now {
            return Err(TokenError::TokenExpired);
        }

        // Update last_used_at
        let _ = self.store.touch_token(&token_row.token_id);

        Ok(TokenValidation {
            token_id: token_row.token_id,
            device_id: token_row.device_id,
            role: DeviceRole::from_str(&token_row.role).unwrap_or_default(),
            scopes: token_row.scopes,
            remaining_ms: token_row.expires_at - now,
        })
    }

    /// Rotate a token (invalidate old, issue new)
    pub fn rotate_token(&self, old_token: &str, old_signature: &str) -> Result<SignedToken, TokenError> {
        // Validate the old token first
        let validation = self.validate_token(old_token, old_signature)?;

        // Revoke the old token
        self.store
            .revoke_token(&validation.token_id)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))?;

        // Issue a new token with same permissions
        self.issue_token(&validation.device_id, validation.role, validation.scopes)
    }

    /// Revoke a specific token
    pub fn revoke_token(&self, token_id: &str) -> Result<bool, TokenError> {
        self.store
            .revoke_token(token_id)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))
    }

    /// Revoke all tokens for a device
    pub fn revoke_device_tokens(&self, device_id: &str) -> Result<u64, TokenError> {
        self.store
            .revoke_device_tokens(device_id)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))
    }

    /// Clean up expired tokens
    pub fn cleanup_expired(&self) -> Result<u64, TokenError> {
        self.store
            .delete_expired_tokens()
            .map_err(|e| TokenError::DatabaseError(e.to_string()))
    }

    /// Get the HMAC secret (for persistence)
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager() -> TokenManager {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        // Create a device for tokens
        store
            .upsert_device("dev-1", "Test", None, &[1u8; 32], "fp", "operator", &[])
            .unwrap();
        TokenManager::new(store)
    }

    #[test]
    fn test_issue_and_validate() {
        let manager = create_test_manager();

        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec!["*".into()])
            .unwrap();

        assert!(!signed.token.is_empty());
        assert!(!signed.signature.is_empty());

        let validation = manager.validate_token(&signed.token, &signed.signature).unwrap();
        assert_eq!(validation.device_id, "dev-1");
        assert_eq!(validation.role, DeviceRole::Operator);
    }

    #[test]
    fn test_invalid_signature() {
        let manager = create_test_manager();

        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();

        let result = manager.validate_token(&signed.token, "wrong-signature");
        assert!(matches!(result, Err(TokenError::SignatureInvalid)));
    }

    #[test]
    fn test_token_rotation() {
        let manager = create_test_manager();

        let old_token = manager
            .issue_token("dev-1", DeviceRole::Operator, vec!["*".into()])
            .unwrap();

        let new_token = manager
            .rotate_token(&old_token.token, &old_token.signature)
            .unwrap();

        // Old token should be invalid
        let old_result = manager.validate_token(&old_token.token, &old_token.signature);
        assert!(old_result.is_err());

        // New token should be valid
        let new_result = manager.validate_token(&new_token.token, &new_token.signature);
        assert!(new_result.is_ok());
    }

    #[test]
    fn test_revoke_token() {
        let manager = create_test_manager();

        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();

        assert!(manager.validate_token(&signed.token, &signed.signature).is_ok());

        manager.revoke_token(&signed.token_id).unwrap();

        assert!(manager.validate_token(&signed.token, &signed.signature).is_err());
    }

    #[test]
    fn test_revoke_device_tokens() {
        let manager = create_test_manager();

        // Issue multiple tokens
        let t1 = manager.issue_token("dev-1", DeviceRole::Operator, vec![]).unwrap();
        let t2 = manager.issue_token("dev-1", DeviceRole::Operator, vec![]).unwrap();

        // Revoke all
        let count = manager.revoke_device_tokens("dev-1").unwrap();
        assert_eq!(count, 2);

        // Both should be invalid
        assert!(manager.validate_token(&t1.token, &t1.signature).is_err());
        assert!(manager.validate_token(&t2.token, &t2.signature).is_err());
    }

    #[test]
    fn test_token_expiry() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        store
            .upsert_device("dev-1", "Test", None, &[1u8; 32], "fp", "operator", &[])
            .unwrap();

        // Create manager with very short expiry
        let manager = TokenManager::with_expiry(store, 1); // 1ms expiry

        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();

        // Wait for expiry
        std::thread::sleep(std::time::Duration::from_millis(10));

        let result = manager.validate_token(&signed.token, &signed.signature);
        assert!(matches!(result, Err(TokenError::TokenExpired)));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p aethecore token --features gateway`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/gateway/security/token.rs
git commit -m "feat(security): rewrite TokenManager with HMAC signing and SQLite storage"
```

---

## Task 7: Rewrite pairing.rs - 8-Character Base32 Codes

**Files:**
- Modify: `core/src/gateway/security/pairing.rs`

**Step 1: Rewrite pairing.rs**

Replace the entire file:

```rust
// core/src/gateway/security/pairing.rs

//! Device and Channel Pairing
//!
//! Unified pairing system supporting both device authentication
//! and channel sender verification with 8-character Base32 codes.

use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

use super::crypto::{generate_pairing_code, DeviceFingerprint};
use super::device::DeviceType;
use super::store::{PairingRequestRow, SecurityStore};

/// Default pairing code expiry (5 minutes in milliseconds)
const DEFAULT_PAIRING_EXPIRY_MS: i64 = 5 * 60 * 1000;

/// Maximum pending pairing requests
const MAX_PENDING_REQUESTS: usize = 10;

/// Pairing-related errors
#[derive(Debug, Error)]
pub enum PairingError {
    #[error("Invalid pairing code")]
    InvalidCode,
    #[error("Pairing code expired")]
    CodeExpired,
    #[error("Too many pending requests (max {0})")]
    TooManyPending(usize),
    #[error("Database error: {0}")]
    DatabaseError(String),
}

/// A pairing request (device or channel)
#[derive(Debug, Clone)]
pub enum PairingRequest {
    Device {
        request_id: String,
        code: String,
        device_name: String,
        device_type: Option<DeviceType>,
        public_key: Vec<u8>,
        fingerprint: DeviceFingerprint,
        remote_addr: Option<String>,
        created_at: i64,
        expires_at: i64,
    },
    Channel {
        request_id: String,
        code: String,
        channel: String,
        sender_id: String,
        metadata: Option<serde_json::Value>,
        created_at: i64,
        expires_at: i64,
    },
}

impl PairingRequest {
    /// Get the pairing code
    pub fn code(&self) -> &str {
        match self {
            PairingRequest::Device { code, .. } => code,
            PairingRequest::Channel { code, .. } => code,
        }
    }

    /// Get remaining seconds until expiry
    pub fn remaining_secs(&self) -> u64 {
        let expires_at = match self {
            PairingRequest::Device { expires_at, .. } => *expires_at,
            PairingRequest::Channel { expires_at, .. } => *expires_at,
        };
        let now = current_timestamp_ms();
        if expires_at > now {
            ((expires_at - now) / 1000) as u64
        } else {
            0
        }
    }
}

impl From<PairingRequestRow> for PairingRequest {
    fn from(row: PairingRequestRow) -> Self {
        if row.pairing_type == "device" {
            let public_key = row.public_key.unwrap_or_default();
            let fingerprint = DeviceFingerprint::from_public_key(&public_key);
            PairingRequest::Device {
                request_id: row.request_id,
                code: row.code,
                device_name: row.device_name.unwrap_or_else(|| "Unknown".into()),
                device_type: row.device_type.and_then(|s| DeviceType::from_str(&s)),
                public_key,
                fingerprint,
                remote_addr: row.remote_addr,
                created_at: row.created_at,
                expires_at: row.expires_at,
            }
        } else {
            PairingRequest::Channel {
                request_id: row.request_id,
                code: row.code,
                channel: row.channel.unwrap_or_default(),
                sender_id: row.sender_id.unwrap_or_default(),
                metadata: row.metadata.and_then(|s| serde_json::from_str(&s).ok()),
                created_at: row.created_at,
                expires_at: row.expires_at,
            }
        }
    }
}

/// Pairing manager for device and channel authentication
pub struct PairingManager {
    store: Arc<SecurityStore>,
    expiry_ms: i64,
    max_pending: usize,
}

impl PairingManager {
    /// Create a new pairing manager
    pub fn new(store: Arc<SecurityStore>) -> Self {
        Self {
            store,
            expiry_ms: DEFAULT_PAIRING_EXPIRY_MS,
            max_pending: MAX_PENDING_REQUESTS,
        }
    }

    /// Create with custom expiry
    pub fn with_expiry(store: Arc<SecurityStore>, expiry_ms: i64) -> Self {
        Self {
            store,
            expiry_ms,
            max_pending: MAX_PENDING_REQUESTS,
        }
    }

    /// Initiate device pairing
    pub fn request_device_pairing(
        &self,
        device_name: String,
        device_type: Option<DeviceType>,
        public_key: Vec<u8>,
        remote_addr: Option<String>,
    ) -> Result<PairingRequest, PairingError> {
        // Check capacity
        let pending_count = self
            .store
            .count_pairing_requests()
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        if pending_count >= self.max_pending {
            return Err(PairingError::TooManyPending(self.max_pending));
        }

        let request_id = Uuid::new_v4().to_string();
        let code = self.generate_unique_code()?;
        let now = current_timestamp_ms();
        let expires_at = now + self.expiry_ms;

        let fingerprint = DeviceFingerprint::from_public_key(&public_key);

        self.store
            .insert_pairing_request(
                &request_id,
                &code,
                "device",
                Some(&device_name),
                device_type.map(|t| t.as_str()),
                Some(&public_key),
                None,
                None,
                remote_addr.as_deref(),
                expires_at,
            )
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        Ok(PairingRequest::Device {
            request_id,
            code,
            device_name,
            device_type,
            public_key,
            fingerprint,
            remote_addr,
            created_at: now,
            expires_at,
        })
    }

    /// Initiate channel sender pairing
    pub fn request_channel_pairing(
        &self,
        channel: String,
        sender_id: String,
        metadata: Option<serde_json::Value>,
    ) -> Result<PairingRequest, PairingError> {
        // Check capacity
        let pending_count = self
            .store
            .count_pairing_requests()
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        if pending_count >= self.max_pending {
            return Err(PairingError::TooManyPending(self.max_pending));
        }

        let request_id = Uuid::new_v4().to_string();
        let code = self.generate_unique_code()?;
        let now = current_timestamp_ms();
        let expires_at = now + self.expiry_ms;

        self.store
            .insert_pairing_request(
                &request_id,
                &code,
                "channel",
                None,
                None,
                None,
                Some(&channel),
                Some(&sender_id),
                None,
                expires_at,
            )
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        Ok(PairingRequest::Channel {
            request_id,
            code,
            channel,
            sender_id,
            metadata,
            created_at: now,
            expires_at,
        })
    }

    /// Get a pairing request by code
    pub fn get_request(&self, code: &str) -> Result<Option<PairingRequest>, PairingError> {
        let row = self
            .store
            .get_pairing_request(code)
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        Ok(row.map(PairingRequest::from))
    }

    /// Confirm a pairing request (removes it from pending)
    pub fn confirm_pairing(&self, code: &str) -> Result<PairingRequest, PairingError> {
        let request = self.get_request(code)?.ok_or(PairingError::InvalidCode)?;

        if request.remaining_secs() == 0 {
            return Err(PairingError::CodeExpired);
        }

        self.store
            .delete_pairing_request(code)
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        Ok(request)
    }

    /// Cancel/reject a pairing request
    pub fn cancel_pairing(&self, code: &str) -> Result<bool, PairingError> {
        self.store
            .delete_pairing_request(code)
            .map_err(|e| PairingError::DatabaseError(e.to_string()))
    }

    /// List all pending pairing requests
    pub fn list_pending(&self) -> Result<Vec<PairingRequest>, PairingError> {
        let rows = self
            .store
            .list_pairing_requests()
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        Ok(rows.into_iter().map(PairingRequest::from).collect())
    }

    /// Clean up expired pairing requests
    pub fn cleanup_expired(&self) -> Result<u64, PairingError> {
        self.store
            .delete_expired_pairing_requests()
            .map_err(|e| PairingError::DatabaseError(e.to_string()))
    }

    /// Generate a unique pairing code
    fn generate_unique_code(&self) -> Result<String, PairingError> {
        // Try up to 10 times to generate a unique code
        for _ in 0..10 {
            let code = generate_pairing_code();
            let existing = self
                .store
                .get_pairing_request(&code)
                .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

            if existing.is_none() {
                return Ok(code);
            }
        }

        // This should be extremely rare with 32^8 possibilities
        Err(PairingError::DatabaseError("Failed to generate unique code".into()))
    }
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager() -> PairingManager {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        PairingManager::new(store)
    }

    #[test]
    fn test_device_pairing_flow() {
        let manager = create_test_manager();

        // Request pairing
        let request = manager
            .request_device_pairing(
                "Test iPhone".into(),
                Some(DeviceType::IOS),
                vec![1u8; 32],
                Some("192.168.1.1".into()),
            )
            .unwrap();

        let code = request.code().to_string();
        assert_eq!(code.len(), 8);

        // Get request
        let fetched = manager.get_request(&code).unwrap().unwrap();
        assert!(matches!(fetched, PairingRequest::Device { .. }));

        // Confirm
        let confirmed = manager.confirm_pairing(&code).unwrap();
        assert!(matches!(confirmed, PairingRequest::Device { device_name, .. } if device_name == "Test iPhone"));

        // Should be gone
        assert!(manager.get_request(&code).unwrap().is_none());
    }

    #[test]
    fn test_channel_pairing_flow() {
        let manager = create_test_manager();

        let request = manager
            .request_channel_pairing("telegram".into(), "user123".into(), None)
            .unwrap();

        let code = request.code().to_string();

        let confirmed = manager.confirm_pairing(&code).unwrap();
        assert!(matches!(confirmed, PairingRequest::Channel { channel, sender_id, .. }
            if channel == "telegram" && sender_id == "user123"));
    }

    #[test]
    fn test_invalid_code() {
        let manager = create_test_manager();
        let result = manager.confirm_pairing("INVALID1");
        assert!(matches!(result, Err(PairingError::InvalidCode)));
    }

    #[test]
    fn test_cancel_pairing() {
        let manager = create_test_manager();

        let request = manager
            .request_device_pairing("Test".into(), None, vec![1u8; 32], None)
            .unwrap();

        let code = request.code().to_string();
        assert!(manager.cancel_pairing(&code).unwrap());
        assert!(manager.get_request(&code).unwrap().is_none());
    }

    #[test]
    fn test_list_pending() {
        let manager = create_test_manager();

        manager
            .request_device_pairing("Device 1".into(), None, vec![1u8; 32], None)
            .unwrap();
        manager
            .request_device_pairing("Device 2".into(), None, vec![2u8; 32], None)
            .unwrap();
        manager
            .request_channel_pairing("telegram".into(), "user1".into(), None)
            .unwrap();

        let pending = manager.list_pending().unwrap();
        assert_eq!(pending.len(), 3);
    }

    #[test]
    fn test_capacity_limit() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = PairingManager {
            store,
            expiry_ms: DEFAULT_PAIRING_EXPIRY_MS,
            max_pending: 2,
        };

        manager
            .request_device_pairing("D1".into(), None, vec![1u8; 32], None)
            .unwrap();
        manager
            .request_device_pairing("D2".into(), None, vec![2u8; 32], None)
            .unwrap();

        let result = manager.request_device_pairing("D3".into(), None, vec![3u8; 32], None);
        assert!(matches!(result, Err(PairingError::TooManyPending(2))));
    }

    #[test]
    fn test_code_format() {
        let manager = create_test_manager();

        for _ in 0..10 {
            let request = manager
                .request_device_pairing("Test".into(), None, vec![1u8; 32], None)
                .unwrap();

            let code = request.code();
            assert_eq!(code.len(), 8);

            // Should not contain confusing characters
            assert!(!code.contains('0'));
            assert!(!code.contains('1'));
            assert!(!code.contains('I'));
            assert!(!code.contains('O'));

            manager.cancel_pairing(code).unwrap();
        }
    }

    #[test]
    fn test_expiry() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = PairingManager::with_expiry(store, 1); // 1ms expiry

        let request = manager
            .request_device_pairing("Test".into(), None, vec![1u8; 32], None)
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        // Should still be fetchable but with 0 remaining
        let fetched = manager.get_request(request.code()).unwrap();
        assert!(fetched.is_none()); // Query filters expired

        // Confirm should fail
        let result = manager.confirm_pairing(request.code());
        assert!(matches!(result, Err(PairingError::InvalidCode)));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p aethecore pairing --features gateway`
Expected: All tests pass

**Step 3: Commit**

```bash
git add core/src/gateway/security/pairing.rs
git commit -m "feat(security): rewrite PairingManager with 8-char Base32 codes and SQLite"
```

---

## Task 8: Run Full Test Suite

**Step 1: Run all security tests**

Run: `cargo test -p aethecore --features gateway -- security`
Expected: All tests pass

**Step 2: Run full test suite**

Run: `cargo test -p aethecore --features gateway`
Expected: Pass (same 2 pre-existing failures in p3_router)

**Step 3: Commit if all tests pass**

```bash
git add -A
git commit -m "test: verify security module integration"
```

---

## Summary

**Completed Tasks:**
1. Add dependencies (ed25519-dalek, base32)
2. Create crypto.rs (Ed25519 + HMAC utilities)
3. Create store.rs (SQLite storage layer)
4. Create device.rs (Device types)
5. Update mod.rs (module exports)
6. Rewrite token.rs (HMAC-signed tokens)
7. Rewrite pairing.rs (8-char Base32 codes)
8. Full test suite verification

**Remaining Tasks (Phase 2 - RPC Handlers):**
- Task 9: Create SecurityManager unified entry point
- Task 10: Create error.rs with error codes
- Task 11: Rewrite handlers/auth.rs for protocol v2
- Task 12: Update handlers/pairing.rs
- Task 13: Add handlers/devices.rs
- Task 14: Add handlers/tokens.rs
- Task 15: Integration with server.rs
- Task 16: Delete old files (device_store.rs, pairing_store.rs)

**Note:** This plan covers the foundational infrastructure (Tasks 1-8). After completing these, the next phase will implement the RPC handlers and server integration.
