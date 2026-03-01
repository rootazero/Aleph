//! Cortex Dreaming Service - Background experience distillation
//!
//! This service runs during idle time or on schedule to process accumulated
//! experiences and extract reusable patterns.

use crate::error::Result;
use crate::memory::cortex::{
    DistillationMode, DistillationPriority, DistillationService, DistillationTask,
};
use crate::memory::store::MemoryBackend;
use crate::memory::value_estimator::cortex::CortexValueEstimator;
use chrono::{Datelike, Timelike};
use crate::sync_primitives::{AtomicBool, AtomicU64, Ordering};
use crate::sync_primitives::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Configuration for Cortex Dreaming Service
#[derive(Debug, Clone)]
pub struct CortexDreamingConfig {
    /// Minimum idle time before starting batch processing (seconds)
    pub min_idle_seconds: u64,
    /// Check interval for idle detection (seconds)
    pub check_interval_seconds: u64,
    /// Maximum experiences to process per batch
    pub max_batch_size: usize,
    /// Minimum value score to consider for distillation
    pub min_value_score: f64,
    /// Rate limit: max distillations per minute
    pub max_distillations_per_minute: usize,
    /// Enable scheduled processing (e.g., daily at 2 AM)
    pub enable_scheduled: bool,
    /// Scheduled processing time (hour, 0-23)
    pub scheduled_hour: u8,
}

impl Default for CortexDreamingConfig {
    fn default() -> Self {
        Self {
            min_idle_seconds: 300, // 5 minutes
            check_interval_seconds: 60,
            max_batch_size: 50,
            min_value_score: 0.6,
            max_distillations_per_minute: 10,
            enable_scheduled: true,
            scheduled_hour: 2, // 2 AM
        }
    }
}

/// Metrics for dreaming service
#[derive(Debug, Default)]
pub struct DreamingMetrics {
    /// Total experiences processed
    pub total_processed: AtomicU64,
    /// Total patterns extracted
    pub total_extracted: AtomicU64,
    /// Total errors encountered
    pub total_errors: AtomicU64,
    /// Last processing timestamp
    pub last_processing_ts: AtomicU64,
}

