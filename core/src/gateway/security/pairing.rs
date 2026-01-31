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
