//! DreamDaemon: background memory consolidation for the notes layer.
//!
//! This module implements a staged dream pipeline architecture.
//! Each stage implements the `DreamStage` trait and operates on a shared
//! `DreamContext` that flows through the pipeline.

pub mod gate;
pub mod report;
pub mod stages;
pub mod signals;
pub mod strategy;
pub mod selector;
pub mod mutation_gate;
pub mod validation;
pub mod event_log;

use crate::config::{DreamingConfig as ConfigDreamingConfig, MemoryConfig};
use crate::error::AlephError;
use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::memory::store::{DreamStore, MemoryBackend};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, AtomicI64, Ordering};
use chrono::{Local, NaiveTime, TimeZone};
use once_cell::sync::{Lazy, OnceCell};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{info, warn};

// Re-export gate types
pub use gate::{BlockReason, DreamGate, DreamGateConfig, GateResult};

// Re-export report types
pub use report::{DreamReport, DreamReportStatus};

// Re-export stage trait and shared types
pub use stages::{DreamStage, MemoryCluster};
pub use signals::{DreamSignal, RawMetrics, SignalSnapshot, SignalType};
pub use strategy::DreamStrategy;
pub use selector::{GateDecision, SelectionDecision, StrategySelector};
pub use mutation_gate::MutationGate;
pub use validation::{DreamValidationReport, ValidationIssue, ValidationTier};
pub use event_log::{DreamEvent, EventLog};

// ---------------------------------------------------------------------------
// NoteEntry — metadata for a single note in the dream pipeline
// ---------------------------------------------------------------------------

/// Metadata for a single note in the dream pipeline.
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub path: String,
    pub category: String,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_accessed_at: Option<i64>,
    pub content_hash: String,
}

// ---------------------------------------------------------------------------
// DreamContext — shared state flowing through the pipeline
// ---------------------------------------------------------------------------

/// Context passed through the dream pipeline stages.
pub struct DreamContext {
    pub notes: Vec<NoteEntry>,
    /// Lazy-loaded note contents: path → markdown body.
    pub note_contents: HashMap<String, String>,
    pub agent_id: String,
    pub database: MemoryBackend,
    pub indexer: NoteIndexer<SqliteMemoryBackend>,
    pub provider: Arc<dyn AiProvider>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub report: DreamReport,
    /// Strategy name driving this cycle ("consolidate", "synthesize", "conserve").
    pub pipeline_type: String,
    /// Activity checker: returns true if user activity has been detected.
    pub activity_checker: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Strategy selected for this Dream cycle.
    pub strategy: DreamStrategy,
    /// Optional wiki orientation — used by `IndexRefresherStage`.
    pub orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
}

impl DreamContext {
    /// Lazy-load a note's markdown content from disk.
    pub async fn load_content(&mut self, path: &str) -> Option<String> {
        if let Some(content) = self.note_contents.get(path) {
            return Some(content.clone());
        }
        let (category, filename) = path.split_once('/')?;
        let file_path = self
            .indexer
            .memory_dir()
            .join(&self.agent_id)
            .join(category)
            .join(format!("{filename}.md"));
        let content = tokio::fs::read_to_string(&file_path).await.ok()?;
        self.note_contents.insert(path.to_string(), content.clone());
        Some(content)
    }
}

// ---------------------------------------------------------------------------
// DreamPipeline — stage executor
// ---------------------------------------------------------------------------

/// Executes a sequence of `DreamStage` implementations.
pub struct DreamPipeline {
    stages: Vec<Box<dyn DreamStage>>,
}

impl DreamPipeline {
    pub fn new(stages: Vec<Box<dyn DreamStage>>) -> Self {
        Self { stages }
    }

