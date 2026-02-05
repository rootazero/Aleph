//! Event Classifier
//!
//! Implements the Skeleton & Pulse event classification model.
//! Events are categorized by their persistence requirements.

use crate::memory::database::resilience::AgentEvent;

/// Event persistence tier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventTier {
    /// Skeleton: Structural events that must be persisted immediately
    /// Examples: TaskStarted, ToolCallCompleted, ArtifactCreated
    Skeleton,

    /// Pulse: Streaming events that can be buffered before persistence
    /// Examples: AiStreamingResponse, ProgressUpdate
    Pulse,

    /// Volatile: Ephemeral events that exist only in memory
    /// Examples: HeartbeatStatus, MetricsUpdate
    Volatile,
}

/// Event types for classification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventType {
    // Skeleton events (immediate persist)
    TaskStarted,
    TaskCompleted,
    TaskFailed,
    TaskInterrupted,
    ToolCallStarted,
    ToolCallCompleted,
    ArtifactCreated,
    ArtifactModified,
    SessionCreated,
    SessionEnded,
    CheckpointCreated,

    // Pulse events (buffered persist)
    AiStreamingChunk,
    ProgressUpdate,
    LogEntry,
    TokenUsage,

    // Volatile events (memory only)
    Heartbeat,
    MetricsSnapshot,
    SubscriptionAck,

    // Custom/unknown events default to Skeleton for safety
    Custom(String),
}

impl EventType {
    /// Parse event type from string
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            // Skeleton
            "task_started" | "taskstarted" => EventType::TaskStarted,
            "task_completed" | "taskcompleted" => EventType::TaskCompleted,
            "task_failed" | "taskfailed" => EventType::TaskFailed,
            "task_interrupted" | "taskinterrupted" => EventType::TaskInterrupted,
            "tool_call_started" | "toolcallstarted" => EventType::ToolCallStarted,
            "tool_call_completed" | "toolcallcompleted" => EventType::ToolCallCompleted,
            "artifact_created" | "artifactcreated" => EventType::ArtifactCreated,
            "artifact_modified" | "artifactmodified" => EventType::ArtifactModified,
            "session_created" | "sessioncreated" => EventType::SessionCreated,
            "session_ended" | "sessionended" => EventType::SessionEnded,
            "checkpoint_created" | "checkpointcreated" => EventType::CheckpointCreated,

            // Pulse
            "ai_streaming_chunk" | "aistreamingchunk" | "streaming_chunk" => {
                EventType::AiStreamingChunk
            }
            "progress_update" | "progressupdate" => EventType::ProgressUpdate,
            "log_entry" | "logentry" | "log" => EventType::LogEntry,
            "token_usage" | "tokenusage" => EventType::TokenUsage,

            // Volatile
            "heartbeat" => EventType::Heartbeat,
            "metrics_snapshot" | "metricssnapshot" | "metrics" => EventType::MetricsSnapshot,
            "subscription_ack" | "subscriptionack" => EventType::SubscriptionAck,

            // Unknown -> Custom
            other => EventType::Custom(other.to_string()),
        }
    }

    /// Convert to string representation
    pub fn as_str(&self) -> &str {
        match self {
            EventType::TaskStarted => "task_started",
            EventType::TaskCompleted => "task_completed",
            EventType::TaskFailed => "task_failed",
            EventType::TaskInterrupted => "task_interrupted",
            EventType::ToolCallStarted => "tool_call_started",
            EventType::ToolCallCompleted => "tool_call_completed",
            EventType::ArtifactCreated => "artifact_created",
            EventType::ArtifactModified => "artifact_modified",
            EventType::SessionCreated => "session_created",
            EventType::SessionEnded => "session_ended",
            EventType::CheckpointCreated => "checkpoint_created",
            EventType::AiStreamingChunk => "ai_streaming_chunk",
            EventType::ProgressUpdate => "progress_update",
            EventType::LogEntry => "log_entry",
            EventType::TokenUsage => "token_usage",
            EventType::Heartbeat => "heartbeat",
            EventType::MetricsSnapshot => "metrics_snapshot",
            EventType::SubscriptionAck => "subscription_ack",
            EventType::Custom(s) => s,
        }
    }
}

/// Event Classifier for determining persistence tier
pub struct EventClassifier;

impl EventClassifier {
    /// Classify an event type into its persistence tier
    pub fn classify(event_type: &EventType) -> EventTier {
        match event_type {
            // Skeleton: Must persist immediately
            EventType::TaskStarted
            | EventType::TaskCompleted
            | EventType::TaskFailed
            | EventType::TaskInterrupted
            | EventType::ToolCallStarted
            | EventType::ToolCallCompleted
            | EventType::ArtifactCreated
            | EventType::ArtifactModified
            | EventType::SessionCreated
            | EventType::SessionEnded
            | EventType::CheckpointCreated => EventTier::Skeleton,

            // Pulse: Buffer before persist
            EventType::AiStreamingChunk
            | EventType::ProgressUpdate
            | EventType::LogEntry
            | EventType::TokenUsage => EventTier::Pulse,

            // Volatile: Memory only
            EventType::Heartbeat | EventType::MetricsSnapshot | EventType::SubscriptionAck => {
                EventTier::Volatile
            }

            // Custom events default to Skeleton for safety
            EventType::Custom(_) => EventTier::Skeleton,
        }
    }

