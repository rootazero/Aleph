//! WorldModel Module
//!
//! Phase 3: WorldModel - Cognitive State Management
//!
//! WorldModel is the "cognitive center" of Aleph, responsible for:
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
use std::collections::HashMap;
use std::path::PathBuf;
use crate::sync_primitives::Arc;
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
            PathBuf::from(home).join(".aleph/worldmodel_state.json")
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

        let mut batch_timer = tokio::time::interval(batch_interval);
        let mut periodic_timer = tokio::time::interval(periodic_interval);

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
                _ = batch_timer.tick(), if !batch_buffer.is_empty() => {
                    log::debug!("Processing batch of {} events", batch_buffer.len());
                    self.process_batch(&batch_buffer).await?;
                    batch_buffer.clear();
                }

                // Strategy 3: Periodic inference safety net
                _ = periodic_timer.tick() => {
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

    /// Process a key event immediately
    ///
    /// Implements immediate inference rules:
    /// - Rule 1: IDE start (Code/Xcode) -> Programming activity
    /// - Rule 2: Display sleep -> Idle activity
    async fn process_immediate(&self, event: DaemonEvent) -> Result<()> {
        use crate::daemon::DerivedEvent;

        let mut state = self.state.write().await;

        match event {
            // Rule 1: IDE startup -> Programming activity
            DaemonEvent::Raw(RawEvent::ProcessEvent {
                event_type: ProcessEventType::Started,
                name,
                timestamp,
                ..
            }) if name.contains("Code") || name.contains("Xcode") => {
                log::info!("Rule 1: IDE started ({}), transitioning to Programming", name);

                let old_activity = state.activity.clone();
                state.activity = ActivityType::Programming {
                    language: None,
                    project: None,
                };
                state.last_updated = Utc::now();

                // Create derived event
                let derived_event = DaemonEvent::Derived(DerivedEvent::ActivityChanged {
                    timestamp,
                    old_activity,
                    new_activity: state.activity.clone(),
                    confidence: 0.95,
                });

                // Store in cache
                let mut cache = self.cache.lock().await;
                cache.recent_events.push(derived_event.clone());
                drop(cache);

                // Publish DerivedEvent
                self.event_bus.send(derived_event)?;

                // Persist state change
                self.persistence.save(&state).await?;
            }

            // Rule 2: Display sleep -> Idle activity
            DaemonEvent::Raw(RawEvent::SystemStateEvent {
                state_type: SystemStateType::DisplaySleep,
                new_value,
                timestamp,
                ..
            }) if new_value.as_bool() == Some(true) => {
                log::info!("Rule 2: Display sleep detected, transitioning to Idle");

                let old_activity = state.activity.clone();
                state.activity = ActivityType::Idle;
                state.last_updated = Utc::now();

                // Create derived event
                let derived_event = DaemonEvent::Derived(DerivedEvent::ActivityChanged {
                    timestamp,
                    old_activity,
                    new_activity: ActivityType::Idle,
                    confidence: 1.0,
                });

                // Store in cache
                let mut cache = self.cache.lock().await;
                cache.recent_events.push(derived_event.clone());
                drop(cache);

                // Publish DerivedEvent
                self.event_bus.send(derived_event)?;

                // Persist state change
                self.persistence.save(&state).await?;
            }

            _ => {
                log::debug!("process_immediate: unhandled key event {:?}", event);
            }
        }

        Ok(())
    }

    /// Process a batch of events
    ///
    /// Implements batch inference rules:
    /// - Rule 3: File modification patterns -> Infer programming language
    async fn process_batch(&self, events: &[DaemonEvent]) -> Result<()> {
        log::debug!("process_batch: {} events", events.len());

        // Rule 3: Analyze file modification patterns to infer programming language
        let fs_events: Vec<&PathBuf> = events
            .iter()
            .filter_map(|e| match e {
                DaemonEvent::Raw(RawEvent::FsEvent { path, .. }) => Some(path),
                _ => None,
            })
            .collect();

        if !fs_events.is_empty() {
            log::debug!("Rule 3: Analyzing {} file events", fs_events.len());

            if let Some(language) = self.infer_language(&fs_events) {
                log::info!(
                    "Rule 3: Inferred dominant language: {}",
                    language
                );

                // Acquire state lock first, then context lock (consistent ordering
                // with periodic_inference to prevent deadlock).
                let mut state = self.state.write().await;
                let mut context = self.context.write().await;
                context.dominant_language = Some(language.clone());
                if let ActivityType::Programming {
                    language: ref mut lang,
                    ..
                } = state.activity
                {
                    *lang = Some(language);
                    state.last_updated = Utc::now();
                    self.persistence.save(&state).await?;
                }
            }
        }

        Ok(())
    }

    /// Run periodic inference safety net
    ///
    /// Implements periodic inference rules:
    /// - Rule 4: Long idle time -> IdleStateChanged event
    /// - Rule 5: Resource pressure changes -> ResourcePressureChanged event
    async fn periodic_inference(&self) -> Result<()> {
        use crate::daemon::{DerivedEvent, PressureType};

        log::debug!("Running periodic inference");

        let state = self.state.read().await;
        let mut context = self.context.write().await;

        // Update activity duration
        let duration_since_update = Utc::now().signed_duration_since(state.last_updated);
        context.activity_duration = duration_since_update;

        // Rule 4: Long idle detection
        if state.activity == ActivityType::Idle && duration_since_update.num_minutes() > 5 {
            log::info!(
                "Rule 4: Long idle detected ({} minutes)",
                duration_since_update.num_minutes()
            );

            let derived_event = DaemonEvent::Derived(DerivedEvent::IdleStateChanged {
                timestamp: Utc::now(),
                is_idle: true,
                idle_duration: Some(duration_since_update),
            });

            // Store in cache
            let mut cache = self.cache.lock().await;
            cache.recent_events.push(derived_event.clone());
            drop(cache);

            self.event_bus.send(derived_event)?;
        }

        // Rule 5: Resource pressure detection
        let old_cpu_level = self.pressure_level_from_cpu(context.system_constraint.cpu_usage);
        let old_battery_level =
            self.pressure_level_from_battery(context.system_constraint.battery_level);

        // For MVP, we just check if levels would cross thresholds
        // In production, this would read actual system metrics
        let new_cpu = context.system_constraint.cpu_usage; // Would be updated from system
        let new_cpu_level = self.pressure_level_from_cpu(new_cpu);

        if old_cpu_level != new_cpu_level {
            log::info!(
                "Rule 5: CPU pressure changed from {:?} to {:?}",
                old_cpu_level,
                new_cpu_level
            );

            let derived_event = DaemonEvent::Derived(
                DerivedEvent::ResourcePressureChanged {
                    timestamp: Utc::now(),
                    pressure_type: PressureType::Cpu,
                    old_level: old_cpu_level,
                    new_level: new_cpu_level,
                },
            );

            // Store in cache
            let mut cache = self.cache.lock().await;
            cache.recent_events.push(derived_event.clone());
            drop(cache);

            self.event_bus.send(derived_event)?;
        }

        if let Some(battery) = context.system_constraint.battery_level {
            let new_battery_level = self.pressure_level_from_battery(Some(battery));

            if old_battery_level != new_battery_level {
                log::info!(
                    "Rule 5: Battery pressure changed from {:?} to {:?}",
                    old_battery_level,
                    new_battery_level
                );

                let derived_event = DaemonEvent::Derived(
                    DerivedEvent::ResourcePressureChanged {
                        timestamp: Utc::now(),
                        pressure_type: PressureType::Battery,
                        old_level: old_battery_level,
                        new_level: new_battery_level,
                    },
                );

                // Store in cache
                let mut cache = self.cache.lock().await;
                cache.recent_events.push(derived_event.clone());
                drop(cache);

                self.event_bus.send(derived_event)?;
            }
        }

        Ok(())
    }

    // =========================================================================
    // Helper Methods for Inference Rules
    // =========================================================================

    /// Infer programming language from file paths
    ///
    /// Simple pattern matching on file extensions for MVP.
    /// Returns the most common language detected, or None if no patterns found.
    fn infer_language(&self, paths: &[&PathBuf]) -> Option<String> {
        let mut language_counts: HashMap<String, usize> = HashMap::new();

        for path in paths {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let language = match ext {
                    "rs" => "Rust",
                    "ts" | "tsx" => "TypeScript",
                    "js" | "jsx" => "JavaScript",
                    "py" => "Python",
                    "go" => "Go",
                    "java" => "Java",
                    "c" | "h" => "C",
                    "cpp" | "hpp" | "cc" => "C++",
                    "swift" => "Swift",
                    "kt" => "Kotlin",
                    "rb" => "Ruby",
                    "php" => "PHP",
                    _ => continue,
                };

                *language_counts.entry(language.to_string()).or_insert(0) += 1;
            }
        }

        // Return the most common language
        language_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(lang, _)| lang)
    }

    /// Convert CPU usage to pressure level
    ///
    /// Thresholds:
    /// - Normal: < 70%
    /// - Warning: 70-90%
    /// - Critical: > 90%
    fn pressure_level_from_cpu(&self, cpu_usage: f64) -> crate::daemon::PressureLevel {
        use crate::daemon::PressureLevel;

        if cpu_usage > 90.0 {
            PressureLevel::Critical
        } else if cpu_usage > 70.0 {
            PressureLevel::Warning
        } else {
            PressureLevel::Normal
        }
    }

    /// Convert battery level to pressure level
    ///
    /// Thresholds:
    /// - Normal: > 20%
    /// - Warning: 10-20%
    /// - Critical: < 10%
    fn pressure_level_from_battery(&self, battery: Option<u8>) -> crate::daemon::PressureLevel {
        use crate::daemon::PressureLevel;

        match battery {
            Some(level) if level < 10 => PressureLevel::Critical,
            Some(level) if level < 20 => PressureLevel::Warning,
            _ => PressureLevel::Normal,
        }
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

    /// Query derived events within a time window
    ///
    /// For MVP, returns events from InferenceCache's recent_events buffer.
    /// Phase 5.2 will add persistent event log.
    ///
    /// # Arguments
    /// * `since` - Start of time window
    /// * `until` - End of time window
    ///
    /// # Returns
    /// Vector of DerivedEvents within the time window
    pub async fn query_derived_events(
        &self,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Vec<crate::daemon::events::DerivedEvent> {
        

        let cache = self.cache.lock().await;

        // Filter events from circular buffer
        cache.recent_events
            .iter()
            .filter_map(|e| {
                // Extract DerivedEvent from DaemonEvent::Derived variant
                if let DaemonEvent::Derived(derived) = e {
                    let ts = Self::event_timestamp(derived);
                    if ts >= since && ts <= until {
                        Some(derived.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    /// Extract timestamp from a DerivedEvent
    fn event_timestamp(event: &crate::daemon::events::DerivedEvent) -> DateTime<Utc> {
        use crate::daemon::events::DerivedEvent;

        match event {
            DerivedEvent::ActivityChanged { timestamp, .. } => *timestamp,
            DerivedEvent::ProgrammingSessionStarted { timestamp, .. } => *timestamp,
            DerivedEvent::ProgrammingSessionEnded { timestamp, .. } => *timestamp,
            DerivedEvent::ResourcePressureChanged { timestamp, .. } => *timestamp,
            DerivedEvent::MeetingStateChanged { timestamp, .. } => *timestamp,
            DerivedEvent::IdleStateChanged { timestamp, .. } => *timestamp,
            DerivedEvent::Aggregated { timestamp, .. } => *timestamp,
        }
    }

    /// Add event to cache (for testing purposes)
    #[cfg(test)]
    pub async fn add_event_to_cache(&self, event: DaemonEvent) {
        let mut cache = self.cache.lock().await;
        cache.recent_events.push(event);
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
    async fn test_rule1_ide_start_to_programming() {
        let wm = create_test_worldmodel().await;

        // Subscribe to event bus BEFORE processing
        let _rx = wm.event_bus.subscribe();

        // Initially should be Idle
        let state = wm.get_core_state().await;
        assert!(matches!(state.activity, ActivityType::Idle));

        // Trigger IDE start event
        let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
            timestamp: Utc::now(),
            pid: 1234,
            name: "Code".to_string(),
            event_type: ProcessEventType::Started,
        });

        wm.process_immediate(event).await.unwrap();

        // Should transition to Programming
        let state = wm.get_core_state().await;
        assert!(matches!(
            state.activity,
            ActivityType::Programming { .. }
        ));
    }

    #[tokio::test]
    async fn test_rule1_xcode_start_to_programming() {
        let wm = create_test_worldmodel().await;

        // Subscribe to event bus BEFORE processing
        let _rx = wm.event_bus.subscribe();

        let event = DaemonEvent::Raw(RawEvent::ProcessEvent {
            timestamp: Utc::now(),
            pid: 5678,
            name: "Xcode".to_string(),
            event_type: ProcessEventType::Started,
        });

        wm.process_immediate(event).await.unwrap();

        let state = wm.get_core_state().await;
        assert!(matches!(
            state.activity,
            ActivityType::Programming { .. }
        ));
    }

    #[tokio::test]
    async fn test_rule2_display_sleep_to_idle() {
        let wm = create_test_worldmodel().await;

        // Subscribe to event bus BEFORE processing
        let _rx = wm.event_bus.subscribe();

        // First set to Programming
        let mut state = wm.state.write().await;
        state.activity = ActivityType::Programming {
            language: Some("Rust".to_string()),
            project: None,
        };
        drop(state);

        // Trigger display sleep event
        let event = DaemonEvent::Raw(RawEvent::SystemStateEvent {
            timestamp: Utc::now(),
            state_type: SystemStateType::DisplaySleep,
            old_value: Some(serde_json::json!(false)),
            new_value: serde_json::json!(true),
        });

        wm.process_immediate(event).await.unwrap();

        // Should transition to Idle
        let state = wm.get_core_state().await;
        assert!(matches!(state.activity, ActivityType::Idle));
    }

    #[tokio::test]
    async fn test_rule3_infer_language_from_files() {
        let wm = create_test_worldmodel().await;

        // Set activity to Programming
        let mut state = wm.state.write().await;
        state.activity = ActivityType::Programming {
            language: None,
            project: None,
        };
        drop(state);

        // Create batch of file modification events
        let events = vec![
            DaemonEvent::Raw(RawEvent::FsEvent {
                timestamp: Utc::now(),
                path: PathBuf::from("/project/main.rs"),
                event_type: crate::daemon::events::FsEventType::Modified,
            }),
            DaemonEvent::Raw(RawEvent::FsEvent {
                timestamp: Utc::now(),
                path: PathBuf::from("/project/lib.rs"),
                event_type: crate::daemon::events::FsEventType::Modified,
            }),
            DaemonEvent::Raw(RawEvent::FsEvent {
                timestamp: Utc::now(),
                path: PathBuf::from("/project/utils.rs"),
                event_type: crate::daemon::events::FsEventType::Modified,
            }),
        ];

        wm.process_batch(&events).await.unwrap();

        // Should have inferred Rust
        let context = wm.get_context().await;
        assert_eq!(context.dominant_language, Some("Rust".to_string()));

        // Activity should now have language set
        let state = wm.get_core_state().await;
        if let ActivityType::Programming { language, .. } = state.activity {
            assert_eq!(language, Some("Rust".to_string()));
        } else {
            panic!("Activity should be Programming");
        }
    }

    #[tokio::test]
    async fn test_infer_language_multiple_languages() {
        let wm = create_test_worldmodel().await;

        let paths = [
            PathBuf::from("file1.rs"),
            PathBuf::from("file2.rs"),
            PathBuf::from("file3.rs"),
            PathBuf::from("file4.py"),
            PathBuf::from("file5.py"),
        ];
        let path_refs: Vec<&PathBuf> = paths.iter().collect();

        // Rust should win (3 vs 2)
        let language = wm.infer_language(&path_refs);
        assert_eq!(language, Some("Rust".to_string()));
    }

    #[tokio::test]
    async fn test_infer_language_typescript() {
        let wm = create_test_worldmodel().await;

        let paths = [PathBuf::from("app.ts"), PathBuf::from("component.tsx")];
        let path_refs: Vec<&PathBuf> = paths.iter().collect();

        let language = wm.infer_language(&path_refs);
        assert_eq!(language, Some("TypeScript".to_string()));
    }

    #[tokio::test]
    async fn test_rule4_long_idle_detection() {
        let wm = create_test_worldmodel().await;

        // Subscribe to event bus BEFORE processing
        let _rx = wm.event_bus.subscribe();

        // Set activity to Idle and backdating last_updated
        let mut state = wm.state.write().await;
        state.activity = ActivityType::Idle;
        state.last_updated = Utc::now() - chrono::Duration::minutes(10);
        drop(state);

        // Run periodic inference
        wm.periodic_inference().await.unwrap();

        // Check that activity_duration was updated
        let context = wm.get_context().await;
        assert!(context.activity_duration.num_minutes() >= 10);
    }

    #[tokio::test]
    async fn test_rule5_cpu_pressure_levels() {
        let wm = create_test_worldmodel().await;

        // Test CPU thresholds
        assert_eq!(
            wm.pressure_level_from_cpu(50.0),
            crate::daemon::PressureLevel::Normal
        );
        assert_eq!(
            wm.pressure_level_from_cpu(75.0),
            crate::daemon::PressureLevel::Warning
        );
        assert_eq!(
            wm.pressure_level_from_cpu(95.0),
            crate::daemon::PressureLevel::Critical
        );
    }

    #[tokio::test]
    async fn test_rule5_battery_pressure_levels() {
        let wm = create_test_worldmodel().await;

        // Test battery thresholds
        assert_eq!(
            wm.pressure_level_from_battery(Some(50)),
            crate::daemon::PressureLevel::Normal
        );
        assert_eq!(
            wm.pressure_level_from_battery(Some(15)),
            crate::daemon::PressureLevel::Warning
        );
        assert_eq!(
            wm.pressure_level_from_battery(Some(5)),
            crate::daemon::PressureLevel::Critical
        );
        assert_eq!(
            wm.pressure_level_from_battery(None),
            crate::daemon::PressureLevel::Normal
        );
    }
}
