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
pub mod skill_gate;
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
pub use distill_action::{DistillAction, DistillActionRecord, DistillOutcome};

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
                // Distill user-correction signals on the FREQUENT consolidate
                // path (not just the rarer synthesize path), so a freshly
                // flagged correction becomes a recallable feedback rule within
                // a day instead of waiting for a high-growth synthesize cycle.
                // Watermark + min_candidates gating make this a cheap no-op when
                // there are no new corrections, so no extra LLM call is incurred.
                Box::new(stages::FeedbackDistillStage {
                    max_per_cycle: dreaming_cfg.feedback_distill_max_per_cycle,
                    min_candidates: dreaming_cfg.feedback_distill_min_candidates,
                    lookback: dreaming_cfg.feedback_lookback,
                }),
                Box::new(stages::NoteDriftStage),
                Box::new(stages::IndexRefresherStage),
                Box::new(stages::NoteDecayStage),
                // System-level skill aging (rule-based Active→Stale at
                // `skill_stale_after_days`). The Stale→Archived / merge
                // decisions live in a future LLM-driven curator stage —
                // see `SkillLifecycleStage` for the R7 boundary.
                Box::new(stages::SkillLifecycleStage {
                    stale_after_days: dreaming_cfg.skill_stale_after_days,
                }),
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
                // learnings. Also present on the Consolidate path so distillation
                // happens daily; only one strategy runs per cycle, so there is no
                // double execution.
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

    /// Stages that operate on global, cross-project state and therefore run
    /// only for the base agent — never per project namespace:
    /// - `feedback_distill`: the user-feedback floor is always-on and global
    ///   (a project must not fork the floor — see `project_scope`).
    /// - `skill_lifecycle`: ages skills in the global usage store.
    /// - `daily_digest`: writes a single global daily insight.
    const GLOBAL_ONLY_STAGES: &'static [&'static str] =
        &["feedback_distill", "skill_lifecycle", "daily_digest"];

    /// Drop the global-only stages, leaving the note-maintenance subset that is
    /// safe to run per project namespace. Built from the same `from_strategy`
    /// list so the project fan-out never drifts from the base pipeline.
    pub fn retain_project_stages(mut self) -> Self {
        self.stages
            .retain(|s| !Self::GLOBAL_ONLY_STAGES.contains(&s.name()));
        self
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
    /// Whether per-project memory namespacing is enabled (mirrors
    /// `MemoryConfig.project_scoped`). When on, the daemon additionally fans
    /// the note-maintenance stages over each `{base}__proj-*` namespace so
    /// project-local notes written by `note_manage` are linted, consolidated
    /// and synthesised too. Default-off → no fan-out → unchanged behaviour.
    project_scoped: bool,
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
            project_scoped: config.project_scoped,
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
        let raw_metrics =
            compute_raw_metrics(&note_index, self.database.as_ref(), DEFAULT_AGENT_ID).await;
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
                    provider: provider.clone(),
                    embedder: embedder.clone(),
                    report: DreamReport {
                        pipeline_type: strategy.to_string(),
                        started_at: run_start,
                        ..Default::default()
                    },
                    pipeline_type: strategy.to_string(),
                    activity_checker: activity_checker.clone(),
                    strategy,
                    orientation: self.orientation.clone(),
                };
                let mut report = pipeline.run(ctx).await?;

                // Per-project namespace maintenance (gated). The base agent ran
                // the full pipeline above; project namespaces created by
                // `note_manage` under `{base}__proj-*` get the note-maintenance
                // subset so their notes are linted/consolidated/synthesised too.
                // The global-only stages (feedback floor, skill lifecycle, daily
                // digest) are excluded — those stay cross-project. Per-namespace
                // failures are logged, never aborting the base cycle.
                if self.project_scoped {
                    let scoped = crate::memory::project_scope::list_scoped_agent_ids(
                        &memory_dir,
                        DEFAULT_AGENT_ID,
                    );
                    if !scoped.is_empty() {
                        let project_pipeline =
                            DreamPipeline::from_strategy(strategy, &self.config)
                                .retain_project_stages();
                        for ns in &scoped {
                            let ns_index =
                                self.database.list_notes(ns).await.unwrap_or_default();
                            let ns_notes: Vec<NoteEntry> =
                                ns_index.iter().map(NoteEntry::from_index_entry).collect();
                            let mut ns_indexer =
                                NoteIndexer::new(memory_dir.clone(), self.database.clone());
                            if let Some(orientation) = &self.orientation {
                                ns_indexer = ns_indexer.with_orientation(orientation.clone());
                            }
                            let ns_ctx = DreamContext {
                                notes: ns_notes,
                                note_contents: HashMap::new(),
                                agent_id: ns.clone(),
                                database: self.database.clone(),
                                indexer: ns_indexer,
                                provider: provider.clone(),
                                embedder: embedder.clone(),
                                report: DreamReport {
                                    pipeline_type: strategy.to_string(),
                                    started_at: run_start,
                                    ..Default::default()
                                },
                                pipeline_type: strategy.to_string(),
                                activity_checker: activity_checker.clone(),
                                strategy,
                                orientation: self.orientation.clone(),
                            };
                            match project_pipeline.run(ns_ctx).await {
                                Ok(r) => info!(
                                    agent = %ns,
                                    stages = ?r.stages_executed,
                                    "project namespace dream complete"
                                ),
                                Err(e) => warn!(
                                    agent = %ns,
                                    error = %e,
                                    "project namespace dream failed"
                                ),
                            }
                        }
                    }
                }

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

