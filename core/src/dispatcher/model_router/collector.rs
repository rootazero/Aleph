//! Metrics Collector for Model Router
//!
//! This module provides async collection and aggregation of runtime metrics
//! from AI model API calls.

use super::metrics::{CallRecord, MultiWindowMetrics, UserFeedback, WindowConfig};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, RwLock};

// =============================================================================
// Errors
// =============================================================================

/// Metrics collection errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum MetricsError {
    #[error("Channel closed")]
    ChannelClosed,

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),
}

// =============================================================================
// Ring Buffer
// =============================================================================

/// Fixed-size ring buffer for storing recent call records
#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    buffer: Vec<Option<T>>,
    head: usize,
    tail: usize,
    size: usize,
    capacity: usize,
}

impl<T: Clone> RingBuffer<T> {
    /// Create a new ring buffer with given capacity
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![None; capacity],
            head: 0,
            tail: 0,
            size: 0,
            capacity,
        }
    }

    /// Push a new item, evicting oldest if full
    pub fn push(&mut self, item: T) {
        self.buffer[self.tail] = Some(item);
        self.tail = (self.tail + 1) % self.capacity;

        if self.size < self.capacity {
            self.size += 1;
        } else {
            // Buffer is full, move head forward (evict oldest)
            self.head = (self.head + 1) % self.capacity;
        }
    }

    /// Get the number of items in buffer
    pub fn len(&self) -> usize {
        self.size
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Check if buffer is full
    pub fn is_full(&self) -> bool {
        self.size == self.capacity
    }

    /// Get capacity
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Iterate over all items from oldest to newest
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        RingBufferIter {
            buffer: &self.buffer,
            head: self.head,
            remaining: self.size,
            capacity: self.capacity,
        }
    }

    /// Get most recent N items (newest first)
    pub fn recent(&self, n: usize) -> Vec<&T> {
        let n = n.min(self.size);
        let mut result = Vec::with_capacity(n);

        for i in 0..n {
            let idx = (self.tail + self.capacity - 1 - i) % self.capacity;
            if let Some(item) = &self.buffer[idx] {
                result.push(item);
            }
        }

        result
    }

    /// Filter items within a time window
    pub fn filter_by_time<F>(&self, since: SystemTime, time_fn: F) -> Vec<&T>
    where
        F: Fn(&T) -> SystemTime,
    {
        self.iter().filter(|item| time_fn(item) >= since).collect()
    }

    /// Clear all items
    pub fn clear(&mut self) {
        for item in self.buffer.iter_mut() {
            *item = None;
        }
        self.head = 0;
        self.tail = 0;
        self.size = 0;
    }
}

struct RingBufferIter<'a, T> {
    buffer: &'a [Option<T>],
    head: usize,
    remaining: usize,
    capacity: usize,
}

impl<'a, T> Iterator for RingBufferIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let idx = self.head;
        self.head = (self.head + 1) % self.capacity;
        self.remaining -= 1;

        self.buffer[idx].as_ref()
    }
}

// =============================================================================
// Collector Trait
// =============================================================================

/// Trait for metrics collection
#[async_trait::async_trait]
pub trait MetricsCollector: Send + Sync {
    /// Record a new call
    async fn record(&self, record: CallRecord);

    /// Get metrics for a specific model
    async fn get_metrics(&self, model_id: &str) -> Option<MultiWindowMetrics>;

    /// Get all model metrics
    async fn all_metrics(&self) -> HashMap<String, MultiWindowMetrics>;

    /// Record user feedback for a call
    async fn record_feedback(&self, call_id: &str, feedback: UserFeedback);

    /// Force flush to persistent storage
    async fn flush(&self) -> Result<(), MetricsError>;
}

// =============================================================================
// Collector Commands
// =============================================================================

/// Commands for async processing
#[derive(Debug)]
enum CollectorCommand {
    Record(CallRecord),
    Feedback {
        call_id: String,
        feedback: UserFeedback,
    },
    Flush,
}

// =============================================================================
// Metrics Configuration
// =============================================================================

