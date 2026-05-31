// core/src/gateway/security/pairing.rs

//! Device and Channel Pairing
//!
//! Unified pairing system supporting both device authentication
//! and channel sender verification with 8-character Base32 codes.

use crate::sync_primitives::Arc;
use dashmap::DashMap;
use thiserror::Error;
use uuid::Uuid;

use super::crypto::{generate_pairing_code, DeviceFingerprint};
use super::device::DeviceType;
use super::store::{PairingRequestData, PairingRequestRow, SecurityStore};

/// Default pairing code expiry (5 minutes in milliseconds)
const DEFAULT_PAIRING_EXPIRY_MS: i64 = 5 * 60 * 1000;

/// Maximum pending pairing requests
const MAX_PENDING_REQUESTS: usize = 10;

/// How long an approved browser session_id is retained after pairing.approve
/// runs, giving the cold browser time to consume it via `pairing.poll`.
/// After this window the entry is dropped from `approved_browser_sessions`
/// and a late poll returns `PollState::Expired`.
const APPROVED_SESSION_TTL_MS: i64 = 5 * 60 * 1000;

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

/// A pairing request (device, channel, or browser)
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
    /// Cold-browser pairing: an anonymous web client (Safari, Chrome on
    /// LAN, mobile) hits `/pair` and asks for a 6-digit numeric code that
    /// the desktop-app operator can approve from the notification bell.
    Browser {
        request_id: String,
        code: String,
        origin_label: String,
        user_agent: String,
        peer_ip: String,
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
            PairingRequest::Browser { code, .. } => code,
        }
    }

    /// Get the expiry timestamp (ms since UNIX epoch)
    pub fn expires_at(&self) -> i64 {
        match self {
            PairingRequest::Device { expires_at, .. } => *expires_at,
            PairingRequest::Channel { expires_at, .. } => *expires_at,
            PairingRequest::Browser { expires_at, .. } => *expires_at,
        }
    }

    /// Get remaining seconds until expiry
    pub fn remaining_secs(&self) -> u64 {
        let expires_at = self.expires_at();
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
        match row.pairing_type.as_str() {
            "device" => {
                let public_key = row.public_key.unwrap_or_default();
                let fingerprint = DeviceFingerprint::from_public_key(&public_key);
                PairingRequest::Device {
                    request_id: row.request_id,
                    code: row.code,
                    device_name: row.device_name.unwrap_or_else(|| "Unknown".into()),
                    device_type: row.device_type.and_then(|s| DeviceType::from_str_opt(&s)),
                    public_key,
                    fingerprint,
                    remote_addr: row.remote_addr,
                    created_at: row.created_at,
                    expires_at: row.expires_at,
                }
            }
            "browser" => PairingRequest::Browser {
                request_id: row.request_id,
                code: row.code,
                // origin_label / user_agent / peer_ip are required for the
                // browser variant; the DB column is nullable for back-compat
                // with rows written before the v9 schema bump, so fall back
                // to empty strings rather than panicking on legacy rows.
                origin_label: row.origin_label.unwrap_or_default(),
                user_agent: row.user_agent.unwrap_or_default(),
                peer_ip: row.peer_ip.unwrap_or_default(),
                created_at: row.created_at,
                expires_at: row.expires_at,
            },
            _ => PairingRequest::Channel {
                request_id: row.request_id,
                code: row.code,
                channel: row.channel.unwrap_or_default(),
                sender_id: row.sender_id.unwrap_or_default(),
                metadata: row.metadata.and_then(|s| serde_json::from_str(&s).ok()),
                created_at: row.created_at,
                expires_at: row.expires_at,
            },
        }
    }
}

/// Result of `PairingManager::poll_browser_pairing`.
///
/// The cold browser polls every ~2s; this enum is the entire response
/// surface. `Approved` carries the freshly-minted `session_id` so the
/// browser can redirect to `/auth/bootstrap/from_pairing?code=…` and the
/// HTTP handler trades it for a session cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollState {
    /// Pairing record still exists in the store and has not been approved.
    Pending,
    /// `pairing.approve` ran; `record_browser_session` deposited the
    /// session_id keyed by the pairing code.
    Approved { session_id: String },
    /// `pairing.reject` ran or the record was cancelled by the operator.
    Rejected,
    /// Record was never created, has expired, or the approval TTL elapsed
    /// before the browser polled.
    Expired,
}

/// Pairing manager for device, channel, and browser pairing.
pub struct PairingManager {
    store: Arc<SecurityStore>,
    expiry_ms: i64,
    max_pending: usize,
    /// In-memory map: pairing code → (session_id, approved_at_ms).
    /// Populated by `record_browser_session` when the operator approves a
    /// Browser pairing; drained by `fetch_browser_session` when the
    /// `/auth/bootstrap/from_pairing` HTTP route is hit. Single-use: entries
    /// are removed on retrieval. TTL bounded by `APPROVED_SESSION_TTL_MS`
    /// so a never-redirected browser doesn't pin memory.
    approved_browser_sessions: DashMap<String, (String, i64)>,
    /// In-memory map: pairing code → "rejected". Lets `poll_browser_pairing`
    /// distinguish "expired / unknown" from "operator explicitly rejected"
    /// after `cancel_pairing` removed the DB row. Same TTL semantics as
    /// `approved_browser_sessions`.
    rejected_browser_codes: DashMap<String, i64>,
}

