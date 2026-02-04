//! WorldModel State Persistence
//!
//! Provides JSON-based persistence for CoreState, enabling graceful daemon
//! restarts without losing user activity state or pending actions.
//!
//! Features:
//! - Pretty-printed JSON for easy debugging
//! - Auto-creates parent directories
//! - Handles missing files gracefully (returns default state)
//! - Automatic cleanup of expired pending actions on restore

use anyhow::Result;
use std::path::PathBuf;
use tokio::fs;
use crate::daemon::worldmodel::state::CoreState;

/// Persistence layer for CoreState
pub struct StatePersistence {
    db_path: PathBuf,
}

impl StatePersistence {
    /// Create a new StatePersistence with the given file path
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    /// Save CoreState to disk as pretty-printed JSON
    ///
    /// Automatically creates parent directories if they don't exist.
    pub async fn save(&self, state: &CoreState) -> Result<()> {
        let json = serde_json::to_string_pretty(state)?;

        // Ensure parent directory exists
        if let Some(parent) = self.db_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(&self.db_path, json).await?;
        Ok(())
    }

    /// Restore CoreState from disk
    ///
    /// Returns default state if file doesn't exist.
    /// Automatically prunes expired pending actions after deserialization.
    pub async fn restore(&self) -> Result<CoreState> {
        if !self.db_path.exists() {
            return Ok(CoreState::default());
        }

        let json = fs::read_to_string(&self.db_path).await?;
        let mut state: CoreState = serde_json::from_str(&json)?;

        state.prune_expired();

        Ok(state)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::worldmodel::state::ActivityType;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_save_and_restore() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let persistence = StatePersistence::new(path.clone());

        let state = CoreState {
            activity: ActivityType::Programming {
                language: Some("Rust".into()),
                project: None,
            },
            session_id: Some("test".into()),
            pending_actions: vec![],
            last_updated: chrono::Utc::now(),
        };

        persistence.save(&state).await.unwrap();
        let restored = persistence.restore().await.unwrap();

        assert_eq!(state.activity, restored.activity);
        assert_eq!(state.session_id, restored.session_id);
    }

    #[tokio::test]
    async fn test_restore_nonexistent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let persistence = StatePersistence::new(path);

        let state = persistence.restore().await.unwrap();
        assert!(matches!(state.activity, ActivityType::Idle));
    }

    #[tokio::test]
    async fn test_restore_prunes_expired_actions() {
        use crate::daemon::dispatcher::policy::{ActionType, NotificationPriority, RiskLevel};
        use crate::daemon::worldmodel::state::PendingAction;
        use chrono::{Duration, Utc};

        let dir = tempdir().unwrap();
        let path = dir.path().join("state_with_expired.json");
        let persistence = StatePersistence::new(path.clone());

        // Create state with expired and valid actions
        let state = CoreState {
            activity: ActivityType::Programming {
                language: Some("Rust".into()),
                project: None,
            },
            session_id: Some("test".into()),
            pending_actions: vec![
                PendingAction {
                    action_type: ActionType::NotifyUser {
                        message: "Expired action".into(),
                        priority: NotificationPriority::Low,
                    },
                    reason: "Test expired".into(),
                    created_at: Utc::now() - Duration::hours(48),
                    expires_at: Some(Utc::now() - Duration::hours(1)),
                    risk_level: RiskLevel::Low,
                },
                PendingAction {
                    action_type: ActionType::NotifyUser {
                        message: "Valid action".into(),
                        priority: NotificationPriority::Normal,
                    },
                    reason: "Test valid".into(),
                    created_at: Utc::now(),
                    expires_at: Some(Utc::now() + Duration::hours(24)),
                    risk_level: RiskLevel::Low,
                },
            ],
            last_updated: Utc::now(),
        };

        // Save state with expired action
        persistence.save(&state).await.unwrap();

        // Restore should prune expired actions
        let restored = persistence.restore().await.unwrap();

        assert_eq!(restored.pending_actions.len(), 1);
        assert!(restored.pending_actions[0].reason == "Test valid");
    }

    #[tokio::test]
    async fn test_save_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/deep/state.json");
        let persistence = StatePersistence::new(path.clone());

        let state = CoreState::default();
        persistence.save(&state).await.unwrap();

        assert!(path.exists());
    }
}