/// Compute a `RawMetrics` snapshot for the Dream cycle.
///
/// Pulls notes (count + 24h growth) from the in-memory note index, then
/// folds in 24h tool-invocation aggregates from `raw_memories` so the
/// signal collector can surface `tool_failure_rate` / `tool_call_volume` /
/// `tool_latency_score` (Spec 3). A backend failure on the tool query is
/// downgraded to a warning — strategy selection runs on the note signals
/// alone rather than aborting the whole cycle.
///
/// Quality and recall rates still require dedicated aggregate queries and
/// remain at zero; a follow-up can fold in the `dream_report` /
/// `recall_signals` tables.
async fn compute_raw_metrics(
    notes: &[NoteIndexEntry],
    store: &dyn crate::memory::store::raw_memory::RawMemoryStore,
    agent_id: &str,
) -> RawMetrics {
    let day_ago = now_timestamp() - 86_400;
    let notes_added_24h = notes.iter().filter(|n| n.created_at >= day_ago).count() as u32;

    let (tool_total, tool_failed, tool_avg_ms) =
        match crate::memory::tool_signal_sink::aggregate_tool_stats(store, agent_id, day_ago, 5_000)
            .await
        {
            Ok(stats) => (stats.total, stats.failed, stats.avg_duration_ms),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    agent = agent_id,
                    "tool-invocation aggregation failed; tool signals will read zero",
                );
                (0, 0, 0)
            }
        };

    RawMetrics {
        total_notes: notes.len() as u32,
        notes_added_24h,
        tool_calls_total_24h: tool_total,
        tool_calls_failed_24h: tool_failed,
        tool_avg_duration_ms_24h: tool_avg_ms,
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
                "feedback_distill",
                "note_drift",
                "index_refresher",
                "note_decay",
                "skill_lifecycle",
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
    fn retain_project_stages_drops_global_only_stages() {
        let cfg = crate::config::types::memory::DreamingConfig::default();
        // Synthesize is the richest pipeline — it contains all three
        // global-only stages plus the note-maintenance subset.
        let project = DreamPipeline::from_strategy(DreamStrategy::Synthesize, &cfg)
            .retain_project_stages();
        let names: Vec<&str> = project.stages.iter().map(|s| s.name()).collect();
        // The three global-only stages must be gone...
        for global in DreamPipeline::GLOBAL_ONLY_STAGES {
            assert!(
                !names.contains(global),
                "{global} must not run per project namespace"
            );
        }
        // ...and the note-maintenance subset must remain, in order.
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "note_synthesis",
                "skill_distill",
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

    #[tokio::test]
    async fn compute_raw_metrics_counts_notes_and_recency() {
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
        let temp = std::env::temp_dir().join(format!("aleph_metrics_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let m = compute_raw_metrics(&notes, store.as_ref(), DEFAULT_AGENT_ID).await;
        assert_eq!(m.total_notes, 3);
        assert_eq!(m.notes_added_24h, 2);
        // With no tool invocations recorded yet, the new fields stay zero.
        assert_eq!(m.tool_calls_total_24h, 0);
        assert_eq!(m.tool_calls_failed_24h, 0);
        assert_eq!(m.tool_avg_duration_ms_24h, 0);
    }

    #[tokio::test]
    async fn compute_raw_metrics_folds_in_tool_invocation_stats() {
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
        let temp =
            std::env::temp_dir().join(format!("aleph_metrics_tool_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());

        // Synthesize three fresh tool invocations: 2 ok (10 + 30 ms) + 1 fail.
        for (name, ok, ms) in [
            ("read_file", true, 10u64),
            ("shell", true, 30),
            ("grep", false, 0),
        ] {
            let raw = RawMemory::new(
                format!("tool {name}"),
                RawMemorySource::ToolInvocation {
                    tool_name: name.into(),
                    success: ok,
                    duration_ms: ms,
                },
            )
            .with_agent(DEFAULT_AGENT_ID);
            store.insert_raw_memory(&raw).await.unwrap();
        }

        let m = compute_raw_metrics(&[], store.as_ref(), DEFAULT_AGENT_ID).await;
        assert_eq!(m.tool_calls_total_24h, 3);
        assert_eq!(m.tool_calls_failed_24h, 1);
        // (10 + 30 + 0) / 3 = 13
        assert_eq!(m.tool_avg_duration_ms_24h, 40 / 3);
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