impl PairingManager {
    /// Create a new pairing manager
    pub fn new(store: Arc<SecurityStore>) -> Self {
        Self {
            store,
            expiry_ms: DEFAULT_PAIRING_EXPIRY_MS,
            max_pending: MAX_PENDING_REQUESTS,
            approved_browser_sessions: DashMap::new(),
            rejected_browser_codes: DashMap::new(),
        }
    }

    /// Create with custom expiry
    pub fn with_expiry(store: Arc<SecurityStore>, expiry_ms: i64) -> Self {
        Self {
            store,
            expiry_ms,
            max_pending: MAX_PENDING_REQUESTS,
            approved_browser_sessions: DashMap::new(),
            rejected_browser_codes: DashMap::new(),
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
            .insert_pairing_request(&PairingRequestData {
                request_id: &request_id,
                code: &code,
                pairing_type: "device",
                device_name: Some(&device_name),
                device_type: device_type.map(|t| t.as_str()),
                public_key: Some(&public_key),
                remote_addr: remote_addr.as_deref(),
                expires_at,
                ..PairingRequestData::default()
            })
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
            .insert_pairing_request(&PairingRequestData {
                request_id: &request_id,
                code: &code,
                pairing_type: "channel",
                channel: Some(&channel),
                sender_id: Some(&sender_id),
                expires_at,
                ..PairingRequestData::default()
            })
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

    /// Create a cold-browser pairing record and return `(code, expires_at_ms)`.
    ///
    /// The code is a 6-digit numeric string in `100000..=999999` (separate
    /// namespace from the 8-char Base32 device code, so device and browser
    /// codes can never collide). `origin_label`, `user_agent`, `peer_ip` are
    /// stored verbatim — they are display-only (the Panel renders them in
    /// the notification card); the security boundary is the operator's
    /// 1-click approve, not these client-supplied strings.
    pub fn create_browser_pairing(
        &self,
        origin_label: &str,
        user_agent: &str,
        peer_ip: &str,
    ) -> Result<(String, i64), PairingError> {
        let pending_count = self
            .store
            .count_pairing_requests()
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        if pending_count >= self.max_pending {
            return Err(PairingError::TooManyPending(self.max_pending));
        }

        let request_id = Uuid::new_v4().to_string();
        let code = self.generate_unique_browser_code()?;
        let now = current_timestamp_ms();
        let expires_at = now + self.expiry_ms;

        self.store
            .insert_pairing_request(&PairingRequestData {
                request_id: &request_id,
                code: &code,
                pairing_type: "browser",
                remote_addr: Some(peer_ip),
                origin_label: Some(origin_label),
                user_agent: Some(user_agent),
                peer_ip: Some(peer_ip),
                expires_at,
                ..PairingRequestData::default()
            })
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        Ok((code, expires_at))
    }

    /// Record the session_id minted for an approved Browser pairing so the
    /// browser's next `pairing.poll` can pick it up. Bounded TTL prevents
    /// a never-redirected browser from pinning memory.
    pub fn record_browser_session(&self, code: &str, session_id: &str) {
        self.gc_browser_state();
        self.approved_browser_sessions.insert(
            code.to_string(),
            (session_id.to_string(), current_timestamp_ms()),
        );
    }

    /// Atomically remove and return the session_id stashed by
    /// `record_browser_session`. Single-use: a second call returns `None`.
    /// Used by the loopback `/auth/bootstrap/from_pairing` HTTP route to
    /// trade an approved pairing code for a session cookie.
    pub fn fetch_browser_session(&self, code: &str) -> Option<String> {
        self.gc_browser_state();
        self.approved_browser_sessions
            .remove(code)
            .map(|(_, (session_id, _))| session_id)
    }

    /// Mark a Browser pairing as operator-rejected so `poll_browser_pairing`
    /// can distinguish "rejected" from "expired / unknown" after the DB row
    /// is removed by `cancel_pairing`.
    pub fn mark_browser_rejected(&self, code: &str) {
        self.gc_browser_state();
        self.rejected_browser_codes
            .insert(code.to_string(), current_timestamp_ms());
    }

    /// Resolve the current state of a Browser pairing for the polling
    /// client. Tri-state: Pending while the DB row exists, Approved /
    /// Rejected per the in-memory side-tables, Expired otherwise.
    pub fn poll_browser_pairing(&self, code: &str) -> PollState {
        self.gc_browser_state();
        if let Some((_, (session_id, _))) = self.approved_browser_sessions.remove(code) {
            return PollState::Approved { session_id };
        }
        if self.rejected_browser_codes.remove(code).is_some() {
            return PollState::Rejected;
        }
        match self.store.get_pairing_request(code) {
            Ok(Some(row)) if row.pairing_type == "browser" => PollState::Pending,
            _ => PollState::Expired,
        }
    }

    /// Drop stale entries from the approved/rejected side-tables. Called
    /// lazily on every browser-pairing op; no background task needed.
    fn gc_browser_state(&self) {
        let cutoff = current_timestamp_ms() - APPROVED_SESSION_TTL_MS;
        self.approved_browser_sessions
            .retain(|_, (_, recorded_at)| *recorded_at >= cutoff);
        self.rejected_browser_codes
            .retain(|_, recorded_at| *recorded_at >= cutoff);
    }

    /// Generate a unique 6-digit numeric browser pairing code in the range
    /// `100000..=999999` (always 6 digits, never collides with the 8-char
    /// Base32 device codespace).
    fn generate_unique_browser_code(&self) -> Result<String, PairingError> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        for _ in 0..10 {
            let code = format!("{:06}", rng.gen_range(100_000..=999_999));
            let existing = self
                .store
                .get_pairing_request(&code)
                .map_err(|e| PairingError::DatabaseError(e.to_string()))?;
            if existing.is_none() {
                return Ok(code);
            }
        }
        Err(PairingError::DatabaseError(
            "Failed to generate unique browser code".into(),
        ))
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
        Err(PairingError::DatabaseError(
            "Failed to generate unique code".into(),
        ))
    }
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
        assert!(
            matches!(confirmed, PairingRequest::Device { device_name, .. } if device_name == "Test iPhone")
        );

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
        assert!(
            matches!(confirmed, PairingRequest::Channel { channel, sender_id, .. }
            if channel == "telegram" && sender_id == "user123")
        );
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
            approved_browser_sessions: DashMap::new(),
            rejected_browser_codes: DashMap::new(),
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

    #[test]
    fn browser_variant_carries_origin_metadata() {
        let pr = PairingRequest::Browser {
            request_id: "rid".into(),
            code: "654321".into(),
            origin_label: "Safari on 192.168.1.5".into(),
            user_agent: "Mozilla/5.0".into(),
            peer_ip: "192.168.1.5".into(),
            created_at: 0,
            expires_at: 300_000,
        };
        assert_eq!(pr.code(), "654321");
        assert_eq!(pr.expires_at(), 300_000);
        assert!(matches!(pr, PairingRequest::Browser { .. }));
    }

    #[test]
    fn create_browser_pairing_produces_six_digit_code() {
        let manager = create_test_manager();
        let (code, expires_at) = manager
            .create_browser_pairing("Safari on LAN", "Mozilla/5.0", "192.168.1.10")
            .unwrap();
        assert_eq!(code.len(), 6, "browser code should be 6 chars: {code}");
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert!(expires_at > current_timestamp_ms());

        // Row round-trips back as Browser variant carrying the metadata.
        let fetched = manager.get_request(&code).unwrap().unwrap();
        match fetched {
            PairingRequest::Browser {
                origin_label,
                user_agent,
                peer_ip,
                ..
            } => {
                assert_eq!(origin_label, "Safari on LAN");
                assert_eq!(user_agent, "Mozilla/5.0");
                assert_eq!(peer_ip, "192.168.1.10");
            }
            other => panic!("expected Browser variant, got {other:?}"),
        }
    }

    #[test]
    fn poll_browser_pairing_pending_then_approved() {
        let manager = create_test_manager();
        let (code, _) = manager
            .create_browser_pairing("Browser", "ua", "127.0.0.1")
            .unwrap();
        assert_eq!(manager.poll_browser_pairing(&code), PollState::Pending);

        manager.record_browser_session(&code, "session-xyz");
        match manager.poll_browser_pairing(&code) {
            PollState::Approved { session_id } => assert_eq!(session_id, "session-xyz"),
            other => panic!("expected Approved, got {other:?}"),
        }
        // Single-use: a second poll-after-approve falls through to expired
        // (the approved entry was drained, the DB row was never deleted by
        // poll itself, but the test approve path mocks that — see handler
        // tests for the full sequence).
    }

    #[test]
    fn poll_browser_pairing_rejected_after_mark() {
        let manager = create_test_manager();
        let (code, _) = manager
            .create_browser_pairing("Browser", "ua", "127.0.0.1")
            .unwrap();
        manager.mark_browser_rejected(&code);
        // cancel_pairing would normally remove the DB row; simulate that.
        let _ = manager.cancel_pairing(&code);
        assert_eq!(manager.poll_browser_pairing(&code), PollState::Rejected);
    }

    #[test]
    fn poll_browser_pairing_unknown_is_expired() {
        let manager = create_test_manager();
        assert_eq!(manager.poll_browser_pairing("999999"), PollState::Expired);
    }

    #[test]
    fn fetch_browser_session_is_single_use() {
        let manager = create_test_manager();
        let (code, _) = manager
            .create_browser_pairing("Browser", "ua", "127.0.0.1")
            .unwrap();
        manager.record_browser_session(&code, "sid-1");
        assert_eq!(
            manager.fetch_browser_session(&code).as_deref(),
            Some("sid-1")
        );
        assert!(manager.fetch_browser_session(&code).is_none());
    }
}
