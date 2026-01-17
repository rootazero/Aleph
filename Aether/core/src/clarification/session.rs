//! Clarification Session Management
//!
//! This module provides session management for parameter collection workflows.
//! It tracks pending clarification requests and manages their lifecycle including
//! creation, retrieval, completion, and expiration cleanup.
//!
//! # Example
//!
//! ```rust,no_run
//! use aethecore::clarification::session::{ClarificationManager, SessionConfig};
//!
//! # async fn example() {
//! let config = SessionConfig::default();
//! let manager = ClarificationManager::new(config);
//!
//! // Create a session for missing parameter
//! let session = manager.create_session(
//!     "location",
//!     "weather_search",
//!     "weather_tool",
//!     "what's the weather",
//! ).await;
//!
//! // Later, complete the session when user provides input
//! let completed = manager.complete_session(&session.session_id).await;
//! # }
//! ```

use super::{ClarificationOption, ClarificationRequest};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

/// A pending clarification session
///
/// Represents an active session where we're waiting for user input
/// to complete a parameter collection.
#[derive(Debug, Clone)]
pub struct PendingClarification {
    /// Unique session identifier (UUID v4)
    pub session_id: String,

    /// Name of the parameter being requested
    pub param_name: String,

    /// Intent type that requires this parameter
    pub intent_type: String,

    /// Tool name that needs the parameter
    pub tool_name: String,

    /// When this session was created
    pub created_at: Instant,

    /// Session timeout duration
    pub timeout: Duration,

    /// Original user input that triggered this clarification
    pub original_input: String,
}

impl PendingClarification {
    /// Create a new pending clarification session
    ///
    /// # Arguments
    ///
    /// * `param_name` - Name of the parameter being requested
    /// * `intent_type` - Intent type requiring this parameter
    /// * `tool_name` - Tool that needs the parameter
    /// * `original_input` - Original user input
    /// * `timeout_secs` - Timeout in seconds
    pub fn new(
        param_name: impl Into<String>,
        intent_type: impl Into<String>,
        tool_name: impl Into<String>,
        original_input: impl Into<String>,
        timeout_secs: u64,
    ) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            param_name: param_name.into(),
            intent_type: intent_type.into(),
            tool_name: tool_name.into(),
            created_at: Instant::now(),
            timeout: Duration::from_secs(timeout_secs),
            original_input: original_input.into(),
        }
    }

    /// Check if this session has expired
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.timeout
    }

    /// Get remaining time before expiration
    ///
    /// Returns Duration::ZERO if already expired
    pub fn remaining_time(&self) -> Duration {
        self.timeout.saturating_sub(self.created_at.elapsed())
    }

    /// Convert to a ClarificationRequest for UI display
    ///
    /// # Arguments
    ///
    /// * `prompt` - The prompt text to display to user
    /// * `options` - Options for select-type clarification
    pub fn to_request(&self, prompt: &str, options: Vec<ClarificationOption>) -> ClarificationRequest {
        if options.is_empty() {
            ClarificationRequest::text(&self.session_id, prompt, None)
                .with_source(&format!("intent:{}", self.intent_type))
        } else {
            ClarificationRequest::select(&self.session_id, prompt, options)
                .with_source(&format!("intent:{}", self.intent_type))
        }
    }
}

/// Configuration for clarification session management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Default timeout for sessions in seconds
    #[serde(default = "default_timeout_secs")]
    pub default_timeout_secs: u64,

    /// Maximum number of concurrent sessions
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,

    /// Interval for cleanup task in seconds
    #[serde(default = "default_cleanup_interval_secs")]
    pub cleanup_interval_secs: u64,
}

fn default_timeout_secs() -> u64 {
    60
}

fn default_max_sessions() -> usize {
    10
}

fn default_cleanup_interval_secs() -> u64 {
    30
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: default_timeout_secs(),
            max_sessions: default_max_sessions(),
            cleanup_interval_secs: default_cleanup_interval_secs(),
        }
    }
}

/// Manager for clarification sessions
///
/// Handles the lifecycle of clarification sessions including creation,
/// retrieval, completion, and expiration cleanup.
#[derive(Clone)]
pub struct ClarificationManager {
    /// Active sessions indexed by session_id
    sessions: Arc<RwLock<HashMap<String, PendingClarification>>>,

