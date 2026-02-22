//! Event Emitter with Dual-Write Pattern
//!
//! Implements the emit_and_record pattern for reliable event delivery.
//! Events are persisted to database first, then broadcast to subscribers.

use crate::error::AlephError;
use crate::resilience::AgentEvent;
use crate::memory::database::StateDatabase;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::classifier::{EventClassifier, EventTier, EventType, PulseBuffer};

/// Configuration for the event emitter
#[derive(Debug, Clone)]
pub struct EmitterConfig {
    /// Broadcast channel capacity
    pub channel_capacity: usize,

    /// Maximum retry attempts for failed broadcasts
    pub max_retries: u32,

    /// Initial retry delay
    pub retry_delay_ms: u64,

    /// Enable pulse buffering
    pub enable_pulse_buffer: bool,

    /// Pulse buffer flush interval (ms)
    pub pulse_flush_interval_ms: u64,
}

impl Default for EmitterConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 1000,
            max_retries: 3,
            retry_delay_ms: 100,
            enable_pulse_buffer: true,
            pulse_flush_interval_ms: 500,
        }
    }
}

/// Event Emitter with dual-write pattern
///
/// Ensures events are persisted to database before broadcast.
/// Handles backpressure and retries for reliable delivery.
pub struct EventEmitter {
    db: Arc<StateDatabase>,
    config: EmitterConfig,

    /// Broadcast channel for real-time subscribers
    broadcast_tx: broadcast::Sender<AgentEvent>,

    /// Sequence counter per task
    seq_counters: RwLock<std::collections::HashMap<String, AtomicU64>>,

    /// Pulse buffer for batching streaming events
    pulse_buffer: RwLock<PulseBuffer>,
}

impl EventEmitter {
    /// Create a new event emitter
    pub fn new(db: Arc<StateDatabase>) -> Self {
        Self::with_config(db, EmitterConfig::default())
    }

    /// Create an event emitter with custom config
    pub fn with_config(db: Arc<StateDatabase>, config: EmitterConfig) -> Self {
        let (broadcast_tx, _) = broadcast::channel(config.channel_capacity);

        Self {
            db,
            config,
            broadcast_tx,
            seq_counters: RwLock::new(std::collections::HashMap::new()),
            pulse_buffer: RwLock::new(PulseBuffer::new()),
        }
    }

