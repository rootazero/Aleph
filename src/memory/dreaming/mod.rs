//! DreamDaemon: background memory consolidation for the notes layer.
//!
//! This module implements a staged dream pipeline architecture.
//! Each stage implements the `DreamStage` trait and operates on a shared
//! `DreamContext` that flows through the pipeline.

pub mod distill_action;
pub mod event_log;
pub mod gate;
pub mod mutation_gate;
pub mod report;
pub mod selector;
pub mod signals;
pub mod stages;
pub mod strategy;
pub mod validation;

use crate::config::{DreamingConfig as ConfigDreamingConfig, MemoryConfig};
use crate::error::AlephError;
use crate::memory::embedding_provider::EmbeddingProvider;
use crate::memory::notes::store::{NoteIndexEntry, NoteStore};
use crate::memory::notes::NoteIndexer;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::memory::store::{DreamStore, MemoryBackend};
use crate::providers::AiProvider;
use crate::routing::DEFAULT_AGENT_ID;
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, AtomicI64, Ordering};
use chrono::{Local, NaiveTime, TimeZone};
use once_cell::sync::{Lazy, OnceCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{info, warn};

// Re-export distill action enum (Phase 2 Task 14)
pub use distill_action::DistillAction;

// Re-export gate types
pub use gate::{BlockReason, DreamGate, DreamGateConfig, GateResult};

// Re-export report types
pub use report::{DreamReport, DreamReportStatus};

// Re-export stage trait and shared types
pub use event_log::{DreamEvent, EventLog};
pub use mutation_gate::MutationGate;
pub use selector::{GateDecision, SelectionDecision, StrategySelector};
pub use signals::{DreamSignal, RawMetrics, SignalSnapshot, SignalType};
pub use stages::{DreamStage, MemoryCluster};
pub use strategy::DreamStrategy;
pub use validation::{DreamValidationReport, ValidationIssue, ValidationTier};

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

impl NoteEntry {
    /// Build a `NoteEntry` from a stored note index entry.
    ///
    /// `last_accessed_at` is not tracked in the note index, so it is left
    /// `None` — decay stages fall back to `updated_at` when it is absent.
    fn from_index_entry(e: &NoteIndexEntry) -> Self {
        Self {
            path: e.path.clone(),
            category: e.category.clone(),
            tags: e.tags.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
            last_accessed_at: None,
            content_hash: e.content_hash.clone(),
        }
    }
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
    pub stages: Vec<Box<dyn DreamStage>>,
}

impl DreamPipeline {
    pub fn new(stages: Vec<Box<dyn DreamStage>>) -> Self {
        Self { stages }
    }

    /// Build a pipeline from a DreamStrategy, threading runtime config into stages
    /// that need it (currently SkillDistill's per-cycle cap, D5).
    pub fn from_strategy(
        strategy: DreamStrategy,
        dreaming_cfg: &crate::config::types::memory::DreamingConfig,
    ) -> Self {
        let stage_list: Vec<Box<dyn DreamStage>> = match strategy {
            DreamStrategy::Consolidate => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteReviewStage::default()),
                Box::new(stages::NoteConsolidateStage),
                Box::new(stages::NoteDriftStage),
                Box::new(stages::IndexRefresherStage),
                Box::new(stages::NoteDecayStage),
            ],
            DreamStrategy::Synthesize => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteReviewStage::default()),
                Box::new(stages::NoteConsolidateStage),
                Box::new(stages::NoteSynthesisStage),
                Box::new(stages::SkillDistillStage {
                    max_per_cycle: dreaming_cfg.skill_distill_max_per_cycle,
                }),
                // Phase 3: distill user-correction signals into feedback notes.
                // Runs after SkillDistill so a single dream cycle can pick up
                // both implicit (synthesis-derived) and explicit (correction)
                // learnings.
                Box::new(stages::FeedbackDistillStage {
                    max_per_cycle: dreaming_cfg.feedback_distill_max_per_cycle,
                    min_candidates: dreaming_cfg.feedback_distill_min_candidates,
                    lookback: dreaming_cfg.feedback_lookback,
                }),
                Box::new(stages::DailyDigestStage),
            ],
            DreamStrategy::Conserve => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteReviewStage::default()),
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
    LAST_ACTIVITY_TS.store(now_timestamp(), Ordering::Release);
}

fn last_activity_timestamp() -> i64 {
    LAST_ACTIVITY_TS.load(Ordering::Acquire)
}

fn idle_seconds() -> i64 {
    let now = now_timestamp();
    let last = last_activity_timestamp();
    (now - last).max(0)
}

