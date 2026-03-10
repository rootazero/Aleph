//! POE Domain Events
//!
//! Event types for the POE (Principle-Operation-Evaluation) bounded context.
//! Every significant state change in a POE lifecycle is captured as an immutable
//! `PoeEvent` wrapped in a `PoeEventEnvelope`.
//!
//! ## Skeleton vs Pulse
//!
//! Events follow the Skeleton/Pulse classification from the resilience layer:
//! - **Skeleton** -- structural mutations that must be persisted immediately
//!   (ManifestCreated, ContractSigned, ValidationCompleted, OutcomeRecorded)
//! - **Pulse** -- high-frequency observations that may be buffered before persist
//!   (OperationAttempted, TrustUpdated)

use serde::{Deserialize, Serialize};

// ============================================================================
// PoeOutcomeKind -- outcome classification
// ============================================================================

/// Classification of a POE task outcome.
///
/// Used inside `OutcomeRecorded` events and maps to a satisfaction score
/// for experience crystallization.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PoeOutcomeKind {
    /// Task completed successfully, all hard constraints passed.
    Success,
    /// Task hit a dead-end; suggests switching strategy.
    StrategySwitch,
    /// Token/attempt budget exhausted without success.
    BudgetExhausted,
    /// Task needs decomposition into sub-tasks (Phase 2).
    DecompositionRequired,
}

impl PoeOutcomeKind {
    /// Convert outcome kind to a satisfaction score for experience crystallization.
    ///
    /// - `Success` = 1.0 (full satisfaction)
    /// - `StrategySwitch` = 0.3 (partial -- learned something)
    /// - `BudgetExhausted` = 0.0 (no satisfaction)
    pub fn to_satisfaction(self) -> f32 {
        match self {
            PoeOutcomeKind::Success => 1.0,
            PoeOutcomeKind::StrategySwitch => 0.3,
            PoeOutcomeKind::BudgetExhausted => 0.0,
            PoeOutcomeKind::DecompositionRequired => 0.5,
        }
    }
}

// ============================================================================
// EventTier -- Skeleton vs Pulse
// ============================================================================

/// Classification of event persistence urgency.
///
/// - `Skeleton` events must be persisted immediately (structural mutations).
/// - `Pulse` events may be buffered before persist (high-frequency observations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventTier {
    /// Must be persisted immediately.
    Skeleton,
    /// May be buffered before persist.
    Pulse,
}

// ============================================================================
// PoeEvent -- the domain event enum
// ============================================================================

/// Domain events for the POE bounded context.
///
/// Every significant state change during a POE lifecycle is captured as one of
/// these variants. The enum is internally tagged with `"type"` for deterministic
/// serialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PoeEvent {
    // ------------------------------------------------------------------
    // Skeleton events (immediate persist)
    // ------------------------------------------------------------------
    /// A success manifest was created for a task.
    ManifestCreated {
        task_id: String,
        objective: String,
        hard_constraints_count: usize,
        soft_metrics_count: usize,
    },

    /// A contract was signed (auto-approved or user-approved).
    ContractSigned {
        task_id: String,
        auto_approved: bool,
        trust_score: Option<f32>,
    },

    /// A validation pass completed for a task attempt.
    ValidationCompleted {
        task_id: String,
        attempt: u8,
        passed: bool,
        distance_score: f32,
        hard_passed: usize,
        hard_total: usize,
    },

    /// Final outcome recorded for a POE task.
    OutcomeRecorded {
        task_id: String,
        outcome: PoeOutcomeKind,
        attempts: u8,
        total_tokens: u32,
        duration_ms: u64,
        best_distance: f32,
    },

    // ------------------------------------------------------------------
    // Pulse events (buffered persist)
    // ------------------------------------------------------------------
    /// An operation attempt was made (token accounting).
    OperationAttempted {
        task_id: String,
        attempt: u8,
        tokens_used: u32,
    },

    /// Trust score was updated for a pattern.
    TrustUpdated {
        pattern_id: String,
        old_score: f32,
        new_score: f32,
    },
}

