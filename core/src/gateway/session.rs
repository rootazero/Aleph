//! HTTP Session Management for Panel UI authentication.
//!
//! Sessions are created after successful shared token login.
//! Session IDs are stored in HttpOnly cookies.

use crate::gateway::security::SecurityStore;
use crate::sync_primitives::Arc;
use uuid::Uuid;

pub struct HttpSessionManager {
    store: Arc<SecurityStore>,
    expiry_hours: u64,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_used_at: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Storage error: {0}")]
    Storage(String),
}

impl HttpSessionManager {
    pub fn new(store: Arc<SecurityStore>, expiry_hours: u64) -> Self {
        Self {
            store,
            expiry_hours,
        }
    }

    pub fn expiry_hours(&self) -> u64 {
        self.expiry_hours
    }

    pub fn create_session(&self, token_hash: &str) -> Result<String, SessionError> {
        let session_id = Uuid::new_v4().to_string();
        let now = current_timestamp_ms();
        let expires_at = now + (self.expiry_hours as i64 * 3600 * 1000);

        self.store
            .insert_session(&session_id, token_hash, now, expires_at)
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        Ok(session_id)
    }

    pub fn validate_session(&self, session_id: &str) -> Result<bool, SessionError> {
        let valid = self
            .store
            .validate_session(session_id)
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        if valid {
            let _ = self.store.touch_session(session_id);
        }

        Ok(valid)
    }

    pub fn revoke_session(&self, session_id: &str) -> Result<(), SessionError> {
        self.store
            .delete_session(session_id)
            .map_err(|e| SessionError::Storage(e.to_string()))
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>, SessionError> {
        let rows = self
            .store
            .list_active_sessions()
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(
                |(session_id, created_at, expires_at, last_used_at)| SessionInfo {
                    session_id,
                    created_at,
                    expires_at,
                    last_used_at,
                },
            )
            .collect())
    }

    pub fn cleanup_expired(&self) -> Result<u64, SessionError> {
        self.store
            .delete_expired_sessions()
            .map_err(|e| SessionError::Storage(e.to_string()))
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
    use crate::gateway::security::SecurityStore;

    #[test]
    fn test_create_and_validate_session() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = HttpSessionManager::new(store, 72);
        let session_id = manager.create_session("test-hash").unwrap();
        assert!(!session_id.is_empty());
        assert!(manager.validate_session(&session_id).unwrap());
    }

    #[test]
    fn test_invalid_session_rejected() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = HttpSessionManager::new(store, 72);
        assert!(!manager.validate_session("nonexistent").unwrap());
    }

    #[test]
    fn test_revoke_session() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = HttpSessionManager::new(store, 72);
        let session_id = manager.create_session("test-hash").unwrap();
        assert!(manager.validate_session(&session_id).unwrap());
        manager.revoke_session(&session_id).unwrap();
        assert!(!manager.validate_session(&session_id).unwrap());
    }

    #[test]
    fn test_list_sessions() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = HttpSessionManager::new(store, 72);
        let _s1 = manager.create_session("hash1").unwrap();
        let _s2 = manager.create_session("hash2").unwrap();
        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_expired_session_invalid() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        // 0 hours = immediately expired
        let manager = HttpSessionManager::new(store, 0);
        let session_id = manager.create_session("test-hash").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(!manager.validate_session(&session_id).unwrap());
    }

    #[test]
    fn test_cleanup_expired() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = HttpSessionManager::new(store, 0);
        let _s1 = manager.create_session("hash1").unwrap();
        let _s2 = manager.create_session("hash2").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let count = manager.cleanup_expired().unwrap();
        assert_eq!(count, 2);
    }
}
