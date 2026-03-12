//! WorldModel Module (Minimal Stub)
//!
//! Provides state storage for CoreState and EnhancedContext.
//! Event processing, inference rules, batch processing, and persistence
//! have been removed as part of the minimal loop migration.

pub mod config;
pub mod state;

pub use config::WorldModelConfig;
pub use state::{
    ActivityType, CircularBuffer, ConfidenceScore, CoreState, Counter, EnhancedContext,
    InferenceCache, MemoryPressure, PendingAction, SystemLoad,
};

use crate::daemon::{DaemonEventBus, Result};
use crate::sync_primitives::Arc;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

/// Minimal WorldModel — holds state without event processing
pub struct WorldModel {
    state: Arc<RwLock<CoreState>>,
    context: Arc<RwLock<EnhancedContext>>,
    #[allow(dead_code)]
    config: WorldModelConfig,
}

impl WorldModel {
    /// Create a new WorldModel instance with default state
    pub async fn new(config: WorldModelConfig, _event_bus: Arc<DaemonEventBus>) -> Result<Self> {
        Ok(Self {
            state: Arc::new(RwLock::new(CoreState::default())),
            context: Arc::new(RwLock::new(EnhancedContext::default())),
            config,
        })
    }

    /// Get a read-only copy of CoreState
    pub async fn get_core_state(&self) -> CoreState {
        self.state.read().await.clone()
    }

    /// Get the state handle (Arc<RwLock<CoreState>>)
    pub fn get_state(&self) -> Arc<RwLock<CoreState>> {
        self.state.clone()
    }

    /// Get a read-only copy of EnhancedContext
    pub async fn get_context(&self) -> EnhancedContext {
        self.context.read().await.clone()
    }

    /// Alias used by some call sites
    pub async fn get_enhanced_context(&self) -> EnhancedContext {
        self.get_context().await
    }

    /// Add a pending action to CoreState
    pub async fn add_pending_action(&self, action: PendingAction) -> Result<()> {
        let mut state = self.state.write().await;
        state.pending_actions.push(action);
        state.last_updated = Utc::now();
        Ok(())
    }

    /// Remove a pending action by ID
    pub async fn remove_pending_action(&self, action_id: &str) -> Result<bool> {
        let mut state = self.state.write().await;
        let before_len = state.pending_actions.len();
        state.pending_actions.retain(|a| a.id() != action_id);
        let removed = state.pending_actions.len() < before_len;
        if removed {
            state.last_updated = Utc::now();
        }
        Ok(removed)
    }

    /// Update expiry time for a pending action
    pub async fn update_pending_action_expiry(
        &self,
        action_id: &str,
        new_expiry: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        let mut state = self.state.write().await;
        let mut updated = false;
        for action in &mut state.pending_actions {
            if action.id() == action_id {
                action.expires_at = new_expiry;
                updated = true;
                break;
            }
        }
        if updated {
            state.last_updated = Utc::now();
        }
        Ok(updated)
    }

    /// Query derived events within a time window (stub — always returns empty)
    pub async fn query_derived_events(
        &self,
        _since: DateTime<Utc>,
        _until: DateTime<Utc>,
    ) -> Vec<crate::daemon::events::DerivedEvent> {
        Vec::new()
    }

    /// Add event to cache (test helper — no-op in stub)
    #[cfg(test)]
    pub async fn add_event_to_cache(&self, _event: crate::daemon::DaemonEvent) {
        // no-op
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::dispatcher::policy::{ActionType, RiskLevel};

    async fn create_test_worldmodel() -> WorldModel {
        let config = WorldModelConfig::default();
        let event_bus = Arc::new(DaemonEventBus::new(100));
        WorldModel::new(config, event_bus).await.unwrap()
    }

    #[tokio::test]
    async fn test_worldmodel_new() {
        let wm = create_test_worldmodel().await;
        let state = wm.get_core_state().await;
        assert!(matches!(state.activity, ActivityType::Idle));
    }

    #[tokio::test]
    async fn test_add_pending_action() {
        let wm = create_test_worldmodel().await;

        let action = PendingAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Test action".to_string(),
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            risk_level: RiskLevel::Low,
        };

        wm.add_pending_action(action).await.unwrap();

        let state = wm.get_core_state().await;
        assert_eq!(state.pending_actions.len(), 1);
    }

    #[tokio::test]
    async fn test_get_context() {
        let wm = create_test_worldmodel().await;
        let context = wm.get_context().await;
        assert!(context.project_root.is_none());
        assert!(context.dominant_language.is_none());
    }
}