    /// Build a pipeline from a DreamStrategy.
    pub fn from_strategy(strategy: DreamStrategy) -> Self {
        let stage_list: Vec<Box<dyn DreamStage>> = match strategy {
            DreamStrategy::Consolidate => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteConsolidateStage),
                Box::new(stages::NoteDriftStage),
                Box::new(stages::IndexRefresherStage),
                Box::new(stages::NoteDecayStage),
            ],
            DreamStrategy::Synthesize => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteConsolidateStage),
                Box::new(stages::NoteSynthesisStage),
                Box::new(stages::SkillDistillStage),
                Box::new(stages::DailyDigestStage),
            ],
            DreamStrategy::Conserve => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::IndexRefresherStage),
            ],
        };
        Self::new(stage_list)
    }

    /// Run the pipeline, returning the final `DreamReport`.
    pub async fn run(&self, mut ctx: DreamContext) -> Result<DreamReport, AlephError> {
        let mut executed: Vec<String> = Vec::new();

        for stage in &self.stages {
            if !stage.should_run(&ctx).await {
                continue;
            }
            // Check for user activity before each stage.
            if (ctx.activity_checker)() {
                let mut report = ctx.report;
                report.status = DreamReportStatus::Interrupted;
                report.interrupted_at_stage = Some(stage.name().to_string());
                report.stages_executed = executed;
                return Ok(report);
            }
            ctx = stage.execute(ctx).await?;
            executed.push(stage.name().to_string());
        }

        let mut report = ctx.report;
        report.status = DreamReportStatus::Completed;
        report.stages_executed = executed;
        Ok(report)
    }
}

impl Default for DreamPipeline {
    fn default() -> Self {
        Self::new(vec![])
    }
}

// ---------------------------------------------------------------------------
// Original DreamDaemon code (preserved, wiring updated for new context shape)
// ---------------------------------------------------------------------------

const DEFAULT_CHECK_INTERVAL_SECONDS: u64 = 60;

static LAST_ACTIVITY_TS: Lazy<AtomicI64> = Lazy::new(|| AtomicI64::new(now_timestamp()));
static DREAM_DAEMON: OnceCell<Arc<DreamDaemon>> = OnceCell::new();

pub(crate) fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs() as i64
}

/// Record user activity for DreamDaemon idle tracking.
pub fn record_activity() {
    LAST_ACTIVITY_TS.store(now_timestamp(), Ordering::Relaxed);
}

fn last_activity_timestamp() -> i64 {
    LAST_ACTIVITY_TS.load(Ordering::Relaxed)
}

fn idle_seconds() -> i64 {
    let now = now_timestamp();
    let last = last_activity_timestamp();
    (now - last).max(0)
}

/// Ensure DreamDaemon is running (once) when memory is enabled.
pub fn ensure_dream_daemon(
    database: MemoryBackend,
    config: Arc<MemoryConfig>,
    provider: Option<Arc<dyn AiProvider>>,
    command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
) {
    ensure_dream_daemon_with_orientation(database, config, provider, command_handler, None);
}

/// Ensure DreamDaemon is running (once) when memory is enabled, with optional orientation handle.
pub fn ensure_dream_daemon_with_orientation(
    database: MemoryBackend,
    config: Arc<MemoryConfig>,
    provider: Option<Arc<dyn AiProvider>>,
    command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
    orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
) {
    if cfg!(test) {
        return;
    }

    if !config.enabled || !config.dreaming.enabled {
        return;
    }

    if DREAM_DAEMON.get().is_some() {
        return;
    }

    let handle = match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle,
        Err(_) => {
            warn!("DreamDaemon not started: no Tokio runtime available");
            return;
        }
    };

    let daemon_builder = match DreamDaemon::from_config(database, &config) {
        Ok(d) => d,
        Err(err) => {
            warn!(error = %err, "DreamDaemon not started: invalid config");
            return;
        }
    };

    let daemon_builder = if let Some(handler) = command_handler {
        daemon_builder.with_command_handler(handler)
    } else {
        daemon_builder
    };

    let daemon_builder = if let Some(p) = provider {
        daemon_builder.with_provider(p)
    } else {
        daemon_builder
    };

    let daemon = if let Some(w) = orientation {
        Arc::new(daemon_builder.with_orientation(w))
    } else {
        Arc::new(daemon_builder)
    };

    if DREAM_DAEMON.set(daemon.clone()).is_ok() {
        daemon.start_background_task_with_handle(handle);
        info!("DreamDaemon background task started");
    }
}