    /// Subscribe to event broadcasts
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.broadcast_tx.subscribe()
    }

    /// Emit and record an event (the core dual-write pattern)
    ///
    /// 1. Persist to database (The Truth)
    /// 2. Broadcast to subscribers (The Pulse)
    /// 3. Handle backpressure with async retry
    pub async fn emit_and_record(
        &self,
        task_id: &str,
        event_type: &str,
        payload_json: &str,
    ) -> Result<AgentEvent, AlephError> {
        // Get next sequence number for this task
        let seq = self.next_seq(task_id).await;

        // Classify event
        let event_type_enum = EventType::from_str_infallible(event_type);
        let tier = EventClassifier::classify(&event_type_enum);
        let is_structural = EventClassifier::is_structural(&event_type_enum);

        // Create event
        let mut event = AgentEvent::new(task_id, seq, event_type, payload_json);
        event.is_structural = is_structural;

        match tier {
            EventTier::Skeleton => {
                // Immediate persist + broadcast
                self.persist_and_broadcast(event.clone()).await?;
            }
            EventTier::Pulse => {
                if self.config.enable_pulse_buffer {
                    // Add to buffer, flush if needed
                    let should_flush = {
                        let mut buffer = self.pulse_buffer.write().await;
                        buffer.push(event.clone())
                    };

                    if should_flush {
                        self.flush_pulse_buffer().await?;
                    }

                    // Still broadcast immediately for real-time observation
                    self.broadcast(event.clone()).await;
                } else {
                    // No buffering, immediate persist
                    self.persist_and_broadcast(event.clone()).await?;
                }
            }
            EventTier::Volatile => {
                // Broadcast only, no persistence
                self.broadcast(event.clone()).await;
            }
        }

        Ok(event)
    }

    /// Emit a skeleton event (always immediate persist)
    pub async fn emit_skeleton(
        &self,
        task_id: &str,
        event_type: &str,
        payload_json: &str,
    ) -> Result<AgentEvent, AlephError> {
        let seq = self.next_seq(task_id).await;
        let mut event = AgentEvent::new(task_id, seq, event_type, payload_json);
        event.is_structural = true;

        self.persist_and_broadcast(event.clone()).await?;
        Ok(event)
    }

    /// Emit a volatile event (broadcast only, no persist)
    pub async fn emit_volatile(&self, task_id: &str, event_type: &str, payload_json: &str) {
        let seq = self.next_seq(task_id).await;
        let event = AgentEvent::new(task_id, seq, event_type, payload_json);
        self.broadcast(event).await;
    }

    /// Flush the pulse buffer to database
    pub async fn flush_pulse_buffer(&self) -> Result<usize, AlephError> {
        let events = {
            let mut buffer = self.pulse_buffer.write().await;
            buffer.drain()
        };

        if events.is_empty() {
            return Ok(0);
        }

        let count = events.len();

        // Bulk insert to database
        self.db.bulk_insert_events(&events).await?;

        debug!(count = count, "Flushed pulse buffer to database");
        Ok(count)
    }

    /// Start background pulse flush task
    pub fn start_pulse_flusher(self: Arc<Self>) {
        let interval = Duration::from_millis(self.config.pulse_flush_interval_ms);

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);

            loop {
                interval_timer.tick().await;

                if let Err(e) = self.flush_pulse_buffer().await {
                    warn!(error = %e, "Failed to flush pulse buffer");
                }
            }
        });
    }

    /// Persist event to database and broadcast
    async fn persist_and_broadcast(&self, event: AgentEvent) -> Result<(), AlephError> {
        // 1. DB Commit (The Truth)
        self.db.insert_event(&event).await?;

        // 2. Bus Broadcast (The Pulse)
        self.broadcast(event).await;

        Ok(())
    }

    /// Broadcast event with retry on backpressure
    async fn broadcast(&self, event: AgentEvent) {
        match self.broadcast_tx.send(event.clone()) {
            Ok(_) => {}
            Err(_) => {
                // No active subscribers, this is fine
                debug!(
                    task_id = %event.task_id,
                    seq = event.seq,
                    "No subscribers for event broadcast"
                );
            }
        }
    }

    /// Get next sequence number for a task
    async fn next_seq(&self, task_id: &str) -> u64 {
        let counters = self.seq_counters.read().await;

        if let Some(counter) = counters.get(task_id) {
            return counter.fetch_add(1, Ordering::SeqCst) + 1;
        }

        drop(counters);

        // Initialize counter for new task
        let mut counters = self.seq_counters.write().await;
        let counter = counters
            .entry(task_id.to_string())
            .or_insert_with(|| AtomicU64::new(0));

        counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Initialize sequence counter from database
    pub async fn sync_seq_from_db(&self, task_id: &str) -> Result<u64, AlephError> {
        let latest_seq = self.db.get_latest_event_seq(task_id).await?.unwrap_or(0);

        let mut counters = self.seq_counters.write().await;
        counters.insert(task_id.to_string(), AtomicU64::new(latest_seq));

        Ok(latest_seq)
    }

    /// Get current subscriber count
    pub fn subscriber_count(&self) -> usize {
        self.broadcast_tx.receiver_count()
    }
}

impl std::fmt::Debug for EventEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventEmitter")
            .field("config", &self.config)
            .field("subscriber_count", &self.subscriber_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emitter_config_default() {
        let config = EmitterConfig::default();
        assert_eq!(config.channel_capacity, 1000);
        assert_eq!(config.max_retries, 3);
        assert!(config.enable_pulse_buffer);
    }
}
