//! WorldModel State Definitions
//!
//! Three-layer state model:
//! - CoreState: KB-level, must be persisted
//! - EnhancedContext: Optional persistence, rebuilt on startup
//! - InferenceCache: In-memory only, not persisted

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// Import types from dispatcher module (implemented in Task 2)
use crate::daemon::dispatcher::policy::{ActionType, RiskLevel};
#[cfg(test)]
use crate::daemon::dispatcher::policy::NotificationPriority;

// =============================================================================
// Layer 1: CoreState (KB-level, must be persisted)
// =============================================================================

/// Core state that must be persisted across daemon restarts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreState {
    /// Current user activity type
    pub activity: ActivityType,

    /// Current session ID (if in a programming session)
    pub session_id: Option<String>,

    /// Pending actions list (for Reconciliation)
    pub pending_actions: Vec<PendingAction>,

    /// Last update timestamp
    pub last_updated: DateTime<Utc>,
}

impl Default for CoreState {
    fn default() -> Self {
        Self {
            activity: ActivityType::Idle,
            session_id: None,
            pending_actions: Vec::new(),
            last_updated: Utc::now(),
        }
    }
}

impl CoreState {
    /// Clean up expired pending actions
    pub fn prune_expired(&mut self) {
        let before_count = self.pending_actions.len();

        self.pending_actions.retain(|action| {
            !action.is_expired()
        });

        let removed_count = before_count - self.pending_actions.len();
        if removed_count > 0 {
            log::info!("Pruned {} expired pending actions", removed_count);
        }
    }
}

/// User activity types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityType {
    Idle,
    Programming {
        language: Option<String>,
        project: Option<String>,
    },
    Meeting {
        participants: usize,
    },
    Reading,
    Unknown,
}

/// Pending action waiting for user confirmation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingAction {
    pub action_type: ActionType,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub risk_level: RiskLevel,
}

impl PendingAction {
    /// Generate unique ID using SHA-256 hash (first 16 chars)
    pub fn id(&self) -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(format!("{:?}{}", self.action_type, self.created_at));
        let result = hasher.finalize();

        // Take first 16 characters of hex string
        format!("{:x}", result)[..16].to_string()
    }

    /// Check if action is expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }
}

// =============================================================================
// Layer 2: EnhancedContext (optional persistence, rebuilt on startup)
// =============================================================================

/// Enhanced context with runtime information
#[derive(Debug, Clone)]
pub struct EnhancedContext {
    /// Current project root directory
    pub project_root: Option<PathBuf>,

    /// Dominant programming language
    pub dominant_language: Option<String>,

    /// System resource constraints
    pub system_constraint: SystemLoad,

    /// Activity duration since last change
    pub activity_duration: Duration,
}

impl Default for EnhancedContext {
    fn default() -> Self {
        Self {
            project_root: None,
            dominant_language: None,
            system_constraint: SystemLoad::default(),
            activity_duration: Duration::zero(),
        }
    }
}

/// System resource load information
#[derive(Debug, Clone)]
pub struct SystemLoad {
    pub cpu_usage: f64,
    pub memory_pressure: MemoryPressure,
    pub battery_level: Option<u8>,
}

impl Default for SystemLoad {
    fn default() -> Self {
        Self {
            cpu_usage: 0.0,
            memory_pressure: MemoryPressure::Normal,
            battery_level: None,
        }
    }
}

/// Memory pressure levels
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryPressure {
    Normal,
    Warning,
    Critical,
}

// =============================================================================
// Layer 3: InferenceCache (in-memory only, not persisted)
// =============================================================================

/// In-memory inference cache
pub struct InferenceCache {
    /// Recent events circular buffer (last 100 events)
    pub recent_events: CircularBuffer<crate::daemon::events::DaemonEvent>,

    /// Unstable inference patterns (confidence < threshold)
    pub unstable_patterns: HashMap<String, ConfidenceScore>,

    /// High-frequency event counters
    pub event_counters: HashMap<String, Counter>,
}

impl InferenceCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            recent_events: CircularBuffer::new(capacity),
            unstable_patterns: HashMap::new(),
            event_counters: HashMap::new(),
        }
    }
}

/// Circular buffer for fixed-size event storage
pub struct CircularBuffer<T> {
    buffer: Vec<T>,
    capacity: usize,
    head: usize,
}