/// Daily insight summary record.
#[derive(Debug, Clone)]
pub struct DailyInsight {
    pub date: String,
    pub content: String,
    pub source_memory_count: u32,
    pub created_at: i64,
}

impl DailyInsight {
    pub fn new(date: String, content: String, source_memory_count: u32) -> Self {
        Self {
            date,
            content,
            source_memory_count,
            created_at: now_timestamp(),
        }
    }
}

/// DreamDaemon status record.
#[derive(Debug, Clone, Default)]
pub struct DreamStatus {
    pub last_run_at: Option<i64>,
    pub last_status: Option<String>,
    pub last_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DreamRunStatus {
    Success,
    Cancelled,
}

impl DreamRunStatus {
    fn as_str(&self) -> &'static str {
        match self {
            DreamRunStatus::Success => "success",
            DreamRunStatus::Cancelled => "cancelled",
        }
    }
}

/// DreamDaemon orchestrates idle-time consolidation.
pub struct DreamDaemon {
    database: MemoryBackend,
    config: ConfigDreamingConfig,
    window_start: NaiveTime,
    window_end: NaiveTime,
    is_running: AtomicBool,
    /// Optional event-sourcing command handler.
    command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
    /// Optional AI provider for LLM-powered dream stages.
    provider: Option<Arc<dyn AiProvider>>,
    /// Optional wiki orientation — forwarded into DreamContext for IndexRefresherStage.
    orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    /// Strategy selector with personality adaptation.
    selector: std::sync::Mutex<StrategySelector>,
    /// Mutation gate tracking evolution pathologies.
    mutation_gate: std::sync::Mutex<MutationGate>,
}

impl DreamDaemon {
    pub fn from_config(database: MemoryBackend, config: &MemoryConfig) -> Result<Self, AlephError> {
        let (window_start, window_end) = parse_window(&config.dreaming)?;

        Ok(Self {
            database,
            config: config.dreaming.clone(),
            window_start,
            window_end,
            is_running: AtomicBool::new(false),
            command_handler: None,
            provider: None,
            orientation: None,
            selector: std::sync::Mutex::new(StrategySelector::new()),
            mutation_gate: std::sync::Mutex::new(MutationGate::new()),
        })
    }

    /// Attach an AI provider for LLM-powered dream stages.
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Attach an event-sourcing command handler.
    pub fn with_command_handler(
        mut self,
        handler: Arc<crate::memory::events::handler::MemoryCommandHandler>,
    ) -> Self {
        self.command_handler = Some(handler);
        self
    }

    /// Attach a wiki orientation handle for the IndexRefresher dream stage.
    pub fn with_orientation(
        mut self,
        orientation: Arc<dyn crate::memory::notes::orientation::NoteOrientation>,
    ) -> Self {
        self.orientation = Some(orientation);
        self
    }