/// Configuration for metrics collector
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Ring buffer capacity
    pub buffer_size: usize,
    /// Aggregation interval
    pub aggregation_interval: Duration,
    /// Flush interval for persistence
    pub flush_interval: Duration,
    /// Window configuration
    pub window_config: WindowConfig,
    /// Enable persistence
    pub persist_enabled: bool,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            buffer_size: 10_000,
            aggregation_interval: Duration::from_secs(60),
            flush_interval: Duration::from_secs(300),
            window_config: WindowConfig::default(),
            persist_enabled: true,
        }
    }
}

// =============================================================================
// In-Memory Collector
// =============================================================================

/// In-memory metrics collector (no persistence)
pub struct InMemoryMetricsCollector {
    /// Raw call records
    records: Arc<RwLock<RingBuffer<CallRecord>>>,

    /// Aggregated metrics per model
    aggregated: Arc<RwLock<HashMap<String, MultiWindowMetrics>>>,

    /// Configuration
    config: MetricsConfig,
}

impl InMemoryMetricsCollector {
    /// Create a new in-memory collector
    pub fn new(config: MetricsConfig) -> Self {
        Self {
            records: Arc::new(RwLock::new(RingBuffer::new(config.buffer_size))),
            aggregated: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Get raw record count
    pub async fn record_count(&self) -> usize {
        self.records.read().await.len()
    }
}

#[async_trait::async_trait]
impl MetricsCollector for InMemoryMetricsCollector {
    async fn record(&self, record: CallRecord) {
        let model_id = record.model_id.clone();

        // Add to ring buffer
        {
            let mut records = self.records.write().await;
            records.push(record.clone());
        }

        // Update aggregated metrics
        {
            let mut aggregated = self.aggregated.write().await;
            let metrics = aggregated
                .entry(model_id.clone())
                .or_insert_with(|| MultiWindowMetrics::new(&model_id));
            metrics.update(&record);
        }
    }

    async fn get_metrics(&self, model_id: &str) -> Option<MultiWindowMetrics> {
        let aggregated = self.aggregated.read().await;
        aggregated.get(model_id).cloned()
    }

    async fn all_metrics(&self) -> HashMap<String, MultiWindowMetrics> {
        let aggregated = self.aggregated.read().await;
        aggregated.clone()
    }

    async fn record_feedback(&self, call_id: &str, feedback: UserFeedback) {
        // Find the record and update feedback
        let records = self.records.read().await;
        for record in records.iter() {
            if record.id == call_id {
                // Update the aggregated metrics with feedback
                let mut aggregated = self.aggregated.write().await;
                if let Some(metrics) = aggregated.get_mut(&record.model_id) {
                    // Create a synthetic update with feedback
                    let mut updated_record = record.clone();
                    updated_record.user_feedback = Some(feedback);

                    // Update satisfaction scores
                    let score = feedback.to_score();
                    let current = metrics.all_time.satisfaction_score.unwrap_or(0.5);
                    metrics.all_time.satisfaction_score = Some(current * 0.9 + score * 0.1);
                    metrics.medium_term.satisfaction_score = Some(current * 0.9 + score * 0.1);
                }
                break;
            }
        }
    }

    async fn flush(&self) -> Result<(), MetricsError> {
        // No-op for in-memory collector
        Ok(())
    }
}

// =============================================================================
// Hybrid Collector (with async processing)
// =============================================================================

/// Hybrid metrics collector with async processing and optional persistence
pub struct HybridMetricsCollector {
    /// Raw call records
    records: Arc<RwLock<RingBuffer<CallRecord>>>,

    /// Aggregated metrics per model
    aggregated: Arc<RwLock<HashMap<String, MultiWindowMetrics>>>,

    /// Command sender for async processing
    command_tx: mpsc::Sender<CollectorCommand>,

    /// Configuration
    config: MetricsConfig,
}

impl HybridMetricsCollector {
    /// Create a new hybrid collector
    pub fn new(config: MetricsConfig) -> Self {
        let (command_tx, command_rx) = mpsc::channel(1000);

        let records = Arc::new(RwLock::new(RingBuffer::new(config.buffer_size)));
        let aggregated = Arc::new(RwLock::new(HashMap::new()));

        let collector = Self {
            records: records.clone(),
            aggregated: aggregated.clone(),
            command_tx,
            config: config.clone(),
        };

        // Spawn background processor
        tokio::spawn(Self::process_commands(
            command_rx, records, aggregated, config,
        ));

        collector
    }

    async fn process_commands(
        mut rx: mpsc::Receiver<CollectorCommand>,
        records: Arc<RwLock<RingBuffer<CallRecord>>>,
        aggregated: Arc<RwLock<HashMap<String, MultiWindowMetrics>>>,
        _config: MetricsConfig,
    ) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                CollectorCommand::Record(record) => {
                    let model_id = record.model_id.clone();

                    // Add to ring buffer
                    {
                        let mut records = records.write().await;
                        records.push(record.clone());
                    }

                    // Update aggregated metrics
                    {
                        let mut agg = aggregated.write().await;
                        let metrics = agg
                            .entry(model_id.clone())
                            .or_insert_with(|| MultiWindowMetrics::new(&model_id));
                        metrics.update(&record);
                    }
                }

                CollectorCommand::Feedback { call_id, feedback } => {
                    let recs = records.read().await;
                    for record in recs.iter() {
                        if record.id == call_id {
                            let mut agg = aggregated.write().await;
                            if let Some(metrics) = agg.get_mut(&record.model_id) {
                                let score = feedback.to_score();
                                let current = metrics.all_time.satisfaction_score.unwrap_or(0.5);
                                metrics.all_time.satisfaction_score =
                                    Some(current * 0.9 + score * 0.1);
                            }
                            break;
                        }
                    }
                }

                CollectorCommand::Flush => {
                    // Persistence would be handled here
                    tracing::debug!("Metrics flush requested");
                }
            }
        }
    }

    /// Get raw record count
    pub async fn record_count(&self) -> usize {
        self.records.read().await.len()
    }

    /// Get recent records for a model
    pub async fn recent_records(&self, model_id: &str, count: usize) -> Vec<CallRecord> {
        let records = self.records.read().await;
        records
            .recent(count)
            .into_iter()
            .filter(|r| r.model_id == model_id)
            .cloned()
            .collect()
    }
}

