//! WorldModel Module
//!
//! Phase 3: WorldModel - Cognitive State Management
//!
//! WorldModel is the "cognitive center" of Aether, responsible for:
//! - Subscribing to Raw Events from DaemonEventBus
//! - Inferring user activities, task contexts, and environmental constraints
//! - Publishing Derived Events to the Bus
//! - Maintaining and persisting CoreState
//!
//! Key Principle: WorldModel does inference only, not decision-making.
//! Decision-making is handled by the Dispatcher (Phase 4).

pub mod config;
pub mod persistence;
pub mod state;

pub use config::WorldModelConfig;
pub use persistence::StatePersistence;
pub use state::{
    ActivityType, CircularBuffer, ConfidenceScore, CoreState, Counter, EnhancedContext,
    InferenceCache, MemoryPressure, PendingAction, SystemLoad,
};

use crate::daemon::{DaemonEvent, DaemonEventBus, ProcessEventType, RawEvent, Result, SystemStateType};
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Duration;

/// WorldModel Framework - Event Processing and State Management
///
/// Implements three processing strategies:
/// 1. Immediate processing for key events (IDE start, display sleep)
/// 2. Batch processing for high-frequency events (every 5 seconds)
/// 3. Periodic inference safety net (every 30 seconds)
pub struct WorldModel {
    state: Arc<RwLock<CoreState>>,
    context: Arc<RwLock<EnhancedContext>>,
    #[allow(dead_code)] // Will be used in Task 5 for inference rules
    cache: Arc<Mutex<InferenceCache>>,
    event_bus: Arc<DaemonEventBus>,
    persistence: Arc<StatePersistence>,
    config: WorldModelConfig,
}