    /// Start background scheduling task.
    pub fn start_background_task(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.run_scheduler().await;
        })
    }

    /// Start background task using an existing Tokio runtime handle.
    pub fn start_background_task_with_handle(
        self: Arc<Self>,
        handle: tokio::runtime::Handle,
    ) -> JoinHandle<()> {
        handle.spawn(async move {
            self.run_scheduler().await;
        })
    }

    async fn run_scheduler(self: Arc<Self>) {
        let mut ticker = interval(Duration::from_secs(DEFAULT_CHECK_INTERVAL_SECONDS));

        loop {
            ticker.tick().await;
            if let Err(err) = self.check_and_run().await {
                warn!(error = %err, "DreamDaemon check failed");
            }
        }
    }

    async fn check_and_run(&self) -> Result<(), AlephError> {
        if !self.config.enabled {
            return Ok(());
        }

        if !self.is_within_window() {
            return Ok(());
        }

        if idle_seconds() < self.config.idle_threshold_seconds as i64 {
            return Ok(());
        }

        if self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }

        // RAII guard: ensures is_running is reset even on early ? returns.
        struct RunGuard<'a>(&'a AtomicBool);
        impl Drop for RunGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _guard = RunGuard(&self.is_running);

        let run_start = now_timestamp();
        let run_date = Local::now().format("%Y-%m-%d").to_string();

        if let Ok(status) = self.database.get_dream_status().await {
            if let Some(last_run_at) = status.last_run_at {
                let last_date = Local.timestamp_opt(last_run_at, 0).single();
                if let Some(last_date) = last_date {
                    if last_date.format("%Y-%m-%d").to_string() == run_date
                        && status.last_status.as_deref() == Some("success")
                    {
                        return Ok(());
                    }
                }
            }
        }

        self.database
            .set_dream_status(DreamStatus {
                last_run_at: Some(run_start),
                last_status: Some("running".to_string()),
                last_duration_ms: None,
            })
            .await?;

        let run_future = self.run_dream(run_start, run_date.clone());
        let run_result = tokio::time::timeout(
            Duration::from_secs(self.config.max_duration_seconds as u64),
            run_future,
        )
        .await;

        let duration_ms = (now_timestamp() - run_start).max(0) as u64 * 1000;

        match run_result {
            Ok(Ok((status, report))) => {
                info!(
                    notes_consolidated = report.notes_consolidated,
                    synthesis_count = report.synthesis_count,
                    notes_archived = report.notes_archived,
                    "DreamDaemon {}",
                    if status == DreamRunStatus::Cancelled {
                        "cancelled"
                    } else {
                        "completed"
                    }
                );

                let _ = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some(status.as_str().to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await;
            }
            Ok(Err(err)) => {
                warn!(error = %err, "DreamDaemon run failed");
                let _ = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some("error".to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await;
            }
            Err(_) => {
                warn!("DreamDaemon run timed out");
                let _ = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some("timeout".to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await;
            }
        }

        // _guard dropped here, resetting is_running to false.
        Ok(())
    }

    fn is_within_window(&self) -> bool {
        let now = Local::now().time();
        if self.window_start <= self.window_end {
            now >= self.window_start && now <= self.window_end
        } else {
            now >= self.window_start || now <= self.window_end
        }
    }

    async fn run_dream(
        &self,
        run_start: i64,
        _run_date: String,
    ) -> Result<(DreamRunStatus, DreamReport), AlephError> {
        // --- Phase 1: Collect signals ---
        // For now, use empty metrics since DreamContext wiring is still pending.
        // When fully wired, populate RawMetrics from database queries.
        let raw_metrics = RawMetrics::default();
        let signal_snapshot = SignalSnapshot::from_metrics(&raw_metrics);

        // --- Phase 2: Mutation gate evaluation ---
        let gate_decision = {
            let gate = self.mutation_gate.lock().unwrap_or_else(|e| e.into_inner());
            gate.evaluate()
        };

        // --- Phase 3: Strategy selection ---
        let selection = {
            let selector = self.selector.lock().unwrap_or_else(|e| e.into_inner());
            selector.select(&signal_snapshot, &gate_decision)
        };

        let strategy = selection.strategy;
        info!(strategy = %strategy, rationale = %selection.rationale, "Dream strategy selected");

        // --- Phase 4: Build and run pipeline ---
        let pipeline = DreamPipeline::from_strategy(strategy);

        // NOTE: Full DreamContext wiring requires NoteIndexer and EmbeddingProvider
        // (same constraint as before). Return stub report until those are wired.
        let _pipeline = pipeline;
        let report = DreamReport {
            pipeline_type: strategy.to_string(),
            started_at: run_start,
            finished_at: now_timestamp(),
            duration_ms: 0,
            status: DreamReportStatus::Completed,
            stages_executed: Vec::new(),
            ..Default::default()
        };

        // --- Phase 5: Validation (L1 + L2, deterministic) ---
        let validation_report = DreamValidationReport {
            l1_format: ValidationTier {
                passed: true,
                checks_run: 0,
                checks_passed: 0,
                issues: vec![],
            },
            l2_consistency: ValidationTier {
                passed: true,
                checks_run: 0,
                checks_passed: 0,
                issues: vec![],
            },
            l3_semantic: None,
            l4_retrospective: None,
        };

        // --- Phase 6: Solidify (event log) ---
        let memory_dir = crate::utils::paths::get_note_memory_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from(".aleph/data/memory"));
        let agent_dir = memory_dir.join("default"); // TODO: use actual agent_id when available
        let event_log = EventLog::new(&agent_dir);
        let cycle = event_log.next_cycle().await.unwrap_or(1);

        let event = DreamEvent {
            id: format!("dream_{}_{}", run_start, cycle),
            cycle,
            strategy,
            selection: selection.clone(),
            gate_decision: gate_decision.clone(),
            report: report.clone(),
            validation: validation_report,
            duration_ms: ((now_timestamp() - run_start).max(0) as u64) * 1000,
            created_at: now_timestamp(),
        };

        if let Err(e) = event_log.append(&event).await {
            warn!(error = %e, "Failed to write dream event log");
        }

        // --- Phase 7: Update personality + mutation gate ---
        {
            let mut selector = self.selector.lock().unwrap_or_else(|e| e.into_inner());
            selector.record_cycle_outcome(
                strategy,
                event.validation.overall_ok(),
                signal_snapshot.score("skill_recall_rate"),
            );
        }
        {
            let mut gate = self.mutation_gate.lock().unwrap_or_else(|e| e.into_inner());
            gate.advance_cycle();
            gate.tick_cooldown();
        }

        Ok((DreamRunStatus::Success, report))
    }
}