#[async_trait::async_trait]
impl MetricsCollector for HybridMetricsCollector {
    async fn record(&self, record: CallRecord) {
        // Non-blocking send
        let _ = self.command_tx.try_send(CollectorCommand::Record(record));
    }

    async fn get_metrics(&self, model_id: &str) -> Option<MultiWindowMetrics> {
        let aggregated = self.aggregated.read().await;
        aggregated.get(model_id).cloned()
    }

    async fn all_metrics(&self) -> HashMap<String, MultiWindowMetrics> {
        let aggregated = self.aggregated.read().await;
        aggregated.clone()
    }

    async fn record_feedback(&self, call_id: &str, feedback: UserFeedback) {
        let _ = self.command_tx.try_send(CollectorCommand::Feedback {
            call_id: call_id.to_string(),
            feedback,
        });
    }

    async fn flush(&self) -> Result<(), MetricsError> {
        self.command_tx
            .send(CollectorCommand::Flush)
            .await
            .map_err(|_| MetricsError::ChannelClosed)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::model_router::{CallOutcome, TaskIntent};

    #[test]
    fn test_ring_buffer_basic() {
        let mut buffer = RingBuffer::<i32>::new(3);

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.capacity(), 3);

        buffer.push(1);
        buffer.push(2);
        buffer.push(3);

        assert!(buffer.is_full());
        assert_eq!(buffer.len(), 3);

        let items: Vec<_> = buffer.iter().collect();
        assert_eq!(items, vec![&1, &2, &3]);
    }

    #[test]
    fn test_ring_buffer_overflow() {
        let mut buffer = RingBuffer::<i32>::new(3);

        buffer.push(1);
        buffer.push(2);
        buffer.push(3);
        buffer.push(4); // Should evict 1

        assert_eq!(buffer.len(), 3);

        let items: Vec<_> = buffer.iter().collect();
        assert_eq!(items, vec![&2, &3, &4]);
    }