impl WorldModel {
    /// Create a new WorldModel instance
    ///
    /// # Arguments
    /// * `config` - WorldModel configuration
    /// * `event_bus` - Shared event bus for pub/sub
    ///
    /// # Returns
    /// A new WorldModel instance with state restored from persistence (if available)
    pub async fn new(config: WorldModelConfig, event_bus: Arc<DaemonEventBus>) -> Result<Self> {
        // Determine state path
        let state_path = config.state_path.clone().unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".aether/worldmodel_state.json")
        });

        let persistence = Arc::new(StatePersistence::new(state_path));

        // Restore CoreState from disk (or use default)
        let core_state = persistence.restore().await?;

        let worldmodel = Self {
            state: Arc::new(RwLock::new(core_state)),
            context: Arc::new(RwLock::new(EnhancedContext::default())),
            cache: Arc::new(Mutex::new(InferenceCache::new(config.cache_size))),
            event_bus,
            persistence,
            config,
        };

        Ok(worldmodel)
    }

    /// Main event loop with three processing strategies
    ///
    /// Runs forever, processing events using tokio::select!:
    /// - Strategy 1: Immediate processing for key events
    /// - Strategy 2: Batch processing every batch_interval seconds
    /// - Strategy 3: Periodic inference every periodic_interval seconds
    ///
    /// # Errors
    /// Returns error if event processing fails
    pub async fn run(&self) -> Result<()> {
        let mut rx = self.event_bus.subscribe();
        let mut batch_buffer: Vec<DaemonEvent> = Vec::new();
        let batch_interval = Duration::from_secs(self.config.batch_interval);
        let periodic_interval = Duration::from_secs(self.config.periodic_interval);

        log::info!("WorldModel event loop started");
        log::info!(
            "Batch interval: {}s, Periodic interval: {}s",
            self.config.batch_interval,
            self.config.periodic_interval
        );

        loop {
            tokio::select! {
                // Strategy 1: Immediate processing for key events
                Ok(event) = rx.recv() => {
                    if self.is_key_event(&event) {
                        log::debug!("Processing key event immediately: {:?}", event);
                        self.process_immediate(event).await?;
                    } else {
                        // Add to batch buffer for later processing
                        batch_buffer.push(event);
                    }
                }

                // Strategy 2: Batch processing every batch_interval seconds
                _ = tokio::time::sleep(batch_interval), if !batch_buffer.is_empty() => {
                    log::debug!("Processing batch of {} events", batch_buffer.len());
                    self.process_batch(&batch_buffer).await?;
                    batch_buffer.clear();
                }

                // Strategy 3: Periodic inference safety net
                _ = tokio::time::sleep(periodic_interval) => {
                    log::debug!("Running periodic inference");
                    self.periodic_inference().await?;
                }
            }
        }
    }

    /// Check if an event requires immediate processing
    ///
    /// Key events include:
    /// - IDE process starts (Code, Xcode)
    /// - Display sleep events
    fn is_key_event(&self, event: &DaemonEvent) -> bool {
        matches!(
            event,
            DaemonEvent::Raw(RawEvent::ProcessEvent {
                event_type: ProcessEventType::Started,
                name,
                ..
            }) if name.contains("Code") || name.contains("Xcode")
        ) || matches!(
            event,
            DaemonEvent::Raw(RawEvent::SystemStateEvent {
                state_type: SystemStateType::DisplaySleep,
                ..
            })
        )
    }

    /// Process a key event immediately (STUB - implemented in Task 5)
    ///
    /// Currently just logs the event. Full inference rules will be added in Task 5.
    async fn process_immediate(&self, event: DaemonEvent) -> Result<()> {
        log::info!("[STUB] process_immediate: {:?}", event);
        // TODO: Task 5 - Implement inference rules
        // - Rule 1: IDE start -> Programming activity
        // - Rule 2: Display sleep -> Idle activity
        Ok(())
    }

    /// Process a batch of events (STUB - implemented in Task 5)
    ///
    /// Currently just logs the batch size. Full inference rules will be added in Task 5.
    async fn process_batch(&self, events: &[DaemonEvent]) -> Result<()> {
        log::info!("[STUB] process_batch: {} events", events.len());
        // TODO: Task 5 - Implement inference rules
        // - Rule 3: File modification patterns -> Infer programming language
        // - Aggregate high-frequency events
        Ok(())
    }

    /// Run periodic inference safety net (STUB - implemented in Task 5)
    ///
    /// Currently just logs the execution. Full inference rules will be added in Task 5.
    async fn periodic_inference(&self) -> Result<()> {
        log::info!("[STUB] periodic_inference");
        // TODO: Task 5 - Implement inference rules
        // - Rule 4: Long idle time -> Possible meeting or away
        // - Check for state inconsistencies
        Ok(())
    }

    // =========================================================================
    // Accessor Methods (for Dispatcher to use)
    // =========================================================================

    /// Get a read-only copy of CoreState
    pub async fn get_core_state(&self) -> CoreState {
        self.state.read().await.clone()
    }

    /// Get a read-only copy of EnhancedContext
    pub async fn get_context(&self) -> EnhancedContext {
        self.context.read().await.clone()
    }

    /// Add a pending action to CoreState
    ///
    /// # Arguments
    /// * `action` - The pending action to add
    pub async fn add_pending_action(&self, action: PendingAction) -> Result<()> {
        let mut state = self.state.write().await;
        state.pending_actions.push(action);
        state.last_updated = Utc::now();
        self.persistence.save(&state).await?;
        Ok(())
    }

    /// Remove a pending action by ID
    ///
    /// # Arguments
    /// * `action_id` - The ID of the action to remove
    ///
    /// # Returns
    /// True if the action was found and removed, false otherwise
    pub async fn remove_pending_action(&self, action_id: &str) -> Result<bool> {
        let mut state = self.state.write().await;
        let before_len = state.pending_actions.len();
        state.pending_actions.retain(|a| a.id() != action_id);
        let removed = state.pending_actions.len() < before_len;

        if removed {
            state.last_updated = Utc::now();
            self.persistence.save(&state).await?;
        }

        Ok(removed)
    }

    /// Update expiry time for a pending action
    ///
    /// # Arguments
    /// * `action_id` - The ID of the action to update
    /// * `new_expiry` - New expiration time
    ///
    /// # Returns
    /// True if the action was found and updated, false otherwise
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
            self.persistence.save(&state).await?;
        }

        Ok(updated)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::dispatcher::policy::{ActionType, RiskLevel};
    use chrono::Duration;
    use tempfile::tempdir;

    async fn create_test_worldmodel() -> WorldModel {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("test_state.json");

        let config = WorldModelConfig {
            state_path: Some(state_path),
            batch_interval: 5,
            periodic_interval: 30,
            cache_size: 100,
            confidence_threshold: 0.7,
        };

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
    async fn test_is_key_event_ide_start() {
        let wm = create_test_worldmodel().await;

        // VS Code start event
        let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
            timestamp: Utc::now(),
            pid: 1234,
            name: "Code".to_string(),
            event_type: ProcessEventType::Started,
        });

        assert!(wm.is_key_event(&event));

        // Xcode start event
        let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
            timestamp: Utc::now(),
            pid: 5678,
            name: "Xcode".to_string(),
            event_type: ProcessEventType::Started,
        });

        assert!(wm.is_key_event(&event));
    }

    #[tokio::test]
    async fn test_is_key_event_display_sleep() {
        let wm = create_test_worldmodel().await;

        let event = DaemonEvent::Raw(RawEvent::SystemStateEvent {
            timestamp: Utc::now(),
            state_type: SystemStateType::DisplaySleep,
            old_value: Some(serde_json::json!(false)),
            new_value: serde_json::json!(true),
        });

        assert!(wm.is_key_event(&event));
    }

    #[tokio::test]
    async fn test_is_not_key_event() {
        let wm = create_test_worldmodel().await;

        // Non-IDE process
        let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
            timestamp: Utc::now(),
            pid: 1234,
            name: "Safari".to_string(),
            event_type: ProcessEventType::Started,
        });

        assert!(!wm.is_key_event(&event));

        // Non-display system event
        let event = DaemonEvent::Raw(RawEvent::SystemStateEvent {
            timestamp: Utc::now(),
            state_type: SystemStateType::BatteryLevel,
            old_value: Some(serde_json::json!(50)),
            new_value: serde_json::json!(40),
        });

        assert!(!wm.is_key_event(&event));
    }

    #[tokio::test]
    async fn test_add_pending_action() {
        let wm = create_test_worldmodel().await;

        let action = PendingAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Test action".to_string(),
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::hours(1)),
            risk_level: RiskLevel::Low,
        };

        wm.add_pending_action(action).await.unwrap();

        let state = wm.get_core_state().await;
        assert_eq!(state.pending_actions.len(), 1);
    }

    #[tokio::test]
    async fn test_remove_pending_action() {
        let wm = create_test_worldmodel().await;

        let action = PendingAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Test action".to_string(),
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::hours(1)),
            risk_level: RiskLevel::Low,
        };

        let action_id = action.id();
        wm.add_pending_action(action).await.unwrap();

        // Verify action was added
        let state = wm.get_core_state().await;
        assert_eq!(state.pending_actions.len(), 1);

        // Remove action
        let removed = wm.remove_pending_action(&action_id).await.unwrap();
        assert!(removed);

        // Verify action was removed
        let state = wm.get_core_state().await;
        assert_eq!(state.pending_actions.len(), 0);

        // Try to remove non-existent action
        let removed = wm.remove_pending_action("nonexistent").await.unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_update_pending_action_expiry() {
        let wm = create_test_worldmodel().await;

        let action = PendingAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Test action".to_string(),
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::hours(1)),
            risk_level: RiskLevel::Low,
        };

        let action_id = action.id();
        let new_expiry = Utc::now() + Duration::hours(24);

        wm.add_pending_action(action).await.unwrap();

        // Update expiry
        let updated = wm
            .update_pending_action_expiry(&action_id, Some(new_expiry))
            .await
            .unwrap();
        assert!(updated);

        // Verify update
        let state = wm.get_core_state().await;
        assert_eq!(state.pending_actions.len(), 1);
        assert_eq!(state.pending_actions[0].expires_at, Some(new_expiry));

        // Try to update non-existent action
        let updated = wm
            .update_pending_action_expiry("nonexistent", Some(new_expiry))
            .await
            .unwrap();
        assert!(!updated);
    }

    #[tokio::test]
    async fn test_get_context() {
        let wm = create_test_worldmodel().await;
        let context = wm.get_context().await;
        assert!(context.project_root.is_none());
        assert!(context.dominant_language.is_none());
    }

    #[tokio::test]
    async fn test_process_immediate_stub() {
        let wm = create_test_worldmodel().await;

        let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
            timestamp: Utc::now(),
            pid: 1234,
            name: "Code".to_string(),
            event_type: ProcessEventType::Started,
        });

        // Should not error, just log
        wm.process_immediate(event).await.unwrap();
    }

    #[tokio::test]
    async fn test_process_batch_stub() {
        let wm = create_test_worldmodel().await;

        let events = vec![
            DaemonEvent::Raw(RawEvent::Heartbeat {
                timestamp: Utc::now(),
            }),
            DaemonEvent::Raw(RawEvent::Heartbeat {
                timestamp: Utc::now(),
            }),
        ];

        // Should not error, just log
        wm.process_batch(&events).await.unwrap();
    }

    #[tokio::test]
    async fn test_periodic_inference_stub() {
        let wm = create_test_worldmodel().await;

        // Should not error, just log
        wm.periodic_inference().await.unwrap();
    }
}
