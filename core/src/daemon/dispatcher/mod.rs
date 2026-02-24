//! Dispatcher Module
//!
//! Phase 4: Dispatcher - Proactive Action Decision System

pub mod config;
pub mod executor;
pub mod mode;
pub mod policy;
pub mod policies;
pub mod scripting;
pub mod yaml_policy;

pub use config::DispatcherConfig;
pub use executor::ActionExecutor;
pub use mode::DispatcherMode;
pub use policy::{
    ActionType, NotificationPriority, Policy, PolicyEngine, ProposedAction, RiskLevel,
};

use crate::daemon::{DaemonEvent, DaemonEventBus, Result};
use crate::daemon::worldmodel::{PendingAction, WorldModel};
use chrono::{Duration, Utc};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Dispatcher - Main orchestrator for proactive actions
///
/// Subscribes to DerivedEvents from WorldModel, evaluates policies,
/// and routes actions based on risk level.
pub struct Dispatcher {
    mode: Arc<RwLock<DispatcherMode>>,
    policy_engine: Arc<PolicyEngine>,
    event_bus: Arc<DaemonEventBus>,
    worldmodel: Arc<WorldModel>,
    executor: Arc<ActionExecutor>,
    config: DispatcherConfig,
}

impl Dispatcher {
    /// Create a new Dispatcher instance
    pub fn new(
        config: DispatcherConfig,
        worldmodel: Arc<WorldModel>,
        event_bus: Arc<DaemonEventBus>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mode: Arc::new(RwLock::new(DispatcherMode::Running)),
            policy_engine: Arc::new(PolicyEngine::new_mvp()),
            event_bus,
            worldmodel,
            executor: ActionExecutor::new(),
            config,
        })
    }

    /// Main event loop
    ///
    /// Subscribes to DerivedEvents from the event bus and processes them
    /// through the policy engine.
    pub async fn run(&self) -> Result<()> {
        let mut rx = self.event_bus.subscribe();

        log::info!("Dispatcher main loop started");

        loop {
            let mode = self.mode.read().await.clone();

            tokio::select! {
                // Listen for Derived Events only
                Ok(event) = rx.recv() => {
                    // Only process Derived Events
                    let derived_event = match event {
                        DaemonEvent::Derived(e) => e,
                        _ => continue,
                    };

                    // Skip processing if in Reconciling mode
                    if !self.should_process_event(&mode) {
                        log::debug!("Skipping event in Reconciling mode: {:?}", derived_event);
                        continue;
                    }

                    // Get enhanced context from WorldModel
                    let context = self.worldmodel.get_context().await;

                    // Evaluate all policies
                    let proposed_actions = self.policy_engine.evaluate_all(&context, &derived_event);

                    if !proposed_actions.is_empty() {
                        log::info!("Policies proposed {} action(s)", proposed_actions.len());
                    }

                    // Handle each proposed action by risk level
                    for action in proposed_actions {
                        if let Err(e) = self.handle_action(action).await {
                            log::error!("Failed to handle action: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Handle a proposed action based on its risk level
    async fn handle_action(&self, action: ProposedAction) -> Result<()> {
        match action.risk_level {
            RiskLevel::Low => {
                // Low risk: Auto-execute immediately
                log::info!("Auto-executing low-risk action: {:?}", action.action_type);
                self.executor.execute(action).await?;
            }

            RiskLevel::Medium => {
                // Medium risk: Add to pending queue for lazy review
                log::info!("Queuing medium-risk action for lazy review: {:?}", action.action_type);

                // Convert ProposedAction to PendingAction
                let pending = self.to_pending_action(&action, RiskLevel::Medium);
                self.worldmodel.add_pending_action(pending).await?;

                // Spawn async task for delayed notification
                let _executor = self.executor.clone();
                let action_clone = action.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                    log::info!("Delayed notification for medium-risk action: {:?}", action_clone.action_type);
                    // TODO: Check if user is idle, then notify via Gateway IPC
                });
            }

            RiskLevel::High => {
                // High risk: Immediately switch to Reconciling mode
                log::warn!("High-risk action detected, entering Reconciling mode");

                // Convert to PendingAction and add to CoreState
                let pending = self.to_pending_action(&action, RiskLevel::High);
                self.worldmodel.add_pending_action(pending.clone()).await?;

                // Switch to Reconciling mode
                self.set_mode(DispatcherMode::Reconciling {
                    pending_high_risk: vec![pending],
                    started_at: Utc::now(),
                }).await;

                // Notify user urgently (placeholder for Gateway IPC)
                self.notify_urgent_action(action).await?;
            }
        }

        Ok(())
    }

    /// Convert ProposedAction to PendingAction with appropriate expiry
    fn to_pending_action(&self, action: &ProposedAction, risk_level: RiskLevel) -> PendingAction {
        let now = Utc::now();

        let expires_at = match risk_level {
            RiskLevel::Low => None, // No expiry for low-risk
            RiskLevel::Medium => {
                Some(now + Duration::hours(self.config.medium_risk_expiry_hours as i64))
            }
            RiskLevel::High => {
                Some(now + Duration::hours(self.config.high_risk_expiry_hours as i64))
            }
        };

        PendingAction {
            action_type: action.action_type.clone(),
            reason: action.reason.clone(),
            created_at: now,
            expires_at,
            risk_level: action.risk_level,
        }
    }

    /// Notify user of urgent high-risk action
    ///
    /// Placeholder for Gateway IPC integration
    async fn notify_urgent_action(&self, action: ProposedAction) -> Result<()> {
        log::warn!("IPC: Urgent action notification: {:?}", action);
        log::warn!("  Reason: {}", action.reason);
        log::warn!("  Risk: {:?}", action.risk_level);
        // TODO: Integrate with Gateway IPC mechanism
        Ok(())
    }

    /// Set dispatcher mode
    pub async fn set_mode(&self, new_mode: DispatcherMode) {
        let mut mode = self.mode.write().await;
        let old_mode = mode.clone();
        *mode = new_mode.clone();

        log::info!("Dispatcher mode transition: {:?} -> {:?}", old_mode, new_mode);
    }

    /// Get current dispatcher mode
    pub async fn get_mode(&self) -> DispatcherMode {
        self.mode.read().await.clone()
    }

    /// Check if dispatcher should process events in current mode
    fn should_process_event(&self, mode: &DispatcherMode) -> bool {
        matches!(mode, DispatcherMode::Running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::worldmodel::{ActivityType, WorldModelConfig};
    use crate::daemon::DerivedEvent;
    use std::collections::HashMap;
    use tempfile::tempdir;

    async fn create_test_dispatcher() -> (Arc<Dispatcher>, Arc<WorldModel>, Arc<DaemonEventBus>) {
        let dir = tempdir().unwrap();
        let state_path = dir.path().join("test_state.json");

        let worldmodel_config = WorldModelConfig {
            state_path: Some(state_path),
            batch_interval: 5,
            periodic_interval: 30,
            cache_size: 100,
            confidence_threshold: 0.7,
        };

        let event_bus = Arc::new(DaemonEventBus::new(100));
        let worldmodel = Arc::new(WorldModel::new(worldmodel_config, event_bus.clone()).await.unwrap());

        let dispatcher_config = DispatcherConfig::default();
        let dispatcher = Dispatcher::new(dispatcher_config, worldmodel.clone(), event_bus.clone());

        (dispatcher, worldmodel, event_bus)
    }

    #[tokio::test]
    async fn test_dispatcher_new() {
        let (dispatcher, _, _) = create_test_dispatcher().await;
        let mode = dispatcher.get_mode().await;
        assert!(matches!(mode, DispatcherMode::Running));
    }

    #[tokio::test]
    async fn test_dispatcher_set_mode() {
        let (dispatcher, _, _) = create_test_dispatcher().await;

        dispatcher.set_mode(DispatcherMode::Reconciling {
            pending_high_risk: vec![],
            started_at: Utc::now(),
        }).await;

        let mode = dispatcher.get_mode().await;
        assert!(matches!(mode, DispatcherMode::Reconciling { .. }));
    }

    #[tokio::test]
    async fn test_should_process_event_running() {
        let (dispatcher, _, _) = create_test_dispatcher().await;
        let mode = DispatcherMode::Running;
        assert!(dispatcher.should_process_event(&mode));
    }

    #[tokio::test]
    async fn test_should_process_event_reconciling() {
        let (dispatcher, _, _) = create_test_dispatcher().await;
        let mode = DispatcherMode::Reconciling {
            pending_high_risk: vec![],
            started_at: Utc::now(),
        };
        assert!(!dispatcher.should_process_event(&mode));
    }

    #[tokio::test]
    async fn test_handle_low_risk_action() {
        let (dispatcher, _, _) = create_test_dispatcher().await;

        let action = ProposedAction {
            action_type: ActionType::NotifyUser {
                message: "Test".to_string(),
                priority: NotificationPriority::Low,
            },
            reason: "Test low risk".to_string(),
            risk_level: RiskLevel::Low,
            metadata: HashMap::new(),
        };

        let result = dispatcher.handle_action(action).await;
        assert!(result.is_ok());

        // Mode should still be Running
        let mode = dispatcher.get_mode().await;
        assert!(matches!(mode, DispatcherMode::Running));
    }

    #[tokio::test]
    async fn test_handle_medium_risk_action() {
        let (dispatcher, worldmodel, _) = create_test_dispatcher().await;

        let action = ProposedAction {
            action_type: ActionType::EnableDoNotDisturb,
            reason: "Test medium risk".to_string(),
            risk_level: RiskLevel::Medium,
            metadata: HashMap::new(),
        };

        let result = dispatcher.handle_action(action).await;
        assert!(result.is_ok());

        // Should be added to pending actions
        let state = worldmodel.get_core_state().await;
        assert_eq!(state.pending_actions.len(), 1);

        // Mode should still be Running
        let mode = dispatcher.get_mode().await;
        assert!(matches!(mode, DispatcherMode::Running));
    }

    #[tokio::test]
    async fn test_handle_high_risk_action() {
        let (dispatcher, worldmodel, _) = create_test_dispatcher().await;

        let action = ProposedAction {
            action_type: ActionType::AdjustBrightness { level: 10 },
            reason: "Test high risk".to_string(),
            risk_level: RiskLevel::High,
            metadata: HashMap::new(),
        };

        let result = dispatcher.handle_action(action).await;
        assert!(result.is_ok());

        // Should be added to pending actions
        let state = worldmodel.get_core_state().await;
        assert_eq!(state.pending_actions.len(), 1);

        // Mode should transition to Reconciling
        let mode = dispatcher.get_mode().await;
        assert!(matches!(mode, DispatcherMode::Reconciling { .. }));
    }

    #[tokio::test]
    async fn test_to_pending_action_low_risk() {
        let (dispatcher, _, _) = create_test_dispatcher().await;

        let action = ProposedAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Test".to_string(),
            risk_level: RiskLevel::Low,
            metadata: HashMap::new(),
        };

        let pending = dispatcher.to_pending_action(&action, RiskLevel::Low);
        assert!(pending.expires_at.is_none());
        assert_eq!(pending.risk_level, RiskLevel::Low);
    }

    #[tokio::test]
    async fn test_to_pending_action_medium_risk() {
        let (dispatcher, _, _) = create_test_dispatcher().await;

        let action = ProposedAction {
            action_type: ActionType::EnableDoNotDisturb,
            reason: "Test".to_string(),
            risk_level: RiskLevel::Medium,
            metadata: HashMap::new(),
        };

        let pending = dispatcher.to_pending_action(&action, RiskLevel::Medium);
        assert!(pending.expires_at.is_some());

        // Should expire in ~12 hours (default config)
        let expiry_hours = (pending.expires_at.unwrap() - Utc::now()).num_hours();
        assert!((11..=12).contains(&expiry_hours));
    }

    #[tokio::test]
    async fn test_to_pending_action_high_risk() {
        let (dispatcher, _, _) = create_test_dispatcher().await;

        let action = ProposedAction {
            action_type: ActionType::AdjustBrightness { level: 10 },
            reason: "Test".to_string(),
            risk_level: RiskLevel::High,
            metadata: HashMap::new(),
        };

        let pending = dispatcher.to_pending_action(&action, RiskLevel::High);
        assert!(pending.expires_at.is_some());

        // Should expire in ~24 hours (default config)
        let expiry_hours = (pending.expires_at.unwrap() - Utc::now()).num_hours();
        assert!((23..=24).contains(&expiry_hours));
    }

    #[tokio::test]
    async fn test_policy_evaluation_integration() {
        let (dispatcher, _worldmodel, event_bus) = create_test_dispatcher().await;

        // Subscribe to the event bus first (needed to avoid "No active receivers" error)
        let _rx = event_bus.subscribe();

        // Simulate a meeting start event (should trigger MeetingMutePolicy)
        let event = DaemonEvent::Derived(DerivedEvent::ActivityChanged {
            timestamp: Utc::now(),
            old_activity: ActivityType::Idle,
            new_activity: ActivityType::Meeting { participants: 5 },
            confidence: 0.95,
        });

        // Send the event
        event_bus.send(event).unwrap();

        // Give dispatcher time to process (in real usage, dispatcher.run() would be running)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify mode is still Running (Low risk auto-executes)
        let mode = dispatcher.get_mode().await;
        assert!(matches!(mode, DispatcherMode::Running));
    }
}
