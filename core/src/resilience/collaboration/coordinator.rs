//! Session Coordinator
//!
//! Manages the lifecycle of subagent sessions, including:
//! - Session creation and handle allocation
//! - Idle state management
//! - Automatic swapping decisions
//! - Handle reuse patterns

use crate::error::AlephError;
use crate::resilience::{SessionStatus, SubagentSession};
use crate::resilience::database::StateDatabase;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::handle::SessionHandle;
use super::swapping::{SwapConfig, SwapManager, SwapResult};

/// Configuration for the Session Coordinator
#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    /// Maximum concurrent active sessions
    pub max_active_sessions: usize,

    /// Maximum idle sessions in memory
    pub max_idle_sessions: usize,

    /// Enable automatic swapping
    pub enable_auto_swap: bool,

    /// Swap configuration
    pub swap_config: SwapConfig,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            max_active_sessions: 5,
            max_idle_sessions: 10,
            enable_auto_swap: true,
            swap_config: SwapConfig::default(),
        }
    }
}

/// Session Coordinator for managing subagent lifecycle
///
/// The coordinator provides a centralized point for:
/// - Creating new sessions
/// - Acquiring handles for existing sessions
/// - Managing session state transitions
/// - Automatic resource optimization (swapping)
pub struct SessionCoordinator {
    db: Arc<StateDatabase>,
    config: CoordinatorConfig,
    swap_manager: Arc<SwapManager>,

    /// Active session handles
    handles: RwLock<HashMap<String, Arc<SessionHandle>>>,
}