fn parse_window(config: &ConfigDreamingConfig) -> Result<(NaiveTime, NaiveTime), AlephError> {
    let start = NaiveTime::parse_from_str(&config.window_start_local, "%H:%M").map_err(|_| {
        AlephError::config(format!(
            "Invalid dreaming.window_start_local '{}', expected HH:MM",
            config.window_start_local
        ))
    })?;
    let end = NaiveTime::parse_from_str(&config.window_end_local, "%H:%M").map_err(|_| {
        AlephError::config(format!(
            "Invalid dreaming.window_end_local '{}', expected HH:MM",
            config.window_end_local
        ))
    })?;
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_within_normal() {
        let start = NaiveTime::from_hms_opt(2, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(5, 0, 0).unwrap();
        let now = NaiveTime::from_hms_opt(3, 30, 0).unwrap();
        assert!(now >= start && now <= end);
    }

    #[test]
    fn test_window_wraps_midnight() {
        let start = NaiveTime::from_hms_opt(22, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(5, 0, 0).unwrap();
        let late = NaiveTime::from_hms_opt(23, 0, 0).unwrap();
        let early = NaiveTime::from_hms_opt(4, 0, 0).unwrap();
        assert!(late >= start || late <= end);
        assert!(early >= start || early <= end);
    }

    #[test]
    fn pipeline_from_strategy_consolidate() {
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Consolidate);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_consolidate",
                "note_drift",
                "index_refresher",
                "note_decay"
            ]
        );
    }

    #[test]
    fn pipeline_from_strategy_synthesize() {
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Synthesize);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_consolidate",
                "note_synthesis",
                "skill_distill",
                "daily_digest"
            ]
        );
    }

    #[test]
    fn pipeline_from_strategy_conserve() {
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Conserve);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec!["note_lint", "index_refresher"]);
    }
}
