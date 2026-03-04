//! Challenge-Response Handshake Manager for WebSocket connections.
//!
//! Implements a challenge-response authentication flow:
//! 1. Server generates a challenge with a random nonce
//! 2. Client computes HMAC-SHA256 signature over the nonce + metadata
//! 3. Server verifies the signature and marks the nonce as used
//!
//! Prevents replay attacks via a used-nonce set and timestamp windowing.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::{DashMap, DashSet};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Challenge
// ---------------------------------------------------------------------------

/// A challenge issued by the server for client authentication.
#[derive(Debug, Clone, Serialize)]
pub struct Challenge {
    /// 64-character hex string (32 random bytes).
    pub nonce: String,
    /// Unix timestamp in milliseconds when the challenge was created.
    pub timestamp: u64,
    /// Unique identifier of the server that issued this challenge.
    pub server_id: String,
}

// ---------------------------------------------------------------------------
// ChallengeError
// ---------------------------------------------------------------------------

/// Errors that can occur during challenge verification.
#[derive(Debug, thiserror::Error)]
pub enum ChallengeError {
    #[error("challenge nonce not found")]
    NonceNotFound,

    #[error("nonce already used (replay attack)")]
    NonceReplay,

    #[error("challenge timestamp expired")]
    TimestampExpired,

    #[error("invalid challenge signature")]
    InvalidSignature,
}

// ---------------------------------------------------------------------------
// PendingNonce (private)
// ---------------------------------------------------------------------------

/// Internal bookkeeping for a nonce that has been issued but not yet verified.
struct PendingNonce {
    /// The nonce string itself (kept for clarity, key in DashMap is the same).
    #[allow(dead_code)]
    nonce: String,
    /// Monotonic instant when this nonce was created (used for pruning).
    created_at: Instant,
    /// Unix millisecond timestamp embedded in the challenge.
    timestamp: u64,
}

// ---------------------------------------------------------------------------
// ChallengeManager
// ---------------------------------------------------------------------------

/// Manages the lifecycle of challenge-response handshakes.
///
/// Thread-safe: all internal state is protected by lock-free concurrent maps.
pub struct ChallengeManager {
    /// Nonces that have been issued and are awaiting verification.
    pending: DashMap<String, PendingNonce>,
    /// Nonces that have already been successfully verified (replay guard).
    used: DashSet<String>,
    /// Identifier of this server instance.
    server_id: String,
}

/// Timestamp tolerance window: challenges are valid for +/- 30 seconds.
const TIMESTAMP_WINDOW_SECS: u64 = 30;

/// Maximum number of entries in the used-nonce set before pruning trims it.
const USED_SET_CAP: usize = 10_000;

impl ChallengeManager {
    /// Create a new `ChallengeManager` with an auto-generated server id.
    pub fn new() -> Self {
        Self::with_server_id(Uuid::new_v4().to_string())
    }

    /// Create a new `ChallengeManager` with an explicit server id.
    pub fn with_server_id(server_id: String) -> Self {
        Self {
            pending: DashMap::new(),
            used: DashSet::new(),
            server_id,
        }
    }

    /// Generate a new [`Challenge`].
    ///
    /// The nonce is 64 hex characters derived from two UUID v4 values.
    pub fn generate(&self) -> Challenge {
        // Two UUID v4 → 32 hex chars each → 64 hex chars total.
        let nonce = format!(
            "{}{}",
            Uuid::new_v4().as_simple(),
            Uuid::new_v4().as_simple(),
        );

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.pending.insert(
            nonce.clone(),
            PendingNonce {
                nonce: nonce.clone(),
                created_at: Instant::now(),
                timestamp,
            },
        );

        Challenge {
            nonce,
            timestamp,
            server_id: self.server_id.clone(),
        }
    }

