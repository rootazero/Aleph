//! Cron Job Scheduling Service
//!
//! Provides scheduled job execution for automating agent tasks.
//!
//! # Features
//!
//! - Rich schedule kinds: cron expressions, intervals, one-shot
//! - JSON file persistence with atomic writes
//! - Concurrent job execution with configurable limits
//! - Job history and run logs
//! - Failure alerting and delivery pipeline
//! - Job chaining (on_success / on_failure triggers)
//! - Template rendering with variable substitution
//!
//! # Architecture
//!
//! - `config` — Type definitions (CronJob, ScheduleKind, JobStateV2, etc.)
//! - `store` — JSON atomic persistence
//! - `clock` — Time abstraction for testability
//! - `schedule` — Pure scheduling computation
//! - `stagger` — Hash-based stagger for cron jobs
//! - `service/` — State container, CRUD ops, timer loop, concurrency, catchup
//! - `execution/` — Three-phase job execution pipeline
//! - `alert` — Failure alerting
//! - `delivery` — Result delivery pipeline
//! - `chain` — Job chaining with cycle detection
//! - `template` — Prompt template rendering
//! - `webhook_target` — Webhook delivery target

pub mod alert;
pub mod chain;
pub mod clock;
pub mod config;
pub mod delivery;
pub mod execution;
pub mod schedule;
pub mod service;
pub mod stagger;
pub mod store;
pub mod template;
pub mod webhook_target;

// ── Re-exports ──────────────────────────────────────────────────────

pub use config::{
    CronConfig, CronJob, CronJobView, DeliveryConfig, DeliveryMode, DeliveryOutcome,
    DeliveryStatus, DeliveryTargetConfig, ErrorReason, ExecutionResult, FailureAlertConfig,
    JobRun, JobSnapshot, JobStateV2, RunStatus, ScheduleKind, SessionTarget, TriggerSource,
};
pub use delivery::{DeliveryEngine, DeliveryTarget};

use crate::sync_primitives::Arc;
use clock::{Clock, SystemClock};
use service::ServiceState;
use store::CronStore;