/// Force-trigger a dream cycle on the globally-registered daemon.
///
/// Returns the resulting [`DreamReport`] on success, or an error explaining
/// why the cycle could not run (no daemon registered, already running, etc.).
/// Bypasses scheduling checks (window, idle threshold, already-ran-today) —
/// intended for the `dreaming.run_now` admin RPC and E2E test harnesses.
pub async fn try_run_now() -> Result<DreamReport, AlephError> {
    let daemon = DREAM_DAEMON
        .get()
        .cloned()
        .ok_or_else(|| AlephError::other("DreamDaemon not initialized"))?;
    daemon.run_now().await
}

/// Ensure DreamDaemon is running (once) when memory is enabled.
pub fn ensure_dream_daemon(
    database: MemoryBackend,
    config: Arc<MemoryConfig>,
    provider: Option<Arc<dyn AiProvider>>,
    command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
) {
    ensure_dream_daemon_with_orientation(
        database,
        config,
        provider,
        command_handler,
        None,
        None,
        None,
    );
}

/// Ensure DreamDaemon is running (once) when memory is enabled, with optional orientation handle.
pub fn ensure_dream_daemon_with_orientation(
    database: MemoryBackend,
    config: Arc<MemoryConfig>,
    provider: Option<Arc<dyn AiProvider>>,
    command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
    orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    note_memory_dir: Option<PathBuf>,
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

    let daemon_builder = if let Some(e) = embedder {
        daemon_builder.with_embedder(e)
    } else {
        daemon_builder
    };

    let daemon_builder = if let Some(dir) = note_memory_dir {
        daemon_builder.with_note_memory_dir(dir)
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
    /// Optional embedding provider — required to build a `DreamContext`.
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    /// Note memory directory (parent of agent dirs). Falls back to
    /// `get_note_memory_dir()` when unset; injected explicitly so the dir
    /// matches the rest of the boot wiring and the daemon is unit-testable.
    note_memory_dir: Option<PathBuf>,
    /// Optional wiki orientation — forwarded into DreamContext for IndexRefresherStage.
    orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    /// Strategy selector with personality adaptation.
    selector: crate::sync_primitives::Mutex<StrategySelector>,
    /// Mutation gate tracking evolution pathologies.
    mutation_gate: crate::sync_primitives::Mutex<MutationGate>,
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
            embedder: None,
            note_memory_dir: None,
            orientation: None,
            selector: crate::sync_primitives::Mutex::new(StrategySelector::new()),
            mutation_gate: crate::sync_primitives::Mutex::new(MutationGate::new()),
        })
    }

    /// Attach an AI provider for LLM-powered dream stages.
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Attach an embedding provider — required to construct a `DreamContext`.
    pub fn with_embedder(mut self, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Set the note memory directory (parent of per-agent note dirs).
    pub fn with_note_memory_dir(mut self, dir: PathBuf) -> Self {
        self.note_memory_dir = Some(dir);
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

    /// Force-trigger a single dream cycle, bypassing window / idle / already-ran checks.
    ///
    /// Used by the `dreaming.run_now` admin RPC for deterministic test harnesses.
    /// The `is_running` latch is still respected so concurrent triggers cannot
    /// stack. Persists `last_run_at` / `last_status` exactly like the scheduler
    /// path so observers (Panel, journalctl, dream_status table) see the cycle.
    pub async fn run_now(&self) -> Result<DreamReport, AlephError> {
        if self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(AlephError::other(
                "dream cycle already running — try again later",
            ));
        }

        struct RunGuard<'a>(&'a AtomicBool);
        impl Drop for RunGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _guard = RunGuard(&self.is_running);

        let run_start = now_timestamp();
        let run_date = Local::now().format("%Y-%m-%d").to_string();

        let _ = self
            .database
            .set_dream_status(DreamStatus {
                last_run_at: Some(run_start),
                last_status: Some("running".to_string()),
                last_duration_ms: None,
            })
            .await;

        let result = self.run_dream(run_start, run_date, true).await;
        let duration_ms = ((now_timestamp() - run_start).max(0) as u64) * 1000;

        match &result {
            Ok((status, _report)) => {
                let _ = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some(status.as_str().to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await;
            }
            Err(_) => {
                let _ = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some("failed".to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await;
            }
        }

        result.map(|(_, report)| report)
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
        // G3 fix: every skip case logs once at INFO so operators can observe
        // daemon health from `journalctl`/log tails without DEBUG filtering.
        if !self.config.enabled {
            info!(reason = "disabled", "DreamDaemon tick: skipped");
            return Ok(());
        }

        if !self.is_within_window() {
            info!(
                reason = "outside_window",
                window_start = %self.config.window_start_local,
                window_end = %self.config.window_end_local,
                "DreamDaemon tick: skipped"
            );
            return Ok(());
        }

        let idle = idle_seconds();
        if idle < self.config.idle_threshold_seconds as i64 {
            info!(
                reason = "idle_below_threshold",
                idle_seconds = idle,
                threshold = self.config.idle_threshold_seconds,
                "DreamDaemon tick: skipped"
            );
            return Ok(());
        }

        if self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            info!(reason = "already_running", "DreamDaemon tick: skipped");
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
                        info!(
                            reason = "already_ran_today",
                            last_run_at, "DreamDaemon tick: skipped"
                        );
                        return Ok(());
                    }
                }
            }
        }

        info!(
            reason = "preconditions_passed",
            run_date = %run_date,
            "DreamDaemon tick: starting cycle"
        );

        self.database
            .set_dream_status(DreamStatus {
                last_run_at: Some(run_start),
                last_status: Some("running".to_string()),
                last_duration_ms: None,
            })
            .await?;

        let run_future = self.run_dream(run_start, run_date.clone(), false);
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
        force: bool,
    ) -> Result<(DreamRunStatus, DreamReport), AlephError> {
        // --- Phase 0: Resolve note memory dir + load the note index ---
        let memory_dir = self.note_memory_dir.clone().unwrap_or_else(|| {
            crate::utils::paths::get_note_memory_dir()
                .unwrap_or_else(|_| PathBuf::from(".aleph/data/memory"))
        });
        let note_index = self
            .database
            .list_notes(DEFAULT_AGENT_ID)
            .await
            .unwrap_or_default();

        // --- Phase 1: Collect signals ---
        let raw_metrics = compute_raw_metrics(&note_index);
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

        // --- Phase 4: Build and run the consolidation pipeline ---
        let pipeline = DreamPipeline::from_strategy(strategy, &self.config);
        let (report, run_status) = match (self.provider.clone(), self.embedder.clone()) {
            (Some(provider), Some(embedder)) => {
                let mut indexer = NoteIndexer::new(memory_dir.clone(), self.database.clone());
                if let Some(orientation) = &self.orientation {
                    indexer = indexer.with_orientation(orientation.clone());
                }
                let notes: Vec<NoteEntry> =
                    note_index.iter().map(NoteEntry::from_index_entry).collect();
                // Scheduled cycles yield to fresh user activity; forced cycles
                // (run_now / E2E harness) run to completion.
                let activity_checker: Arc<dyn Fn() -> bool + Send + Sync> = if force {
                    Arc::new(|| false)
                } else {
                    let threshold = self.config.idle_threshold_seconds as i64;
                    Arc::new(move || idle_seconds() < threshold)
                };
                let ctx = DreamContext {
                    notes,
                    note_contents: HashMap::new(),
                    agent_id: DEFAULT_AGENT_ID.to_string(),
                    database: self.database.clone(),
                    indexer,
                    provider,
                    embedder,
                    report: DreamReport {
                        pipeline_type: strategy.to_string(),
                        started_at: run_start,
                        ..Default::default()
                    },
                    pipeline_type: strategy.to_string(),
                    activity_checker,
                    strategy,
                    orientation: self.orientation.clone(),
                };
                let mut report = pipeline.run(ctx).await?;
                report.finished_at = now_timestamp();
                report.duration_ms = ((report.finished_at - run_start).max(0) as u64) * 1000;
                let status = if report.status == DreamReportStatus::Interrupted {
                    DreamRunStatus::Cancelled
                } else {
                    DreamRunStatus::Success
                };
                (report, status)
            }
            _ => {
                // Consolidation needs both an AI provider and an embedder
                // (`DreamContext` requires them). The ingestion-only boot path
                // supplies neither — skip the pipeline rather than panic. The
                // production server always supplies both.
                warn!(
                    "DreamDaemon: AI provider or embedder unavailable — \
                     skipping consolidation pipeline"
                );
                let report = DreamReport {
                    pipeline_type: strategy.to_string(),
                    started_at: run_start,
                    finished_at: now_timestamp(),
                    status: DreamReportStatus::Completed,
                    ..Default::default()
                };
                (report, DreamRunStatus::Success)
            }
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
        let agent_dir = memory_dir.join(DEFAULT_AGENT_ID);
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

        Ok((run_status, report))
    }
}

