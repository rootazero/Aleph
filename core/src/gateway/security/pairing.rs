//! Device Pairing Flow
//!
//! Handles the pairing process for new devices connecting to the Gateway.
//! Uses a PIN-based approach where the device displays a PIN and the user
//! confirms it on an already-paired device.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Default pairing code expiry time (5 minutes)
const PAIRING_CODE_EXPIRY: Duration = Duration::from_secs(300);

/// A pending pairing request
#[allow(dead_code)]
struct PairingRequest {
    /// The 6-digit pairing code
    code: String,
    /// When the request was created
    created_at: Instant,
    /// Name of the device requesting pairing
    device_name: String,
    /// Device type (e.g., "ios", "android", "desktop")
    device_type: Option<String>,
    /// IP address of the requesting device
    remote_addr: Option<String>,
}

/// Manager for device pairing flow
///
/// Handles the generation and validation of pairing codes for
/// authenticating new devices with the Gateway.
pub struct PairingManager {
    pending: Arc<RwLock<HashMap<String, PairingRequest>>>,
    expiry: Duration,
}

impl PairingManager {
    /// Create a new pairing manager with default settings
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            expiry: PAIRING_CODE_EXPIRY,
        }
    }

    /// Create a pairing manager with custom expiry
    pub fn with_expiry(expiry: Duration) -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            expiry,
        }
    }

    /// Initiate a new pairing request
    ///
    /// # Arguments
    ///
    /// * `device_name` - Human-readable name for the device
    ///
    /// # Returns
    ///
    /// A 6-digit pairing code to display to the user
    pub async fn initiate_pairing(&self, device_name: String) -> String {
        self.initiate_pairing_with_info(device_name, None, None).await
    }

    /// Initiate pairing with additional device info
    ///
    /// # Arguments
    ///
    /// * `device_name` - Human-readable name for the device
    /// * `device_type` - Type of device (ios, android, desktop, etc.)
    /// * `remote_addr` - IP address of the device
    ///
    /// # Returns
    ///
    /// A 6-digit pairing code
    pub async fn initiate_pairing_with_info(
        &self,
        device_name: String,
        device_type: Option<String>,
        remote_addr: Option<String>,
    ) -> String {
        // Generate a unique 6-digit code
        let code = loop {
            let candidate = format!("{:06}", rand::random::<u32>() % 1_000_000);
            let pending = self.pending.read().await;
            if !pending.contains_key(&candidate) {
                break candidate;
            }
        };

        let mut pending = self.pending.write().await;
        pending.insert(
            code.clone(),
            PairingRequest {
                code: code.clone(),
                created_at: Instant::now(),
                device_name,
                device_type,
                remote_addr,
            },
        );

        code
    }

    /// Confirm a pairing request
    ///
    /// # Arguments
    ///
    /// * `code` - The pairing code to confirm
    ///
    /// # Returns
    ///
    /// The device name if the code was valid and not expired
    pub async fn confirm_pairing(&self, code: &str) -> Option<String> {
        let mut pending = self.pending.write().await;

        if let Some(request) = pending.remove(code) {
            if request.created_at.elapsed() < self.expiry {
                return Some(request.device_name);
            }
        }
        None
    }

    /// Get information about a pending pairing request
    ///
    /// # Arguments
    ///
    /// * `code` - The pairing code to look up
    ///
    /// # Returns
    ///
    /// Device name, type, and remaining time if valid
    pub async fn get_pairing_info(&self, code: &str) -> Option<PairingInfo> {
        let pending = self.pending.read().await;

        pending.get(code).and_then(|request| {
            let elapsed = request.created_at.elapsed();
            if elapsed < self.expiry {
                Some(PairingInfo {
                    device_name: request.device_name.clone(),
                    device_type: request.device_type.clone(),
                    remote_addr: request.remote_addr.clone(),
                    remaining_secs: (self.expiry - elapsed).as_secs(),
                })
            } else {
                None
            }
        })
    }

    /// Cancel a pending pairing request
    pub async fn cancel_pairing(&self, code: &str) -> bool {
        let mut pending = self.pending.write().await;
        pending.remove(code).is_some()
    }

    /// List all pending pairing requests
    ///
    /// Returns a list of (code, device_name, remaining_seconds) tuples
    pub async fn list_pending(&self) -> Vec<(String, String, u64)> {
        let pending = self.pending.read().await;
        pending
            .iter()
            .filter_map(|(code, request)| {
                let elapsed = request.created_at.elapsed();
                if elapsed < self.expiry {
                    Some((
                        code.clone(),
                        request.device_name.clone(),
                        (self.expiry - elapsed).as_secs(),
                    ))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Clean up expired pairing requests
    ///
    /// This should be called periodically to free memory.
    pub async fn cleanup_expired(&self) -> usize {
        let mut pending = self.pending.write().await;
        let before = pending.len();
        pending.retain(|_, req| req.created_at.elapsed() < self.expiry);
        before - pending.len()
    }

    /// Get the number of pending pairing requests
    pub async fn pending_count(&self) -> usize {
        let pending = self.pending.read().await;
        pending.values()
            .filter(|req| req.created_at.elapsed() < self.expiry)
            .count()
    }
}

impl Default for PairingManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about a pending pairing request
#[derive(Debug, Clone)]
pub struct PairingInfo {
    /// Device name
    pub device_name: String,
    /// Device type (ios, android, desktop, etc.)
    pub device_type: Option<String>,
    /// Remote IP address
    pub remote_addr: Option<String>,
    /// Seconds remaining before expiry
    pub remaining_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initiate_and_confirm() {
        let manager = PairingManager::new();
        let code = manager.initiate_pairing("Test Device".to_string()).await;

        assert_eq!(code.len(), 6);

        let device_name = manager.confirm_pairing(&code).await;
        assert_eq!(device_name, Some("Test Device".to_string()));

        // Code should be consumed
        assert!(manager.confirm_pairing(&code).await.is_none());
    }

    #[tokio::test]
    async fn test_invalid_code() {
        let manager = PairingManager::new();
        assert!(manager.confirm_pairing("000000").await.is_none());
    }

    #[tokio::test]
    async fn test_expiry() {
        let manager = PairingManager::with_expiry(Duration::from_millis(10));
        let code = manager.initiate_pairing("Test".to_string()).await;

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(manager.confirm_pairing(&code).await.is_none());
    }

    #[tokio::test]
    async fn test_cancel() {
        let manager = PairingManager::new();
        let code = manager.initiate_pairing("Test".to_string()).await;

        assert!(manager.cancel_pairing(&code).await);
        assert!(manager.confirm_pairing(&code).await.is_none());
    }

    #[tokio::test]
    async fn test_list_pending() {
        let manager = PairingManager::new();
        manager.initiate_pairing("Device 1".to_string()).await;
        manager.initiate_pairing("Device 2".to_string()).await;

        let pending = manager.list_pending().await;
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn test_unique_codes() {
        let manager = PairingManager::new();
        let mut codes = Vec::new();

        for i in 0..100 {
            let code = manager.initiate_pairing(format!("Device {}", i)).await;
            assert!(!codes.contains(&code), "Duplicate code generated");
            codes.push(code);
        }
    }
}