    /// Verify a challenge response from a client.
    ///
    /// # Verification steps
    /// 1. Reject if the nonce was already used (replay).
    /// 2. Remove from pending; reject if not found.
    /// 3. Check timestamp is within the +-30 s window.
    /// 4. Compute expected HMAC-SHA256 signature; reject on mismatch.
    /// 5. Record the nonce as used and return success.
    pub fn verify(
        &self,
        nonce: &str,
        device_id: &str,
        signature: &str,
        token: &str,
    ) -> Result<(), ChallengeError> {
        // 1. Replay check
        if self.used.contains(nonce) {
            return Err(ChallengeError::NonceReplay);
        }

        // 2. Pending lookup
        let (_, pending) = self
            .pending
            .remove(nonce)
            .ok_or(ChallengeError::NonceNotFound)?;

        // 3. Timestamp window check
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let window_ms = TIMESTAMP_WINDOW_SECS * 1000;
        let lower = pending.timestamp.saturating_sub(window_ms);
        let upper = pending.timestamp.saturating_add(window_ms);

        if now_ms < lower || now_ms > upper {
            return Err(ChallengeError::TimestampExpired);
        }

        // 4. Signature verification
        let expected = compute_signature(token, nonce, pending.timestamp, device_id);
        if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
            return Err(ChallengeError::InvalidSignature);
        }

        // 5. Mark as used
        self.used.insert(nonce.to_owned());

        Ok(())
    }

    /// Number of pending (un-verified) challenges.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Remove stale entries.
    ///
    /// - Pending nonces older than `max_age` are dropped.
    /// - If the used-nonce set exceeds [`USED_SET_CAP`], it is cleared.
    pub fn prune(&self, max_age: Duration) {
        let now = Instant::now();

        self.pending.retain(|_key, entry| {
            now.duration_since(entry.created_at) < max_age
        });

        if self.used.len() > USED_SET_CAP {
            self.used.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// compute_signature (public helper)
// ---------------------------------------------------------------------------

/// Compute the expected HMAC-SHA256 signature for a challenge response.
///
/// ```text
/// msg = "{nonce}{timestamp}{device_id}"
/// sig = HMAC-SHA256(key = token, msg)   → hex-encoded
/// ```
pub fn compute_signature(token: &str, nonce: &str, timestamp: u64, device_id: &str) -> String {
    let msg = format!("{nonce}{timestamp}{device_id}");

    let mut mac =
        HmacSha256::new_from_slice(token.as_bytes()).expect("HMAC accepts any key length");
    mac.update(msg.as_bytes());

    hex::encode(mac.finalize().into_bytes())
}

// ---------------------------------------------------------------------------
// Constant-time comparison
// ---------------------------------------------------------------------------

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_challenge() {
        let mgr = ChallengeManager::new();
        let challenge = mgr.generate();

        assert_eq!(challenge.nonce.len(), 64, "nonce should be 64 hex chars");
        assert!(challenge.timestamp > 0, "timestamp must be positive");
        assert!(!challenge.server_id.is_empty(), "server_id must not be empty");
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn test_verify_challenge_success() {
        let mgr = ChallengeManager::with_server_id("test-server".into());
        let token = "my-secret-token";
        let device_id = "device-42";

        let challenge = mgr.generate();

        let sig = compute_signature(token, &challenge.nonce, challenge.timestamp, device_id);

        let result = mgr.verify(&challenge.nonce, device_id, &sig, token);
        assert!(result.is_ok(), "verify should succeed: {result:?}");
        assert_eq!(mgr.pending_count(), 0, "nonce should be consumed");
    }

    #[test]
    fn test_verify_wrong_signature_fails() {
        let mgr = ChallengeManager::new();
        let challenge = mgr.generate();

        let result = mgr.verify(&challenge.nonce, "device-1", "bad-signature", "token");
        assert!(
            matches!(result, Err(ChallengeError::InvalidSignature)),
            "expected InvalidSignature, got {result:?}"
        );
    }

    #[test]
    fn test_nonce_replay_prevention() {
        let mgr = ChallengeManager::new();
        let token = "tok";
        let device_id = "dev";
        let challenge = mgr.generate();

        let sig = compute_signature(token, &challenge.nonce, challenge.timestamp, device_id);

        // First verify succeeds
        assert!(mgr.verify(&challenge.nonce, device_id, &sig, token).is_ok());

        // Second verify must fail with NonceReplay
        let result = mgr.verify(&challenge.nonce, device_id, &sig, token);
        assert!(
            matches!(result, Err(ChallengeError::NonceReplay)),
            "expected NonceReplay, got {result:?}"
        );
    }

    #[test]
    fn test_prune_expired_nonces() {
        let mgr = ChallengeManager::new();

        for _ in 0..5 {
            mgr.generate();
        }
        assert_eq!(mgr.pending_count(), 5);

        // Duration::ZERO means everything is expired
        mgr.prune(Duration::ZERO);
        assert_eq!(mgr.pending_count(), 0, "all nonces should be pruned");
    }
}