    /// Classify an AgentEvent by its event_type field
    pub fn classify_event(event: &AgentEvent) -> EventTier {
        let event_type = EventType::from_str(&event.event_type);
        Self::classify(&event_type)
    }

    /// Check if event is structural (should be marked in DB)
    pub fn is_structural(event_type: &EventType) -> bool {
        matches!(Self::classify(event_type), EventTier::Skeleton)
    }

    /// Check if event should be persisted to database
    pub fn should_persist(event_type: &EventType) -> bool {
        !matches!(Self::classify(event_type), EventTier::Volatile)
    }
}

/// Pulse buffer for batching streaming events
pub struct PulseBuffer {
    /// Buffer of pending events
    events: Vec<AgentEvent>,

    /// Maximum events before flush
    max_events: usize,

    /// Maximum time before flush (milliseconds)
    max_age_ms: u64,

    /// Timestamp of first event in buffer
    first_event_time: Option<u64>,
}

impl PulseBuffer {
    /// Create a new pulse buffer with default settings
    pub fn new() -> Self {
        Self::with_config(50, 500)
    }

    /// Create a pulse buffer with custom settings
    pub fn with_config(max_events: usize, max_age_ms: u64) -> Self {
        Self {
            events: Vec::with_capacity(max_events),
            max_events,
            max_age_ms,
            first_event_time: None,
        }
    }

    /// Add an event to the buffer
    ///
    /// Returns true if the buffer should be flushed
    pub fn push(&mut self, event: AgentEvent) -> bool {
        if self.events.is_empty() {
            self.first_event_time = Some(current_timestamp_ms());
        }

        self.events.push(event);

        self.should_flush()
    }

    /// Check if buffer should be flushed
    pub fn should_flush(&self) -> bool {
        // Flush if max events reached
        if self.events.len() >= self.max_events {
            return true;
        }

        // Flush if max age exceeded
        if let Some(first_time) = self.first_event_time {
            let now = current_timestamp_ms();
            if now.saturating_sub(first_time) >= self.max_age_ms {
                return true;
            }
        }

        false
    }

    /// Take all events from the buffer
    pub fn drain(&mut self) -> Vec<AgentEvent> {
        self.first_event_time = None;
        std::mem::take(&mut self.events)
    }

    /// Number of events in buffer
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl Default for PulseBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Get current timestamp in milliseconds
fn current_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_parsing() {
        assert_eq!(EventType::from_str("task_started"), EventType::TaskStarted);
        assert_eq!(EventType::from_str("TaskStarted"), EventType::TaskStarted);
        assert_eq!(
            EventType::from_str("tool_call_completed"),
            EventType::ToolCallCompleted
        );
        assert_eq!(EventType::from_str("heartbeat"), EventType::Heartbeat);
        assert_eq!(
            EventType::from_str("unknown_event"),
            EventType::Custom("unknown_event".to_string())
        );
    }

    #[test]
    fn test_classification() {
        assert_eq!(
            EventClassifier::classify(&EventType::TaskStarted),
            EventTier::Skeleton
        );
        assert_eq!(
            EventClassifier::classify(&EventType::AiStreamingChunk),
            EventTier::Pulse
        );
        assert_eq!(
            EventClassifier::classify(&EventType::Heartbeat),
            EventTier::Volatile
        );
        assert_eq!(
            EventClassifier::classify(&EventType::Custom("unknown".to_string())),
            EventTier::Skeleton
        );
    }

    #[test]
    fn test_should_persist() {
        assert!(EventClassifier::should_persist(&EventType::TaskStarted));
        assert!(EventClassifier::should_persist(&EventType::AiStreamingChunk));
        assert!(!EventClassifier::should_persist(&EventType::Heartbeat));
    }

    #[test]
    fn test_pulse_buffer_size_flush() {
        let mut buffer = PulseBuffer::with_config(3, 10000);

        let event = AgentEvent::new("task1", 1, "progress_update", "{}");

        assert!(!buffer.push(event.clone()));
        assert!(!buffer.push(event.clone()));
        assert!(buffer.push(event)); // Third event triggers flush

        let events = buffer.drain();
        assert_eq!(events.len(), 3);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_pulse_buffer_drain() {
        let mut buffer = PulseBuffer::new();
        buffer.push(AgentEvent::new("task1", 1, "log", "{}"));
        buffer.push(AgentEvent::new("task1", 2, "log", "{}"));

        let events = buffer.drain();
        assert_eq!(events.len(), 2);
        assert!(buffer.is_empty());
    }
}