impl SessionCoordinator {
    /// Create a new Session Coordinator
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self::with_config(db, CoordinatorConfig::default())
    }

    /// Create a Session Coordinator with custom config
    pub fn with_config(db: Arc<StateDatabase>, config: CoordinatorConfig) -> Self {
        let swap_manager = Arc::new(SwapManager::with_config(
            db.clone(),
            Arc::new(crate::resilience::recovery::ShadowReplayEngine::new(
                db.clone(),
            )),
            config.swap_config.clone(),
        ));

        Self {
            db,
            config,
            swap_manager,
            handles: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new subagent session
    ///
    /// Returns a handle for controlling the session.
    pub async fn create_session(
        &self,
        agent_type: &str,
        parent_session_id: &str,
    ) -> Result<Arc<SessionHandle>, AlephError> {
        // Check if we're at capacity
        let active_count = self
            .db
            .count_sessions_by_status(SessionStatus::Active)
            .await?;

        if active_count >= self.config.max_active_sessions as u64 {
            return Err(AlephError::config(format!(
                "Maximum active sessions reached: {}",
                self.config.max_active_sessions
            )));
        }

        // Create session in database
        let session_id = uuid::Uuid::new_v4().to_string();
        let session = SubagentSession::new(&session_id, agent_type, parent_session_id);

        self.db.insert_session(&session).await?;

        // Create handle
        let handle = Arc::new(SessionHandle::new(session_id.clone(), self.db.clone()));

        // Store handle
        let mut handles = self.handles.write().await;
        handles.insert(session_id.clone(), handle.clone());

        info!(
            session_id = %session_id,
            agent_type = %agent_type,
            parent = %parent_session_id,
            "Created new subagent session"
        );

        // Check swap pressure
        if self.config.enable_auto_swap {
            self.maybe_swap().await;
        }

        Ok(handle)
    }

    /// Get a handle for an existing session
    ///
    /// If the session is swapped, it will be restored first.
    pub async fn get_handle(&self, session_id: &str) -> Result<Arc<SessionHandle>, AlephError> {
        // Check if we already have the handle
        {
            let handles = self.handles.read().await;
            if let Some(handle) = handles.get(session_id) {
                return Ok(handle.clone());
            }
        }

        // Get session from database
        let session = self
            .db
            .get_session(session_id)
            .await?
            .ok_or_else(|| AlephError::config(format!("Session not found: {}", session_id)))?;

        // If swapped, restore first
        if session.status == SessionStatus::Swapped {
            self.swap_manager.swap_in(session_id).await?;
        }

        // Create handle
        let handle = Arc::new(SessionHandle::new(session_id.to_string(), self.db.clone()));

        // Store handle
        let mut handles = self.handles.write().await;
        handles.insert(session_id.to_string(), handle.clone());

        Ok(handle)
    }

    /// Find or create a session of the given type
    ///
    /// Prefers reusing idle sessions of the same type.
    pub async fn acquire_session(
        &self,
        agent_type: &str,
        parent_session_id: &str,
    ) -> Result<Arc<SessionHandle>, AlephError> {
        // Look for idle sessions of the same type
        let idle_sessions = self.db.get_idle_sessions(100).await?;

        for session in idle_sessions {
            if session.agent_type == agent_type && session.parent_session_id == parent_session_id {
                info!(
                    session_id = %session.id,
                    agent_type = %agent_type,
                    "Reusing idle session"
                );
                return self.get_handle(&session.id).await;
            }
        }

        // No suitable idle session found, create new one
        self.create_session(agent_type, parent_session_id).await
    }

    /// Release a session (mark as idle)
    pub async fn release_session(&self, session_id: &str) -> Result<(), AlephError> {
        self.db
            .update_session_status(session_id, SessionStatus::Idle, None)
            .await?;

        debug!(session_id = %session_id, "Session released to idle");

        // Check swap pressure
        if self.config.enable_auto_swap {
            self.maybe_swap().await;
        }

        Ok(())
    }

    /// Close a session and remove its handle
    pub async fn close_session(&self, session_id: &str) -> Result<(), AlephError> {
        // Remove handle
        let mut handles = self.handles.write().await;
        if let Some(handle) = handles.remove(session_id) {
            handle.close().await?;
        } else {
            // Just delete from database
            self.db.delete_session(session_id).await?;
        }

        info!(session_id = %session_id, "Session closed");
        Ok(())
    }

    /// Get all active handles
    pub async fn active_handles(&self) -> Vec<Arc<SessionHandle>> {
        let handles = self.handles.read().await;
        handles.values().cloned().collect()
    }

    /// Get session count by status
    pub async fn get_session_counts(&self) -> Result<SessionCounts, AlephError> {
        let active = self
            .db
            .count_sessions_by_status(SessionStatus::Active)
            .await?;
        let idle = self
            .db
            .count_sessions_by_status(SessionStatus::Idle)
            .await?;
        let swapped = self
            .db
            .count_sessions_by_status(SessionStatus::Swapped)
            .await?;

        Ok(SessionCounts {
            active: active as usize,
            idle: idle as usize,
            swapped: swapped as usize,
        })
    }

    /// Trigger swap if under pressure
    async fn maybe_swap(&self) {
        match self.swap_manager.auto_swap().await {
            Ok(result) => {
                if result.swapped_out > 0 {
                    info!(
                        swapped = result.swapped_out,
                        bytes = result.swap_size_bytes,
                        "Auto-swapped idle sessions"
                    );
                }
            }
            Err(e) => {
                warn!(error = %e, "Auto-swap failed");
            }
        }
    }

    /// Force swap all eligible idle sessions
    pub async fn force_swap(&self) -> Result<SwapResult, AlephError> {
        self.swap_manager.auto_swap().await
    }

    /// Get the swap manager for direct access
    pub fn swap_manager(&self) -> &Arc<SwapManager> {
        &self.swap_manager
    }
}

/// Session counts by status
#[derive(Debug, Clone)]
pub struct SessionCounts {
    pub active: usize,
    pub idle: usize,
    pub swapped: usize,
}

impl SessionCounts {
    /// Total sessions across all states
    pub fn total(&self) -> usize {
        self.active + self.idle + self.swapped
    }
}

impl std::fmt::Debug for SessionCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCoordinator")
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coordinator_config_default() {
        let config = CoordinatorConfig::default();
        assert_eq!(config.max_active_sessions, 5);
        assert_eq!(config.max_idle_sessions, 10);
        assert!(config.enable_auto_swap);
    }

    #[test]
    fn test_session_counts() {
        let counts = SessionCounts {
            active: 3,
            idle: 5,
            swapped: 2,
        };
        assert_eq!(counts.total(), 10);
    }
}
