//! Distillation service for converting raw experiences into reusable patterns
//!
//! This module implements the core distillation pipeline that processes
//! experiences from the replay buffer and extracts reusable patterns.

use crate::error::{AlephError, Result};
use crate::memory::cortex::{DistillationMode, DistillationTask};
use crate::memory::store::MemoryBackend;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Priority level for distillation tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DistillationPriority {
    /// Low priority (batch processing)
    Low = 0,
    /// Normal priority
    Normal = 1,
    /// High priority (realtime triggers)
    High = 2,
}

/// Distillation task with priority
#[derive(Debug, Clone)]
pub struct PrioritizedTask {
    pub task: DistillationTask,
    pub priority: DistillationPriority,
}

/// Configuration for distillation service
#[derive(Debug, Clone)]
pub struct DistillationConfig {
    /// Maximum number of concurrent distillation tasks
    pub max_concurrent_tasks: usize,
    /// Task queue capacity
    pub queue_capacity: usize,
    /// Enable realtime distillation
    pub enable_realtime: bool,
    /// Enable batch distillation
    pub enable_batch: bool,
}

impl Default for DistillationConfig {
    fn default() -> Self {
        Self {
            max_concurrent_tasks: 2,
            queue_capacity: 100,
            enable_realtime: true,
            enable_batch: true,
        }
    }
}

/// Distillation service for processing experiences
pub struct DistillationService {
    /// Database handle
    db: MemoryBackend,
    /// Configuration
    config: DistillationConfig,
    /// Task sender
    task_tx: mpsc::Sender<PrioritizedTask>,
    /// Worker handle
    worker_handle: Option<JoinHandle<()>>,
}

impl DistillationService {
    /// Create a new distillation service
    pub fn new(db: MemoryBackend, config: DistillationConfig) -> (Self, mpsc::Receiver<PrioritizedTask>) {
        let (task_tx, task_rx) = mpsc::channel(config.queue_capacity);

        let service = Self {
            db,
            config,
            task_tx,
            worker_handle: None,
        };

        (service, task_rx)
    }

    /// Start the distillation service
    pub async fn start(&mut self, task_rx: mpsc::Receiver<PrioritizedTask>) -> Result<()> {
        if self.worker_handle.is_some() {
            warn!("DistillationService already started");
            return Ok(());
        }

        info!("Starting DistillationService");

        let db = self.db.clone();
        let config = self.config.clone();

        let handle = tokio::spawn(async move {
            Self::worker_loop(db, config, task_rx).await;
        });

        self.worker_handle = Some(handle);
        Ok(())
    }

    /// Stop the distillation service
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(handle) = self.worker_handle.take() {
            info!("Stopping DistillationService");
            handle.abort();
            let _ = handle.await;
        }
        Ok(())
    }

    /// Enqueue a distillation task
    pub async fn enqueue_task(
        &self,
        task: DistillationTask,
        priority: DistillationPriority,
    ) -> Result<()> {
        // Check if mode is enabled
        match task.mode {
            DistillationMode::RealTime if !self.config.enable_realtime => {
                debug!("Realtime distillation disabled, skipping task");
                return Ok(());
            }
            DistillationMode::Batch if !self.config.enable_batch => {
                debug!("Batch distillation disabled, skipping task");
                return Ok(());
            }
            _ => {}
        }

        let prioritized = PrioritizedTask { task, priority };

        self.task_tx
            .send(prioritized)
            .await
            .map_err(|e| AlephError::Other {
                message: format!("Failed to enqueue task: {}", e),
                suggestion: None,
            })?;

        debug!("Enqueued distillation task with priority {:?}", priority);
        Ok(())
    }

    /// Worker loop that processes distillation tasks
    async fn worker_loop(
        db: MemoryBackend,
        _config: DistillationConfig,
        mut task_rx: mpsc::Receiver<PrioritizedTask>,
    ) {
        info!("DistillationService worker started");

        while let Some(prioritized_task) = task_rx.recv().await {
            debug!(
                "Processing distillation task: trace_id={}, mode={}, priority={:?}",
                prioritized_task.task.trace_id,
                prioritized_task.task.mode,
                prioritized_task.priority
            );

            // TODO: Implement actual distillation logic
            // This will be implemented in Task #7 (Pattern Extraction)

            if let Err(e) = Self::process_task(&db, &prioritized_task.task).await {
                error!(
                    "Failed to process distillation task {}: {}",
                    prioritized_task.task.trace_id, e
                );
            }
        }

        info!("DistillationService worker stopped");
    }

    /// Process a single distillation task
    async fn process_task(_db: &crate::memory::store::lance::LanceMemoryBackend, task: &DistillationTask) -> Result<()> {
        // Placeholder implementation
        // Will be replaced with actual pattern extraction in Task #7
        debug!("Processing task: {:?}", task);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_db() -> (MemoryBackend, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let backend = crate::memory::store::lance::LanceMemoryBackend::open_or_create(temp_dir.path()).await.unwrap();
        (Arc::new(backend), temp_dir)
    }

    #[tokio::test]
    async fn test_service_lifecycle() {
        let (db, _temp) = create_test_db().await;
        let config = DistillationConfig::default();

        let (mut service, task_rx) = DistillationService::new(db, config);

        // Start service
        service.start(task_rx).await.unwrap();
        assert!(service.worker_handle.is_some());

        // Stop service
        service.stop().await.unwrap();
        assert!(service.worker_handle.is_none());
    }

    #[tokio::test]
    async fn test_enqueue_task() {
        let (db, _temp) = create_test_db().await;
        let config = DistillationConfig::default();

        let (mut service, task_rx) = DistillationService::new(db, config);
        service.start(task_rx).await.unwrap();

        let task = DistillationTask {
            trace_id: "test-trace-123".to_string(),
            mode: DistillationMode::RealTime,
        };

        service
            .enqueue_task(task, DistillationPriority::High)
            .await
            .unwrap();

        service.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_disabled_mode() {
        let (db, _temp) = create_test_db().await;
        let config = DistillationConfig {
            enable_realtime: false,
            ..Default::default()
        };

        let (mut service, task_rx) = DistillationService::new(db, config);
        service.start(task_rx).await.unwrap();

        let task = DistillationTask {
            trace_id: "test-trace-456".to_string(),
            mode: DistillationMode::RealTime,
        };

        // Should succeed but skip processing
        service
            .enqueue_task(task, DistillationPriority::Normal)
            .await
            .unwrap();

        service.stop().await.unwrap();
    }
}