impl<T: Clone> CircularBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            capacity,
            head: 0,
        }
    }

    pub fn push(&mut self, item: T) {
        if self.buffer.len() < self.capacity {
            self.buffer.push(item);
        } else {
            self.buffer[self.head] = item;
            self.head = (self.head + 1) % self.capacity;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.buffer.iter()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// Confidence score for unstable patterns
#[derive(Debug, Clone)]
pub struct ConfidenceScore {
    pub value: f64,
    pub last_updated: DateTime<Utc>,
}

/// Event counter for batching
#[derive(Debug, Clone)]
pub struct Counter {
    pub count: usize,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

impl Counter {
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            count: 0,
            first_seen: now,
            last_seen: now,
        }
    }

    pub fn increment(&mut self) {
        self.count += 1;
        self.last_seen = Utc::now();
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_core_state_serialization() {
        // Create a CoreState with some data
        let state = CoreState {
            activity: ActivityType::Programming {
                language: Some("rust".to_string()),
                project: Some("/path/to/project".to_string()),
            },
            session_id: Some("session-123".to_string()),
            pending_actions: vec![PendingAction {
                action_type: ActionType::MuteSystemAudio,
                reason: "Test action".to_string(),
                created_at: Utc::now(),
                expires_at: Some(Utc::now() + Duration::hours(24)),
                risk_level: RiskLevel::Low,
            }],
            last_updated: Utc::now(),
        };

        // Serialize to JSON
        let json = serde_json::to_string(&state).expect("Failed to serialize");
        assert!(!json.is_empty());

        // Deserialize back
        let deserialized: CoreState =
            serde_json::from_str(&json).expect("Failed to deserialize");

        // Verify fields match
        assert_eq!(
            format!("{:?}", state.activity),
            format!("{:?}", deserialized.activity)
        );
        assert_eq!(state.session_id, deserialized.session_id);
        assert_eq!(state.pending_actions.len(), deserialized.pending_actions.len());
    }

    #[test]
    fn test_prune_expired_removes_old_actions() {
        let mut state = CoreState::default();

        // Add expired action
        state.pending_actions.push(PendingAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Expired action".to_string(),
            created_at: Utc::now() - Duration::hours(48),
            expires_at: Some(Utc::now() - Duration::hours(1)),
            risk_level: RiskLevel::Low,
        });

        // Add non-expired action
        state.pending_actions.push(PendingAction {
            action_type: ActionType::UnmuteSystemAudio,
            reason: "Valid action".to_string(),
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::hours(24)),
            risk_level: RiskLevel::Low,
        });

        // Add action with no expiry
        state.pending_actions.push(PendingAction {
            action_type: ActionType::NotifyUser {
                message: "No expiry".to_string(),
                priority: NotificationPriority::Low,
            },
            reason: "Persistent action".to_string(),
            created_at: Utc::now(),
            expires_at: None,
            risk_level: RiskLevel::Low,
        });

        assert_eq!(state.pending_actions.len(), 3);

        // Prune expired actions
        state.prune_expired();

        // Should have removed only the expired action
        assert_eq!(state.pending_actions.len(), 2);
        assert!(state.pending_actions.iter().all(|a| !a.is_expired()));
    }

    #[test]
    fn test_pending_action_id_generation() {
        let action1 = PendingAction {
            action_type: ActionType::MuteSystemAudio,
            reason: "Test".to_string(),
            created_at: Utc::now(),
            expires_at: None,
            risk_level: RiskLevel::Low,
        };

        let action2 = PendingAction {
            action_type: ActionType::UnmuteSystemAudio,
            reason: "Test".to_string(),
            created_at: action1.created_at,
            expires_at: None,
            risk_level: RiskLevel::Low,
        };

        // IDs should be different for different action types
        assert_ne!(action1.id(), action2.id());

        // ID should be 16 characters
        assert_eq!(action1.id().len(), 16);

        // ID should be consistent for same action
        assert_eq!(action1.id(), action1.id());
    }

    #[test]
    fn test_circular_buffer_push_and_iter() {
        let mut buffer = CircularBuffer::new(3);

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);

        // Add 3 items (under capacity)
        buffer.push(1);
        buffer.push(2);
        buffer.push(3);

        assert_eq!(buffer.len(), 3);
        let items: Vec<_> = buffer.iter().copied().collect();
        assert_eq!(items, vec![1, 2, 3]);

        // Add 4th item (should wrap around)
        buffer.push(4);
        assert_eq!(buffer.len(), 3);
        let items: Vec<_> = buffer.iter().copied().collect();
        assert_eq!(items, vec![4, 2, 3]);

        // Add 5th item
        buffer.push(5);
        assert_eq!(buffer.len(), 3);
        let items: Vec<_> = buffer.iter().copied().collect();
        assert_eq!(items, vec![4, 5, 3]);
    }

    #[test]
    fn test_activity_type_serialization() {
        let idle = ActivityType::Idle;
        let json = serde_json::to_string(&idle).unwrap();
        let deserialized: ActivityType = serde_json::from_str(&json).unwrap();
        assert_eq!(idle, deserialized);

        let programming = ActivityType::Programming {
            language: Some("rust".to_string()),
            project: Some("/test".to_string()),
        };
        let json = serde_json::to_string(&programming).unwrap();
        let deserialized: ActivityType = serde_json::from_str(&json).unwrap();
        assert_eq!(programming, deserialized);
    }

    #[test]
    fn test_counter_increment() {
        let mut counter = Counter::new();
        assert_eq!(counter.count, 0);

        counter.increment();
        assert_eq!(counter.count, 1);
        assert!(counter.last_seen >= counter.first_seen);

        counter.increment();
        assert_eq!(counter.count, 2);
    }

    #[test]
    fn test_enhanced_context_default() {
        let context = EnhancedContext::default();
        assert!(context.project_root.is_none());
        assert!(context.dominant_language.is_none());
        assert_eq!(context.system_constraint.cpu_usage, 0.0);
        assert_eq!(
            context.system_constraint.memory_pressure,
            MemoryPressure::Normal
        );
    }
}