/// Callback for job execution
pub type JobExecutor = Arc<
    dyn Fn(String, String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;

/// Shared handle to CronService for use in gateway handlers
pub type SharedCronService = Arc<tokio::sync::Mutex<CronService>>;

/// High-level cron service wrapping the internal ServiceState.
///
/// Provides a simple async API for gateway handlers and CLI.
pub struct CronService {
    state: Arc<ServiceState<SystemClock>>,
}

impl CronService {
    /// Create a new CronService from configuration.
    ///
    /// Loads (or creates) the JSON store and initializes the service state.
    pub fn new(config: CronConfig) -> Result<Self, String> {
        config.validate().map_err(|e| format!("invalid config: {e}"))?;

        // Resolve store path: change .db to .json for the new store format
        let db_path = config.expand_db_path();
        let store_path = if db_path.ends_with(".db") {
            format!("{}on", db_path) // .db -> .dbon (not ideal)
        } else {
            db_path.clone()
        };
        // Use a .json path derived from the configured path
        let store_path = store_path
            .replace("cron.db", "cron.json")
            .replace("cron.dbon", "cron.json");

        // Create parent directory if needed
        let path = std::path::PathBuf::from(&store_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create store directory: {e}"))?;
        }

        let store = CronStore::load(path)?;
        let clock = Arc::new(SystemClock);
        let store = Arc::new(tokio::sync::Mutex::new(store));
        let state = Arc::new(ServiceState::new(store, clock, config));

        Ok(Self { state })
    }

    // ── Read operations ─────────────────────────────────────────────

    /// List all jobs as read-only views.
    pub async fn list_jobs(&self) -> Result<Vec<CronJobView>, String> {
        let store = self.state.store.lock().await;
        Ok(service::ops::list_jobs(&store))
    }

    /// Get a single job by ID as a read-only view.
    pub async fn get_job(&self, id: &str) -> Result<CronJobView, String> {
        let store = self.state.store.lock().await;
        service::ops::get_job(&store, id)
            .ok_or_else(|| format!("job not found: {id}"))
    }

    // ── Write operations ────────────────────────────────────────────

    /// Add a new job. Returns the job ID.
    pub async fn add_job(&self, job: CronJob) -> Result<String, String> {
        let mut store = self.state.store.lock().await;
        let id = service::ops::add_job(&mut store, job, self.state.clock.as_ref());
        store.persist()?;
        Ok(id)
    }

    /// Update an existing job with partial changes.
    pub async fn update_job(&self, id: &str, updates: service::ops::CronJobUpdates) -> Result<(), String> {
        let mut store = self.state.store.lock().await;
        service::ops::update_job(&mut store, id, updates, self.state.clock.as_ref())?;
        store.persist()?;
        Ok(())
    }

    /// Delete a job by ID.
    pub async fn delete_job(&self, id: &str) -> Result<(), String> {
        let mut store = self.state.store.lock().await;
        service::ops::delete_job(&mut store, id)?;
        store.persist()?;
        Ok(())
    }

    /// Enable a job by ID.
    pub async fn enable_job(&self, id: &str) -> Result<(), String> {
        let mut store = self.state.store.lock().await;
        let job = store
            .get_job_mut(id)
            .ok_or_else(|| format!("job not found: {id}"))?;
        if !job.enabled {
            job.enabled = true;
            job.updated_at = self.state.clock.now_ms();
            service::ops::recompute_next_run_full(job, self.state.clock.as_ref());
        }
        store.persist()?;
        Ok(())
    }

    /// Disable a job by ID.
    pub async fn disable_job(&self, id: &str) -> Result<(), String> {
        let mut store = self.state.store.lock().await;
        let job = store
            .get_job_mut(id)
            .ok_or_else(|| format!("job not found: {id}"))?;
        if job.enabled {
            job.enabled = false;
            job.state.next_run_at_ms = None;
            job.updated_at = self.state.clock.now_ms();
        }
        store.persist()?;
        Ok(())
    }

    /// Toggle a job's enabled state. Returns the new enabled state.
    pub async fn toggle_job(&self, id: &str) -> Result<bool, String> {
        let mut store = self.state.store.lock().await;
        let result = service::ops::toggle_job(&mut store, id, self.state.clock.as_ref())?;
        store.persist()?;
        Ok(result)
    }

    /// Access the internal service state (for advanced use cases like timer loops).
    pub fn state(&self) -> &Arc<ServiceState<SystemClock>> {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cron_service_basic_lifecycle() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("cron.json").to_string_lossy().to_string();

        let config = CronConfig {
            db_path,
            ..CronConfig::default()
        };
        let service = CronService::new(config).unwrap();

        // List empty
        let jobs = service.list_jobs().await.unwrap();
        assert!(jobs.is_empty());

        // Add a job
        let job = CronJob::new(
            "Test Job",
            "agent-1",
            "do something",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        let id = service.add_job(job).await.unwrap();

        // Get it back
        let view = service.get_job(&id).await.unwrap();
        assert_eq!(view.name, "Test Job");
        assert!(view.state.next_run_at_ms.is_some());

        // List has one
        let jobs = service.list_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);

        // Toggle disable
        service.disable_job(&id).await.unwrap();
        let view = service.get_job(&id).await.unwrap();
        assert!(!view.enabled);
        assert!(view.state.next_run_at_ms.is_none());

        // Toggle enable
        service.enable_job(&id).await.unwrap();
        let view = service.get_job(&id).await.unwrap();
        assert!(view.enabled);
        assert!(view.state.next_run_at_ms.is_some());

        // Delete
        service.delete_job(&id).await.unwrap();
        let jobs = service.list_jobs().await.unwrap();
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn cron_service_update() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("cron.json").to_string_lossy().to_string();

        let config = CronConfig {
            db_path,
            ..CronConfig::default()
        };
        let service = CronService::new(config).unwrap();

        let job = CronJob::new(
            "Original",
            "agent-1",
            "original prompt",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        let id = service.add_job(job).await.unwrap();

        let updates = service::ops::CronJobUpdates {
            name: Some("Updated".to_string()),
            prompt: Some("new prompt".to_string()),
            ..Default::default()
        };
        service.update_job(&id, updates).await.unwrap();

        let view = service.get_job(&id).await.unwrap();
        assert_eq!(view.name, "Updated");
        assert_eq!(view.prompt, "new prompt");
    }
}