impl PoeEvent {
    /// Return the serde tag string for this event variant.
    ///
    /// Matches the `#[serde(tag = "type")]` discriminant so callers can
    /// filter events by type without deserializing the full payload.
    pub fn event_type_tag(&self) -> &'static str {
        match self {
            PoeEvent::ManifestCreated { .. } => "ManifestCreated",
            PoeEvent::ContractSigned { .. } => "ContractSigned",
            PoeEvent::ValidationCompleted { .. } => "ValidationCompleted",
            PoeEvent::OutcomeRecorded { .. } => "OutcomeRecorded",
            PoeEvent::OperationAttempted { .. } => "OperationAttempted",
            PoeEvent::TrustUpdated { .. } => "TrustUpdated",
        }
    }

    /// Whether this event is a Skeleton event (must be persisted immediately).
    ///
    /// Only `OperationAttempted` and `TrustUpdated` are Pulse (buffered).
    /// All other variants are Skeleton.
    pub fn is_skeleton(&self) -> bool {
        !matches!(
            self,
            PoeEvent::OperationAttempted { .. } | PoeEvent::TrustUpdated { .. }
        )
    }

    /// Extract the task_id from any event variant that has one.
    ///
    /// Returns `None` for `TrustUpdated` which uses `pattern_id` instead.
    pub fn task_id(&self) -> Option<&str> {
        match self {
            PoeEvent::ManifestCreated { task_id, .. }
            | PoeEvent::ContractSigned { task_id, .. }
            | PoeEvent::ValidationCompleted { task_id, .. }
            | PoeEvent::OutcomeRecorded { task_id, .. }
            | PoeEvent::OperationAttempted { task_id, .. } => Some(task_id),
            PoeEvent::TrustUpdated { .. } => None,
        }
    }
}

// ============================================================================
// PoeEventEnvelope -- metadata wrapper
// ============================================================================

/// Immutable envelope wrapping a `PoeEvent` with metadata.
///
/// Stored as a single row in the event store (SQLite). The `id` field
/// is the SQLite auto-increment primary key (0 before insert, assigned
/// on write). The `seq` field provides per-task monotonic ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoeEventEnvelope {
    /// Auto-increment global ID (assigned by SQLite on insert; 0 before insert).
    pub id: i64,
    /// The task this event belongs to.
    pub task_id: String,
    /// Per-task monotonic sequence number.
    pub seq: u32,
    /// The domain event payload.
    pub event: PoeEvent,
    /// Skeleton or Pulse classification.
    pub tier: EventTier,
    /// When the event occurred (Unix timestamp, milliseconds).
    pub timestamp: i64,
    /// Optional correlation to a session or parent task.
    pub correlation_id: Option<String>,
}

impl PoeEventEnvelope {
    /// Build a new envelope.
    ///
    /// `id` is set to 0 (assigned by DB on insert).
    /// `tier` is auto-determined from `event.is_skeleton()`.
    /// `timestamp` is set to current UTC time in milliseconds.
    pub fn new(
        task_id: String,
        seq: u32,
        event: PoeEvent,
        correlation_id: Option<String>,
    ) -> Self {
        let tier = if event.is_skeleton() {
            EventTier::Skeleton
        } else {
            EventTier::Pulse
        };
        let timestamp = chrono::Utc::now().timestamp_millis();
        Self {
            id: 0,
            task_id,
            seq,
            event,
            tier,
            timestamp,
            correlation_id,
        }
    }

    /// Convenience: return the event type tag.
    pub fn event_type_tag(&self) -> &'static str {
        self.event.event_type_tag()
    }

    /// Convenience: whether the inner event is Skeleton.
    pub fn is_skeleton(&self) -> bool {
        self.event.is_skeleton()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outcome_kind_serialization() {
        let kind = PoeOutcomeKind::Success;
        let json = serde_json::to_string(&kind).unwrap();
        let deserialized: PoeOutcomeKind = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, PoeOutcomeKind::Success));
    }

    #[test]
    fn test_event_type_tag() {
        let event = PoeEvent::ManifestCreated {
            task_id: "t1".into(),
            objective: "build auth".into(),
            hard_constraints_count: 3,
            soft_metrics_count: 1,
        };
        assert_eq!(event.event_type_tag(), "ManifestCreated");
        assert!(event.is_skeleton());
    }

    #[test]
    fn test_pulse_event_classification() {
        let event = PoeEvent::OperationAttempted {
            task_id: "t1".into(),
            attempt: 1,
            tokens_used: 5000,
        };
        assert_eq!(event.event_type_tag(), "OperationAttempted");
        assert!(!event.is_skeleton());
    }

    #[test]
    fn test_envelope_serialization_roundtrip() {
        let envelope = PoeEventEnvelope::new(
            "task-1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "task-1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 2,
                total_tokens: 10000,
                duration_ms: 5000,
                best_distance: 0.1,
            },
            None,
        );
        let json = serde_json::to_string(&envelope).unwrap();
        let deserialized: PoeEventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.task_id, "task-1");
        assert_eq!(deserialized.seq, 0);
        assert!(matches!(deserialized.tier, EventTier::Skeleton));
    }

    #[test]
    fn test_outcome_kind_to_satisfaction() {
        assert_eq!(PoeOutcomeKind::Success.to_satisfaction(), 1.0);
        assert_eq!(PoeOutcomeKind::StrategySwitch.to_satisfaction(), 0.3);
        assert_eq!(PoeOutcomeKind::BudgetExhausted.to_satisfaction(), 0.0);
    }
}