    /// Configuration
    config: SessionConfig,
}

impl ClarificationManager {
    /// Create a new clarification manager
    pub fn new(config: SessionConfig) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Create a new clarification session
    ///
    /// # Arguments
    ///
    /// * `param_name` - Name of the parameter being requested
    /// * `intent_type` - Intent type requiring this parameter
    /// * `tool_name` - Tool that needs the parameter
    /// * `original_input` - Original user input
    ///
    /// # Returns
    ///
    /// The newly created PendingClarification session
    pub async fn create_session(
        &self,
        param_name: impl Into<String>,
        intent_type: impl Into<String>,
        tool_name: impl Into<String>,
        original_input: impl Into<String>,
    ) -> PendingClarification {
        let session = PendingClarification::new(
            param_name,
            intent_type,
            tool_name,
            original_input,
            self.config.default_timeout_secs,
        );

        let session_id = session.session_id.clone();
        let session_clone = session.clone();

        {
            let mut sessions = self.sessions.write().await;

            // Enforce max sessions limit by removing oldest if needed
            while sessions.len() >= self.config.max_sessions {
                if let Some(oldest_id) = self.find_oldest_session(&sessions) {
                    sessions.remove(&oldest_id);
                } else {
                    break;
                }
            }

            sessions.insert(session_id, session_clone);
        }

        session
    }