    #[test]
    fn test_ring_buffer_recent() {
        let mut buffer = RingBuffer::<i32>::new(5);

        for i in 1..=5 {
            buffer.push(i);
        }

        let recent = buffer.recent(3);
        assert_eq!(recent, vec![&5, &4, &3]);

        let recent_all = buffer.recent(10); // More than capacity
        assert_eq!(recent_all.len(), 5);
    }

    #[test]
    fn test_ring_buffer_clear() {
        let mut buffer = RingBuffer::<i32>::new(3);

        buffer.push(1);
        buffer.push(2);
        assert_eq!(buffer.len(), 2);

        buffer.clear();
        assert!(buffer.is_empty());
    }

    fn create_test_record(id: &str, model_id: &str, success: bool) -> CallRecord {
        if success {
            CallRecord::success(
                id,
                model_id,
                TaskIntent::CodeGeneration,
                100,
                200,
                Duration::from_millis(1000),
            )
        } else {
            CallRecord::failure(
                id,
                model_id,
                TaskIntent::CodeGeneration,
                Duration::from_millis(5000),
                CallOutcome::Timeout,
            )
        }
    }

    #[tokio::test]
    async fn test_in_memory_collector_record() {
        let collector = InMemoryMetricsCollector::new(MetricsConfig::default());

        let record = create_test_record("1", "model-a", true);
        collector.record(record).await;

        assert_eq!(collector.record_count().await, 1);

        let metrics = collector.get_metrics("model-a").await;
        assert!(metrics.is_some());

        let metrics = metrics.unwrap();
        assert_eq!(metrics.all_time.total_calls, 1);
        assert_eq!(metrics.all_time.successful_calls, 1);
    }

    #[tokio::test]
    async fn test_in_memory_collector_multiple_models() {
        let collector = InMemoryMetricsCollector::new(MetricsConfig::default());

        collector
            .record(create_test_record("1", "model-a", true))
            .await;
        collector
            .record(create_test_record("2", "model-b", true))
            .await;
        collector
            .record(create_test_record("3", "model-a", false))
            .await;

        let all = collector.all_metrics().await;
        assert_eq!(all.len(), 2);

        let a_metrics = collector.get_metrics("model-a").await.unwrap();
        assert_eq!(a_metrics.all_time.total_calls, 2);
        assert_eq!(a_metrics.all_time.successful_calls, 1);

        let b_metrics = collector.get_metrics("model-b").await.unwrap();
        assert_eq!(b_metrics.all_time.total_calls, 1);
    }

    #[tokio::test]
    async fn test_in_memory_collector_feedback() {
        let collector = InMemoryMetricsCollector::new(MetricsConfig::default());

        let record = create_test_record("call-1", "model-a", true);
        collector.record(record).await;

        collector
            .record_feedback("call-1", UserFeedback::Positive)
            .await;

        let metrics = collector.get_metrics("model-a").await.unwrap();
        assert!(metrics.all_time.satisfaction_score.is_some());
    }

    #[tokio::test]
    async fn test_hybrid_collector_record() {
        let collector = HybridMetricsCollector::new(MetricsConfig::default());

        let record = create_test_record("1", "model-a", true);
        collector.record(record).await;

        // Give async processor time to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        let metrics = collector.get_metrics("model-a").await;
        assert!(metrics.is_some());
    }

    #[tokio::test]
    async fn test_hybrid_collector_recent_records() {
        let collector = HybridMetricsCollector::new(MetricsConfig::default());

        for i in 0..5 {
            collector
                .record(create_test_record(&format!("{}", i), "model-a", true))
                .await;
        }

        // Give async processor time
        tokio::time::sleep(Duration::from_millis(100)).await;

        let recent = collector.recent_records("model-a", 3).await;
        assert_eq!(recent.len(), 3);
    }

    #[tokio::test]
    async fn test_collector_flush() {
        let collector = InMemoryMetricsCollector::new(MetricsConfig::default());
        let result = collector.flush().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_metrics_config_default() {
        let config = MetricsConfig::default();
        assert_eq!(config.buffer_size, 10_000);
        assert_eq!(config.aggregation_interval, Duration::from_secs(60));
        assert_eq!(config.flush_interval, Duration::from_secs(300));
    }
}