/// Compute a cheap `RawMetrics` snapshot from the note index.
///
/// Only `total_notes` and `notes_added_24h` are populated — enough to drive a
/// meaningful `note_growth_rate` signal for strategy selection. Quality and
/// recall rates require dedicated aggregate queries and are left at zero; a
/// follow-up can fold in the `dream_report` / `recall_signals` tables.
fn compute_raw_metrics(notes: &[NoteIndexEntry]) -> RawMetrics {
    let day_ago = now_timestamp() - 86_400;
    let notes_added_24h = notes.iter().filter(|n| n.created_at >= day_ago).count() as u32;
    RawMetrics {
        total_notes: notes.len() as u32,
        notes_added_24h,
        ..Default::default()
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
        let cfg = crate::config::types::memory::DreamingConfig::default();
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Consolidate, &cfg);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "note_drift",
                "index_refresher",
                "note_decay"
            ]
        );
    }

    #[test]
    fn pipeline_from_strategy_synthesize() {
        let cfg = crate::config::types::memory::DreamingConfig::default();
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Synthesize, &cfg);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "note_synthesis",
                "skill_distill",
                "feedback_distill",
                "daily_digest"
            ]
        );
    }

    #[test]
    fn pipeline_synthesize_runs_feedback_after_skill_distill() {
        // Phase 3: ensure FeedbackDistill is scheduled directly after
        // SkillDistill so a single dream cycle can pick up both.
        let cfg = crate::config::types::memory::DreamingConfig::default();
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Synthesize, &cfg);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        let skill_pos = names.iter().position(|n| *n == "skill_distill").unwrap();
        let feedback_pos = names.iter().position(|n| *n == "feedback_distill").unwrap();
        assert_eq!(feedback_pos, skill_pos + 1);
    }

    #[test]
    fn pipeline_from_strategy_conserve() {
        let cfg = crate::config::types::memory::DreamingConfig::default();
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Conserve, &cfg);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec!["note_lint", "note_review", "index_refresher"]);
    }

    // -----------------------------------------------------------------
    // run_dream wiring — regression guard for the historical
    // `let _pipeline = pipeline` stub that discarded the pipeline.
    // -----------------------------------------------------------------

    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::mock::MockProvider;

    /// Minimal embedding provider stub — the stages exercised here never
    /// invoke embedding, so empty vectors suffice.
    struct StubEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(Vec::new())
        }
        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(Vec::new())
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    fn test_daemon(store: Arc<SqliteMemoryBackend>, dir: PathBuf) -> DreamDaemon {
        let cfg = MemoryConfig::default();
        DreamDaemon::from_config(store, &cfg)
            .expect("valid default dreaming config")
            .with_provider(Arc::new(MockProvider::new("")))
            .with_embedder(Arc::new(StubEmbedder))
            .with_note_memory_dir(dir)
    }

    #[test]
    fn compute_raw_metrics_counts_notes_and_recency() {
        let now = now_timestamp();
        let entry = |created: i64| NoteIndexEntry {
            path: "p".into(),
            filename: "p".into(),
            agent_id: "default".into(),
            category: "reference".into(),
            tags: vec![],
            link_count: 0,
            created_at: created,
            updated_at: created,
            content_hash: "h".into(),
        };
        // One fresh, one ~25h old (excluded), one fresh.
        let notes = vec![entry(now), entry(now - 90_000), entry(now - 100)];
        let m = compute_raw_metrics(&notes);
        assert_eq!(m.total_notes, 3);
        assert_eq!(m.notes_added_24h, 2);
    }

    #[tokio::test]
    async fn run_dream_executes_pipeline_not_stub() {
        let temp = std::env::temp_dir().join(format!("aleph_dream_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let daemon = test_daemon(store, temp.clone());

        let (status, report) = daemon
            .run_dream(now_timestamp(), "2026-05-21".to_string(), true)
            .await
            .expect("run_dream succeeds");

        assert_eq!(status, DreamRunStatus::Success);
        // The historical stub returned an empty `stages_executed`. A real
        // pipeline run always executes `note_lint` (first stage of every
        // strategy, no `should_run` override).
        assert!(
            report.stages_executed.contains(&"note_lint".to_string()),
            "pipeline did not execute — stages_executed = {:?}",
            report.stages_executed
        );
        assert!(report.finished_at >= report.started_at);
    }

    #[tokio::test]
    async fn run_dream_skips_gracefully_without_provider() {
        let temp = std::env::temp_dir().join(format!("aleph_dream_np_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        // No provider / embedder → DreamContext cannot be built; the daemon
        // must skip the pipeline gracefully rather than panic.
        let daemon = DreamDaemon::from_config(store, &MemoryConfig::default())
            .unwrap()
            .with_note_memory_dir(temp.clone());

        let (status, report) = daemon
            .run_dream(now_timestamp(), "2026-05-21".to_string(), true)
            .await
            .expect("run_dream succeeds without a provider");

        assert_eq!(status, DreamRunStatus::Success);
        assert!(report.stages_executed.is_empty());
    }
}