    /// Get a session by ID
    ///
    /// Returns None if session doesn't exist or has expired
    pub async fn get_session(&self, session_id: &str) -> Option<PendingClarification> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).and_then(|s| {
            if s.is_expired() {
                None
            } else {
                Some(s.clone())
            }
        })
    }

    /// Complete and remove a session
    ///
    /// Returns the session if it existed (even if expired)
    pub async fn complete_session(&self, session_id: &str) -> Option<PendingClarification> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id)
    }

    /// Cleanup all expired sessions
    ///
    /// Returns the number of sessions that were removed
    pub async fn cleanup_expired(&self) -> usize {
        let mut sessions = self.sessions.write().await;
        let expired_ids: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.is_expired())
            .map(|(id, _)| id.clone())
            .collect();

        let count = expired_ids.len();
        for id in expired_ids {
            sessions.remove(&id);
        }

        count
    }

    /// Get count of active (non-expired) sessions
    pub async fn active_session_count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.values().filter(|s| !s.is_expired()).count()
    }

    /// Find the oldest session by creation time
    fn find_oldest_session(&self, sessions: &HashMap<String, PendingClarification>) -> Option<String> {
        sessions
            .iter()
            .min_by_key(|(_, s)| s.created_at)
            .map(|(id, _)| id.clone())
    }

    /// Get the configuration
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_pending_clarification_new() {
        let session = PendingClarification::new(
            "location",
            "weather_search",
            "weather_tool",
            "what's the weather",
            60,
        );

        assert!(!session.session_id.is_empty());
        assert_eq!(session.param_name, "location");
        assert_eq!(session.intent_type, "weather_search");
        assert_eq!(session.tool_name, "weather_tool");
        assert_eq!(session.original_input, "what's the weather");
        assert_eq!(session.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_pending_clarification_uuid_uniqueness() {
        let session1 = PendingClarification::new("p1", "i1", "t1", "input1", 60);
        let session2 = PendingClarification::new("p2", "i2", "t2", "input2", 60);

        assert_ne!(session1.session_id, session2.session_id);
    }

    #[test]
    fn test_pending_clarification_is_expired() {
        let session = PendingClarification::new("param", "intent", "tool", "input", 0);

        // With 0 second timeout, should be expired immediately
        std::thread::sleep(Duration::from_millis(10));
        assert!(session.is_expired());
    }

    #[test]
    fn test_pending_clarification_not_expired() {
        let session = PendingClarification::new("param", "intent", "tool", "input", 60);

        assert!(!session.is_expired());
    }

    #[test]
    fn test_pending_clarification_remaining_time() {
        let session = PendingClarification::new("param", "intent", "tool", "input", 60);

        let remaining = session.remaining_time();
        assert!(remaining.as_secs() <= 60);
        assert!(remaining.as_secs() >= 59); // Should be close to 60
    }

    #[test]
    fn test_pending_clarification_remaining_time_expired() {
        let session = PendingClarification::new("param", "intent", "tool", "input", 0);

        std::thread::sleep(Duration::from_millis(10));
        let remaining = session.remaining_time();
        assert_eq!(remaining, Duration::ZERO);
    }

    #[test]
    fn test_pending_clarification_to_request_with_options() {
        let session = PendingClarification::new(
            "location",
            "weather_search",
            "weather_tool",
            "what's the weather",
            60,
        );

        let options = vec![
            ClarificationOption::new("beijing", "Beijing"),
            ClarificationOption::new("shanghai", "Shanghai"),
        ];

        let request = session.to_request("Select a location:", options);

        assert_eq!(request.id, session.session_id);
        assert_eq!(request.prompt, "Select a location:");
        assert!(request.options.is_some());
        assert_eq!(request.options.as_ref().unwrap().len(), 2);
        assert_eq!(request.source, Some("intent:weather_search".to_string()));
    }

    #[test]
    fn test_pending_clarification_to_request_text() {
        let session = PendingClarification::new(
            "location",
            "weather_search",
            "weather_tool",
            "what's the weather",
            60,
        );

        let request = session.to_request("Enter a location:", vec![]);

        assert_eq!(request.id, session.session_id);
        assert_eq!(request.prompt, "Enter a location:");
        assert!(request.options.is_none());
        assert_eq!(request.source, Some("intent:weather_search".to_string()));
    }

    #[test]
    fn test_session_config_default() {
        let config = SessionConfig::default();

        assert_eq!(config.default_timeout_secs, 60);
        assert_eq!(config.max_sessions, 10);
        assert_eq!(config.cleanup_interval_secs, 30);
    }

    #[test]
    fn test_session_config_serialize_deserialize() {
        let config = SessionConfig {
            default_timeout_secs: 120,
            max_sessions: 20,
            cleanup_interval_secs: 60,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SessionConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.default_timeout_secs, 120);
        assert_eq!(deserialized.max_sessions, 20);
        assert_eq!(deserialized.cleanup_interval_secs, 60);
    }

    #[tokio::test]
    async fn test_clarification_manager_new() {
        let config = SessionConfig::default();
        let manager = ClarificationManager::new(config);

        assert_eq!(manager.active_session_count().await, 0);
    }

    #[tokio::test]
    async fn test_clarification_manager_create_session() {
        let config = SessionConfig::default();
        let manager = ClarificationManager::new(config);

        let session = manager
            .create_session("location", "weather_search", "weather_tool", "weather input")
            .await;

        assert_eq!(session.param_name, "location");
        assert_eq!(session.intent_type, "weather_search");
        assert_eq!(session.tool_name, "weather_tool");
        assert_eq!(session.original_input, "weather input");
        assert_eq!(manager.active_session_count().await, 1);
    }

    #[tokio::test]
    async fn test_clarification_manager_get_session() {
        let config = SessionConfig::default();
        let manager = ClarificationManager::new(config);

        let session = manager
            .create_session("param", "intent", "tool", "input")
            .await;

        let retrieved = manager.get_session(&session.session_id).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().session_id, session.session_id);
    }

    #[tokio::test]
    async fn test_clarification_manager_get_session_not_found() {
        let config = SessionConfig::default();
        let manager = ClarificationManager::new(config);

        let retrieved = manager.get_session("non-existent-id").await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_clarification_manager_get_session_expired() {
        let config = SessionConfig {
            default_timeout_secs: 0, // Immediate expiration
            ..Default::default()
        };
        let manager = ClarificationManager::new(config);

        let session = manager
            .create_session("param", "intent", "tool", "input")
            .await;

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        let retrieved = manager.get_session(&session.session_id).await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_clarification_manager_complete_session() {
        let config = SessionConfig::default();
        let manager = ClarificationManager::new(config);

        let session = manager
            .create_session("param", "intent", "tool", "input")
            .await;
        let session_id = session.session_id.clone();

        assert_eq!(manager.active_session_count().await, 1);

        let completed = manager.complete_session(&session_id).await;
        assert!(completed.is_some());
        assert_eq!(completed.unwrap().session_id, session_id);
        assert_eq!(manager.active_session_count().await, 0);
    }

    #[tokio::test]
    async fn test_clarification_manager_complete_session_not_found() {
        let config = SessionConfig::default();
        let manager = ClarificationManager::new(config);

        let completed = manager.complete_session("non-existent-id").await;
        assert!(completed.is_none());
    }

    #[tokio::test]
    async fn test_clarification_manager_cleanup_expired() {
        let config = SessionConfig {
            default_timeout_secs: 0, // Immediate expiration
            max_sessions: 10,
            ..Default::default()
        };
        let manager = ClarificationManager::new(config);

        // Create multiple sessions
        manager
            .create_session("param1", "intent1", "tool1", "input1")
            .await;
        manager
            .create_session("param2", "intent2", "tool2", "input2")
            .await;
        manager
            .create_session("param3", "intent3", "tool3", "input3")
            .await;

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        let cleaned = manager.cleanup_expired().await;
        assert_eq!(cleaned, 3);
        assert_eq!(manager.active_session_count().await, 0);
    }

    #[tokio::test]
    async fn test_clarification_manager_max_sessions_enforcement() {
        let config = SessionConfig {
            default_timeout_secs: 60,
            max_sessions: 2,
            ..Default::default()
        };
        let manager = ClarificationManager::new(config);

        // Create sessions up to and beyond limit
        let _session1 = manager
            .create_session("param1", "intent1", "tool1", "input1")
            .await;
        let _session2 = manager
            .create_session("param2", "intent2", "tool2", "input2")
            .await;
        let session3 = manager
            .create_session("param3", "intent3", "tool3", "input3")
            .await;

        // Should still only have max_sessions
        assert_eq!(manager.active_session_count().await, 2);

        // The newest session should still exist
        let retrieved = manager.get_session(&session3.session_id).await;
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_clarification_manager_clone() {
        let config = SessionConfig::default();
        let manager = ClarificationManager::new(config);

        let session = manager
            .create_session("param", "intent", "tool", "input")
            .await;

        // Clone the manager
        let manager_clone = manager.clone();

        // Both should see the same session
        assert!(manager.get_session(&session.session_id).await.is_some());
        assert!(manager_clone.get_session(&session.session_id).await.is_some());

        // Completing via clone should affect original
        manager_clone.complete_session(&session.session_id).await;
        assert!(manager.get_session(&session.session_id).await.is_none());
    }

    #[tokio::test]
    async fn test_clarification_manager_concurrent_access() {
        let config = SessionConfig::default();
        let manager = ClarificationManager::new(config);

        // Spawn multiple tasks that create sessions concurrently
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let manager = manager.clone();
                tokio::spawn(async move {
                    manager
                        .create_session(
                            format!("param{}", i),
                            format!("intent{}", i),
                            format!("tool{}", i),
                            format!("input{}", i),
                        )
                        .await
                })
            })
            .collect();

        // Wait for all to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // All sessions should be created (up to max)
        assert_eq!(manager.active_session_count().await, 10);
    }

    #[test]
    fn test_session_config_with_custom_values() {
        let config = SessionConfig {
            default_timeout_secs: 300,
            max_sessions: 50,
            cleanup_interval_secs: 120,
        };

        assert_eq!(config.default_timeout_secs, 300);
        assert_eq!(config.max_sessions, 50);
        assert_eq!(config.cleanup_interval_secs, 120);
    }

    #[tokio::test]
    async fn test_clarification_manager_get_config() {
        let config = SessionConfig {
            default_timeout_secs: 90,
            max_sessions: 15,
            cleanup_interval_secs: 45,
        };
        let manager = ClarificationManager::new(config);

        let retrieved_config = manager.config();
        assert_eq!(retrieved_config.default_timeout_secs, 90);
        assert_eq!(retrieved_config.max_sessions, 15);
        assert_eq!(retrieved_config.cleanup_interval_secs, 45);
    }
}