impl DreamingMetrics {
    pub fn increment_processed(&self) {
        self.total_processed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_extracted(&self) {
        self.total_extracted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn increment_errors(&self) {
        self.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn update_last_processing(&self, timestamp: u64) {
        self.last_processing_ts.store(timestamp, Ordering::Relaxed);
    }

    pub fn get_stats(&self) -> (u64, u64, u64, u64) {
        (
            self.total_processed.load(Ordering::Relaxed),
            self.total_extracted.load(Ordering::Relaxed),
            self.total_errors.load(Ordering::Relaxed),
            self.last_processing_ts.load(Ordering::Relaxed),
        )
    }
}

/// Cortex Dreaming Service
pub struct CortexDreamingService {
    db: MemoryBackend,
    distillation_service: Arc<RwLock<DistillationService>>,
    value_estimator: Arc<CortexValueEstimator>,
    config: CortexDreamingConfig,
    metrics: Arc<DreamingMetrics>,
    running: Arc<AtomicBool>,
    worker_handle: Option<JoinHandle<()>>,
}

impl CortexDreamingService {
    /// Create a new Cortex Dreaming Service
    pub fn new(
        db: MemoryBackend,
        distillation_service: Arc<RwLock<DistillationService>>,
        value_estimator: Arc<CortexValueEstimator>,
        config: CortexDreamingConfig,
    ) -> Self {
        Self {
            db,
            distillation_service,
            value_estimator,
            config,
            metrics: Arc::new(DreamingMetrics::default()),
            running: Arc::new(AtomicBool::new(false)),
            worker_handle: None,
        }
    }

    /// Start the dreaming service
    pub async fn start(&mut self) -> Result<()> {
        if self.running.load(Ordering::Relaxed) {
            warn!("CortexDreamingService already running");
            return Ok(());
        }

        info!("Starting CortexDreamingService");
        self.running.store(true, Ordering::Relaxed);

        let db = self.db.clone();
        let distillation_service = self.distillation_service.clone();
        let value_estimator = self.value_estimator.clone();
        let config = self.config.clone();
        let metrics = self.metrics.clone();
        let running = self.running.clone();

        let handle = tokio::spawn(async move {
            Self::worker_loop(
                db,
                distillation_service,
                value_estimator,
                config,
                metrics,
                running,
            )
            .await;
        });

        self.worker_handle = Some(handle);
        Ok(())
    }

    /// Stop the dreaming service
    pub async fn stop(&mut self) -> Result<()> {
        if !self.running.load(Ordering::Relaxed) {
            return Ok(());
        }

        info!("Stopping CortexDreamingService");
        self.running.store(false, Ordering::Relaxed);

        if let Some(handle) = self.worker_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        Ok(())
    }

    /// Get current metrics
    pub fn metrics(&self) -> (u64, u64, u64, u64) {
        self.metrics.get_stats()
    }

    /// Worker loop
    async fn worker_loop(
        db: MemoryBackend,
        distillation_service: Arc<RwLock<DistillationService>>,
        value_estimator: Arc<CortexValueEstimator>,
        config: CortexDreamingConfig,
        metrics: Arc<DreamingMetrics>,
        running: Arc<AtomicBool>,
    ) {
        info!("CortexDreamingService worker started");

        let mut check_interval = interval(Duration::from_secs(config.check_interval_seconds));
        let mut last_scheduled_day = 0u32;

        while running.load(Ordering::Relaxed) {
            check_interval.tick().await;

            // Check if we should run scheduled processing
            if config.enable_scheduled {
                let now = chrono::Local::now();
                let current_day = now.ordinal0(); // 0-based day of year

                if now.hour() == config.scheduled_hour as u32 && current_day != last_scheduled_day
                {
                    info!("Starting scheduled batch processing");
                    last_scheduled_day = current_day;

                    if let Err(e) = Self::process_batch(
                        &db,
                        &distillation_service,
                        &value_estimator,
                        &config,
                        &metrics,
                    )
                    .await
                    {
                        error!("Scheduled batch processing failed: {}", e);
                    }
                }
            }

            // Check idle time for opportunistic processing
            let idle_secs = Self::get_idle_seconds();
            if idle_secs >= config.min_idle_seconds {
                debug!("System idle for {}s, starting batch processing", idle_secs);

                if let Err(e) = Self::process_batch(
                    &db,
                    &distillation_service,
                    &value_estimator,
                    &config,
                    &metrics,
                )
                .await
                {
                    error!("Idle batch processing failed: {}", e);
                }
            }
        }

        info!("CortexDreamingService worker stopped");
    }

    /// Process a batch of candidate experiences
    async fn process_batch(
        _db: &crate::memory::store::lance::LanceMemoryBackend,
        distillation_service: &Arc<RwLock<DistillationService>>,
        value_estimator: &CortexValueEstimator,
        config: &CortexDreamingConfig,
        metrics: &DreamingMetrics,
    ) -> Result<()> {
        info!("Starting batch processing");

        // Query candidate experiences
        // TODO: Implement experience queries via new store API
        // Old code: db.query_experiences_by_status(EvolutionStatus::Candidate, config.max_batch_size)
        let candidates: Vec<crate::memory::cortex::Experience> = Vec::new();

        if candidates.is_empty() {
            debug!("No candidate experiences found");
            return Ok(());
        }

        info!("Found {} candidate experiences", candidates.len());

        // Score and filter candidates
        let mut scored_candidates = Vec::new();
        for exp in candidates {
            let score = value_estimator.estimate(&exp).await?;
            if score.final_score >= config.min_value_score {
                scored_candidates.push((exp, score.final_score));
            }
        }

        // Sort by score (highest first)
        scored_candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        info!(
            "Filtered to {} high-value experiences",
            scored_candidates.len()
        );

        // Enqueue distillation tasks with rate limiting
        let max_per_batch = config.max_distillations_per_minute;
        let to_process = scored_candidates
            .into_iter()
            .take(max_per_batch)
            .collect::<Vec<_>>();

        let service = distillation_service.read().await;
        for (exp, score) in to_process {
            let task = DistillationTask {
                trace_id: exp.id.clone(),
                mode: DistillationMode::Batch,
            };

            // Higher score = higher priority
            let priority = if score >= 0.9 {
                DistillationPriority::High
            } else if score >= 0.75 {
                DistillationPriority::Normal
            } else {
                DistillationPriority::Low
            };

            match service.enqueue_task(task, priority).await {
                Ok(_) => {
                    metrics.increment_processed();
                    debug!("Enqueued distillation for experience: {}", exp.id);
                }
                Err(e) => {
                    metrics.increment_errors();
                    error!("Failed to enqueue distillation: {}", e);
                }
            }
        }

        metrics.update_last_processing(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
                .as_secs(),
        );

        info!("Batch processing completed");
        Ok(())
    }

    /// Get system idle time in seconds
    /// TODO: Integrate with actual activity tracking
    fn get_idle_seconds() -> u64 {
        // Placeholder implementation
        // In real implementation, this would check:
        // - Last user interaction timestamp
        // - Last agent loop execution
        // - System activity indicators
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::cortex::DistillationConfig;
    use tempfile::TempDir;

    async fn create_test_db() -> (MemoryBackend, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let backend = crate::memory::store::lance::LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap();
        (Arc::new(backend), temp_dir)
    }

    #[tokio::test]
    async fn test_service_lifecycle() {
        let (db, _temp) = create_test_db().await;

        let distillation_config = DistillationConfig::default();
        let (distillation_service, _rx) = DistillationService::new(db.clone(), distillation_config);
        let distillation_service = Arc::new(RwLock::new(distillation_service));

        let value_estimator = Arc::new(CortexValueEstimator::default());
        let config = CortexDreamingConfig::default();

        let mut service = CortexDreamingService::new(
            db,
            distillation_service,
            value_estimator,
            config,
        );

        // Start service
        service.start().await.unwrap();
        assert!(service.running.load(Ordering::Relaxed));

        // Stop service
        service.stop().await.unwrap();
        assert!(!service.running.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_metrics() {
        let (db, _temp) = create_test_db().await;

        let distillation_config = DistillationConfig::default();
        let (distillation_service, _rx) = DistillationService::new(db.clone(), distillation_config);
        let distillation_service = Arc::new(RwLock::new(distillation_service));

        let value_estimator = Arc::new(CortexValueEstimator::default());
        let config = CortexDreamingConfig::default();

        let service = CortexDreamingService::new(
            db,
            distillation_service,
            value_estimator,
            config,
        );

        // Check initial metrics
        let (processed, extracted, errors, _) = service.metrics();
        assert_eq!(processed, 0);
        assert_eq!(extracted, 0);
        assert_eq!(errors, 0);

        // Increment metrics
        service.metrics.increment_processed();
        service.metrics.increment_extracted();

        let (processed, extracted, _, _) = service.metrics();
        assert_eq!(processed, 1);
        assert_eq!(extracted, 1);
    }
}
