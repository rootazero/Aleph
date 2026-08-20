//! `DreamDaemon`: background memory consolidation for the notes layer.
//!
//! This module implements a staged dream pipeline architecture.
//! Each stage implements the `DreamStage` trait and operates on a shared
//! `DreamContext` that flows through the pipeline.

pub mod distill_action;
pub mod event_log;
pub mod evolution;
pub mod mutation_gate;
mod project_cycle;
pub mod report;
pub mod selector;
pub mod signals;
pub mod skill_gate;
pub mod stages;
pub mod strategy;
pub mod validation;

use crate::config::types::memory::MemoryDecayPolicy;
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
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{info, warn};

// Re-export distill action enum (Phase 2 Task 14)
pub use distill_action::{DistillAction, DistillActionRecord, DistillOutcome};

// Re-export report types
pub use report::{CycleDecision, DreamReport, DreamReportStatus};

// Re-export stage trait and shared types
pub use event_log::{DreamEvent, EventLog};
pub use evolution::{
    evaluate_gate, memory_health_score, score_merge_candidate, EditBudget, EvolutionOutcome,
    GateOutcome,
};
pub use mutation_gate::MutationGate;
pub use selector::{GateDecision, SelectionDecision, StrategySelector};
pub use signals::{DreamSignal, RawMetrics, SignalSnapshot, SignalType};
pub use stages::DreamStage;
pub use strategy::DreamStrategy;
pub use validation::{DreamValidationReport, ValidationIssue, ValidationTier};

/// How many past dream cycles the daemon rehydrates at the start of each run.
///
/// Derived from the consumers' own declared windows so the read can never drift
/// out of sync with what the detectors need: widening a detector's window
/// widens this automatically.
const DREAM_HISTORY_WINDOW: usize =
    if MutationGate::HISTORY_WINDOW > StrategySelector::HISTORY_WINDOW {
        MutationGate::HISTORY_WINDOW
    } else {
        StrategySelector::HISTORY_WINDOW
    };

/// Ceiling on how many non-base corpora may run a maintenance sub-cycle in one
/// night.
///
/// The base agent is never counted — it always dreams. This bounds the *fan-out*
/// axis, which is the one the nightly bill now grows along: one corpus exists
/// per user partition, per room partition, per project directory and per
/// non-default agent, and each admitted corpus costs one maintenance pipeline
/// (whose own per-stage caps — `skill_distill_max_per_cycle`,
/// `feedback_distill_max_per_cycle`, `synthesis_max_insights`,
/// `drift_max_pairs_per_run` — already bound the calls *within* a corpus).
///
/// Deliberately conservative: unmaintained corpora are not lost, they wait for
/// the next window, and a corpus that skipped because nothing changed spends
/// none of this budget. Corpora are visited in enumeration order; the skip gate
/// (`project_cycle::corpus_needs_maintenance`) is what keeps that order from
/// starving the tail, because a corpus already maintained is skipped for free on
/// the next night and lets the queue drain.
///
/// **This is the DEFAULT, not the live value.** The fan-out reads
/// `[memory.dreaming] max_corpus_cycles_per_night`; this constant is the number
/// that knob falls back to when unset, and it is referenced directly by
/// `config::types::memory::defaults::default_dream_max_corpus_cycles_per_night`
/// — one number, one definition, checked by the type system rather than by a
/// source-scanning guard.
pub(crate) const MAX_CORPUS_CYCLES_PER_NIGHT: usize = 8;

/// Every note corpus that the nightly fan-out should offer a maintenance cycle:
/// all corpora on disk except `base`, which ran the full pipeline itself.
///
/// One function so "which corpora get maintained" has exactly one answer, and it
/// is derived from [`list_note_corpora`] — the single source for "which corpora
/// exist" — rather than from a config flag that only ever described one of the
/// four ways a corpus comes into being.
///
/// [`list_note_corpora`]: crate::memory::project_scope::list_note_corpora
fn maintenance_corpora(memory_dir: &Path, base: &str) -> Vec<String> {
    crate::memory::project_scope::list_note_corpora(memory_dir)
        .into_iter()
        .filter(|id| id != base)
        .collect()
}

/// Everything one completed dream cycle produced: the counters, the run status,
/// and the decision that led to them.
///
/// The decision travels with the report because both persistence paths
/// (`check_and_run` and `run_now`) must write it — a cycle's "why" is not a
/// property of how it was triggered.
#[derive(Debug, Clone)]
pub(crate) struct DreamCycleOutcome {
    pub status: DreamRunStatus,
    pub report: DreamReport,
    pub decision: CycleDecision,
}

// ---------------------------------------------------------------------------
// NoteEntry — metadata for a single note in the dream pipeline
// ---------------------------------------------------------------------------

/// Metadata for a single note in the dream pipeline.
///
/// Recall recency is not carried here: `NoteDecayStage` reads it directly
/// from `recall_signals` (the live access-tracking source) when scoring.
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub path: String,
    pub category: String,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub content_hash: String,
}

impl NoteEntry {
    /// Build a `NoteEntry` from a stored note index entry.
    fn from_index_entry(e: &NoteIndexEntry) -> Self {
        Self {
            // rust-doctor-disable-next-line excessive-clone
            path: e.path.clone(),
            // rust-doctor-disable-next-line excessive-clone
            category: e.category.clone(),
            // rust-doctor-disable-next-line excessive-clone
            tags: e.tags.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
            // rust-doctor-disable-next-line excessive-clone
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
    /// Per-cycle edit budget ("textual learning rate") bounding how much memory
    /// destructive stages may rewrite this cycle. Shared across `NoteConsolidate`
    /// (merges), `NoteDecay` (archival) and the distill `Supersede` action
    /// (`SkillDistill` / `FeedbackDistill`); additive growth is not budgeted.
    pub evolution_budget: EditBudget,
}

impl DreamContext {
    /// Lazy-load a note's markdown content from disk.
    pub async fn load_content(&mut self, path: &str) -> Option<String> {
        if let Some(content) = self.note_contents.get(path) {
            // rust-doctor-disable-next-line excessive-clone
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
        // rust-doctor-disable-next-line excessive-clone
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
    #[must_use]
    pub fn new(stages: Vec<Box<dyn DreamStage>>) -> Self {
        Self { stages }
    }

    /// Build a pipeline from a `DreamStrategy`, threading runtime config into stages
    /// that need it (currently `SkillDistill`'s per-cycle cap, D5).
    #[must_use]
    pub fn from_strategy(
        strategy: DreamStrategy,
        dreaming_cfg: &crate::config::types::memory::DreamingConfig,
        decay_policy: &MemoryDecayPolicy,
    ) -> Self {
        let note_decay = || stages::NoteDecayStage {
            half_life_days: decay_policy.half_life_days,
            min_strength: decay_policy.min_strength,
            // rust-doctor-disable-next-line excessive-clone
            protected_types: decay_policy.protected_types.clone(),
        };
        let stage_list: Vec<Box<dyn DreamStage>> = match strategy {
            DreamStrategy::Consolidate => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteReviewStage::default()),
                Box::new(stages::NoteConsolidateStage {
                    max_pairs: dreaming_cfg.consolidate_max_pairs_per_run,
                }),
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
                // The failure rail, alongside the correction rail. Same
                // frequent path for the same reason: a failure pattern is worth
                // knowing about the next day, not the next high-growth cycle.
                // Its own watermark + failure quorum make it a free no-op when
                // nothing new broke, so this costs no LLM call on a quiet night.
                Box::new(stages::ToolFailureDistillStage::default()),
                Box::new(stages::NoteDriftStage {
                    max_pairs: dreaming_cfg.drift_max_pairs_per_run,
                }),
                Box::new(stages::IndexRefresherStage),
                // Materialize behavioral co-recall edges BEFORE the graph
                // recompute so the 5-signal relevance pass sees them this
                // cycle. Pure deterministic aggregation, zero LLM.
                Box::new(stages::CoRecallEdgesStage),
                // Materialize the note knowledge graph (community/cohesion +
                // insights) BEFORE weave/decay consume it: weave reads the
                // freshly-computed `isolated` set and decay benefits from the
                // recomputed link topology. Pure deterministic, zero LLM.
                Box::new(stages::GraphRecomputeStage),
                // Weave orphan notes into the link graph BEFORE decay scores
                // them: a freshly woven link immediately counts toward
                // link_weight / the >=3-incoming-links protection, breaking
                // the orphan→no-link-weight→archived vicious cycle.
                Box::new(stages::NoteWeaveStage::default()),
                // Materialize unlinked-mention soft edges AFTER weave (real
                // links win) and BEFORE decay (mention edges count toward
                // link_weight the same cycle). Deterministic, zero LLM.
                Box::new(stages::MentionWeaveStage),
                Box::new(note_decay()),
                // System-level skill aging (rule-based Active→Stale at
                // `skill_stale_after_days`). The Stale→Archived / merge
                // decisions live in a future LLM-driven curator stage —
                // see `SkillLifecycleStage` for the R7 boundary.
                Box::new(stages::SkillLifecycleStage {
                    stale_after_days: dreaming_cfg.skill_stale_after_days,
                }),
                // Graduate goal lessons (Round 2 state file) into durable notes
                // so insights survive the ring buffer and goal deletion. Cheap
                // no-op when no goal has new lessons. Global-only (goals are not
                // project-namespaced).
                Box::new(stages::GoalLessonsPromoteStage::default()),
            ],
            DreamStrategy::Synthesize => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteReviewStage::default()),
                Box::new(stages::NoteConsolidateStage {
                    max_pairs: dreaming_cfg.consolidate_max_pairs_per_run,
                }),
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
                // Present on BOTH LLM paths, like FeedbackDistill: only one
                // strategy runs per cycle, so a stage that lives on only one of
                // them silently stops distilling for as long as the selector
                // prefers the other.
                Box::new(stages::ToolFailureDistillStage::default()),
                // System-level co-occurrence mining: draft gated MetaSkill
                // (workflow) proposals from recurring skill chains. Pure
                // deterministic aggregation (no LLM) on the rarer high-growth
                // synthesize path — see `WorkflowProposalStage` for the R7
                // boundary. Drafts are gated; nothing auto-activates.
                Box::new(stages::WorkflowProposalStage::default()),
                Box::new(stages::CorpusNarrativeStage),
                Box::new(stages::DailyDigestStage),
            ],
            DreamStrategy::Conserve => vec![
                Box::new(stages::NoteLintStage),
                Box::new(stages::NoteReviewStage::default()),
                Box::new(stages::IndexRefresherStage),
                Box::new(stages::CoRecallEdgesStage),
                Box::new(stages::GraphRecomputeStage),
            ],
        };
        Self::new(stage_list)
    }

    /// Stages that operate on global, cross-project state and therefore run
    /// only for the base agent — never per project namespace:
    /// - `skill_lifecycle`: ages skills in the global usage store.
    /// - `daily_digest`: writes a single global daily insight.
    /// - `workflow_proposal`: mines the global skill co-occurrence rings and
    ///   writes to the single global `workflows/proposals/` dir.
    ///
    /// `feedback_distill` used to be on this list and is deliberately NOT any
    /// more. Every read and write it performs is already keyed on
    /// `ctx.agent_id` — the correction rows, the `feedback_distill` watermark,
    /// the `feedback/` notes it writes and the index refresh — so "global" was
    /// only ever true by accident: corrections were filed under whatever agent
    /// the tool registry was built with at boot, which was always the base
    /// agent. Once `flag_user_correction` began filing under the *turn's*
    /// agent, keeping the stage base-only meant a correction given to any
    /// non-default agent had no consumer at all: written, surfaced back to the
    /// user as "distilled by the nightly dream cycle", and then never read.
    /// The corpus fan-out is the consumer, so the stage has to be in it.
    ///
    /// `tool_failure_distill` is NOT on this list either, and the reason is
    /// the same one. ⚠️ **The check behind it stopped one composition short
    /// until 2026-08-09**: `RawMemoryToolSink` baked the turn's *persona* into
    /// every `ToolInvocation` row, not the turn's *partition*, so rows for a
    /// scoped run landed in the base corpus and this stage's fourth gate leg
    /// (`has_undistilled_tool_failures`, keyed on `ctx.agent_id`) could never be
    /// true for a `__u-*` / `__p-*` corpus. Both sink construction sites now
    /// compose through `project_scope::session_write_id`, so failures really
    /// are partitioned per corpus. Every read and write the stage performs
    /// — the rows, its watermark, the `lesson/` notes, the index refresh — is
    /// keyed on `ctx.agent_id`. Running it base-only would mean a sub-agent
    /// that keeps failing at the same tool never learns anything, while the
    /// base agent's lessons would be silently derived from a partition it does
    /// not own.
    const GLOBAL_ONLY_STAGES: &'static [&'static str] = &[
        "corpus_narrative",
        "skill_lifecycle",
        "daily_digest",
        "workflow_proposal",
        "goal_lessons_promote",
    ];

    /// Drop the global-only stages, leaving the note-maintenance subset that is
    /// safe to run per project namespace. Built from the same `from_strategy`
    /// list so the project fan-out never drifts from the base pipeline.
    #[must_use]
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

        // Cap the unbounded growth of `recall_signals` (P1 from review).
        // The table grew without bound because `cleanup_old_signals` was
        // defined but never wired into any background task. Dream is the
        // natural cadence: once per cycle, regardless of which stages ran.
        if let Err(e) = ctx.database.cleanup_old_signals(RECALL_SIGNAL_RETENTION_DAYS) {
            tracing::warn!(
                error = %e,
                "recall_signals retention cleanup failed; continuing dream cycle"
            );
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

/// Retention window for `recall_signals` rows. The table grows by one row
/// per recall; without a cap the dedup index and downstream aggregation
/// queries slow over time. 90 days matches the typical "this note was
/// recently useful" horizon used by `note_decay` and similar stages.
const RECALL_SIGNAL_RETENTION_DAYS: u32 = 90;

static LAST_ACTIVITY_TS: Lazy<AtomicI64> = Lazy::new(|| AtomicI64::new(now_timestamp()));

static DREAM_DAEMON: OnceCell<Arc<DreamDaemon>> = OnceCell::new();

pub(crate) fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs() as i64
}

/// Record user activity for `DreamDaemon` idle tracking.
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

/// Ensure `DreamDaemon` is running (once) when memory is enabled.
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

/// Ensure `DreamDaemon` is running (once) when memory is enabled, with optional orientation handle.
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

    // rust-doctor-disable-next-line excessive-clone
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
    #[must_use]
    pub fn new(date: String, content: String, source_memory_count: u32) -> Self {
        Self {
            date,
            content,
            source_memory_count,
            created_at: now_timestamp(),
        }
    }
}

/// `DreamDaemon` status record.
#[derive(Debug, Clone, Default)]
pub struct DreamStatus {
    pub last_run_at: Option<i64>,
    pub last_status: Option<String>,
    pub last_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DreamRunStatus {
    Success,
    Cancelled,
}

impl DreamRunStatus {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Whether a scheduled cycle must be skipped because one already ran today.
///
/// `cancelled` is the one status that earns a retry. It was written by the
/// pre-idle-cut cycles that yielded to fresh user activity before doing any
/// work; the guard still honors those persisted rows so an abandoned cycle
/// gets a second chance once the user goes idle again.
///
/// Every other same-day status — `success`, `timeout`, `error`, or a stale
/// `running` left behind by a crashed process — means "today's cycle is spent,
/// do not start another". Retrying on `timeout`/`error` was the original defect:
/// the guard skipped only on `success`, so a cycle that could never succeed
/// restarted on every tick for the whole window, re-running every LLM stage from
/// scratch (~130 full cycles, and tens of thousands of provider calls, in a
/// single night). A broken cycle must cost one attempt, not a window's worth.
fn should_skip_scheduled_run(status: &DreamStatus, today: &str) -> bool {
    let Some(last_run_at) = status.last_run_at else {
        return false;
    };
    let Some(last_date) = Local.timestamp_opt(last_run_at, 0).single() else {
        return false;
    };
    if last_date.format("%Y-%m-%d").to_string() != today {
        return false;
    }
    status.last_status.as_deref() != Some("cancelled")
}

/// Feedback rules that actually landed on disk during this cycle.
///
/// This is what `dream_reports.feedback_distilled` promises ("total feedback
/// rules distilled") and what `governance_metrics` surfaces as
/// `feedback_distilled_sum` — which the loop-graph audit template reads as a
/// Goodhart counter-metric: *corrections rising while `feedback_distilled_sum`
/// stays flat ⇒ the watchdog loop is failing to turn user corrections into
/// rules*.
///
/// Two things must therefore be excluded, and both used to be counted:
/// * **rejections.** `distill_actions` carries a row for EVERY terminal state,
///   including `FilteredInvalid` / `FilteredNonCandidate` / `FilteredEvidence`
///   (recall-evidence and budget) / `Error`. Counting those inverted the
///   signal outright — the harder the gates correctly rejected, the healthier
///   the column looked.
/// * **skips.** `apply_distill_action` is a pure no-op for `Skip`, so an
///   `Applied` skip records a decision, not a rule on disk.
fn feedback_rules_landed(report: &DreamReport) -> u32 {
    report
        .distill_actions
        .iter()
        .filter(|r| {
            r.stage == "feedback_distill"
                && r.outcome == DistillOutcome::Applied
                && r.action_kind != "skip"
        })
        .count() as u32
}

/// `DreamDaemon` orchestrates idle-time consolidation.
pub struct DreamDaemon {
    database: MemoryBackend,
    config: ConfigDreamingConfig,
    /// Time-decay / archival policy (`memory.memory_decay`), threaded into
    /// `NoteDecayStage` so the previously-dead config drives behaviour.
    decay_policy: MemoryDecayPolicy,
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
    /// Optional wiki orientation — forwarded into `DreamContext` for `IndexRefresherStage`.
    orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
    /// Best-ever memory-health score, tracked across cycles for the evolution
    /// gate (SkillOpt's best-checkpoint). Loaded from `dream_best_health__*` at
    /// construction and persisted on every `AcceptNewBest` so the honest
    /// absolute best survives a restart instead of resetting to 0 (which would
    /// let a worse-than-historical cycle masquerade as a new best).
    best_health: crate::sync_primitives::Mutex<f64>,
    // `project_scoped` was mirrored here until 2026-08-08 to gate the
    // per-namespace fan-out. It gated the wrong axis (see the fan-out site),
    // and once the gate was removed the field had zero readers — withdrawn
    // rather than left as a knob nothing consults. The config option itself
    // still governs what it actually governs, in `project_scope.rs`.
}

impl DreamDaemon {
    pub fn from_config(database: MemoryBackend, config: &MemoryConfig) -> Result<Self, AlephError> {
        let (window_start, window_end) = parse_window(&config.dreaming)?;

        // Restore the persisted best-ever health so the evolution gate does not
        // forget its historical best on every reboot. A read failure or missing
        // value degrades to 0.0 — byte-compatible with the pre-persistence
        // behaviour (gate re-establishes best from the next accepted cycle).
        let best_health = database
            .get_best_health(DEFAULT_AGENT_ID)
            .unwrap_or(None)
            .unwrap_or(0.0);

        Ok(Self {
            database,
            // rust-doctor-disable-next-line excessive-clone
            config: config.dreaming.clone(),
            // rust-doctor-disable-next-line excessive-clone
            decay_policy: config.memory_decay.clone(),
            window_start,
            window_end,
            is_running: AtomicBool::new(false),
            command_handler: None,
            provider: None,
            embedder: None,
            note_memory_dir: None,
            orientation: None,
            best_health: crate::sync_primitives::Mutex::new(best_health),
        })
    }

    /// Test-only view of the in-memory best-health checkpoint, for asserting
    /// that `from_config` reloads the persisted value across a "restart".
    #[cfg(test)]
    pub(crate) fn best_health_for_test(&self) -> f64 {
        *self.best_health.lock().unwrap_or_else(|e| e.into_inner())
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

    /// Attach a wiki orientation handle for the `IndexRefresher` dream stage.
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
    /// path so observers (Panel, journalctl, `dream_status` table) see the cycle.
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

        if let Err(e) = self
            .database
            .set_dream_status(DreamStatus {
                last_run_at: Some(run_start),
                last_status: Some("running".to_string()),
                last_duration_ms: None,
            })
            .await
        {
            tracing::warn!(error = %e, "failed to persist dream status (start)");
        }

        // Bound the forced run by `max_duration_seconds`, exactly like the
        // scheduled path (`check_and_run`). Without this a hung LLM stage would
        // run forever AND hold the `is_running` latch via `RunGuard`, blocking
        // every subsequent scheduled cycle.
        let result = match tokio::time::timeout(
            Duration::from_secs(u64::from(self.config.max_duration_seconds)),
            self.run_dream(run_start, run_date, true),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(AlephError::other(format!(
                "dream cycle exceeded max_duration_seconds ({})",
                self.config.max_duration_seconds
            ))),
        };
        let duration_ms = ((now_timestamp() - run_start).max(0) as u64) * 1000;

        match &result {
            Ok(outcome) => {
                if let Err(e) = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some(outcome.status.as_str().to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await
                {
                    tracing::warn!(error = %e, "failed to persist dream status (ok)");
                }
                // A forced cycle is a real cycle: it must land in the
                // `dream_reports` audit table like a scheduled one. This writer
                // used to live only on the scheduled path, so every `run_now`
                // was invisible both to the Panel's run history and to the
                // governance audit's "did dreaming do anything" probe — a
                // forced consolidation night read as "the daemon did nothing".
                self.persist_run_row(outcome, DEFAULT_AGENT_ID, run_start, duration_ms);
            }
            Err(_) => {
                if let Err(e) = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some("failed".to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await
                {
                    tracing::warn!(error = %e, "failed to persist dream status (failed)");
                }
            }
        }

        result.map(|outcome| outcome.report)
    }

    /// Append one completed cycle to the `dream_reports` audit table.
    ///
    /// Single writer for the scheduled (`check_and_run`) path, the forced
    /// (`run_now`) path, and every project sub-cycle — the counters, the
    /// SkillOpt gate verdict (`evolution_json`) and the cycle decision
    /// (`decision_json`) must not depend on *which* corpus ran or on how the
    /// cycle was triggered. Best-effort: a failed insert (e.g. a PK clash on a
    /// same-second re-run) is logged, never fatal.
    ///
    /// `namespace` is the corpus's real storage partition key — the base agent
    /// id, or `{base}__proj-*`. It is also part of the row id, because the base
    /// cycle and its sub-cycles routinely finish within the same second and the
    /// id is the primary key.
    fn persist_run_row(
        &self,
        outcome: &DreamCycleOutcome,
        namespace: &str,
        run_start: i64,
        duration_ms: u64,
    ) {
        let report = &outcome.report;
        let persisted = crate::memory::store::sqlite::dream_reports::PersistedDreamReport {
            id: format!("dream_{run_start}_{namespace}"),
            // rust-doctor-disable-next-line excessive-clone
            pipeline_type: report.pipeline_type.clone(),
            started_at: run_start,
            finished_at: report.finished_at.max(run_start),
            duration_ms: duration_ms as i64,
            synthesis_count: report.synthesis_count,
            notes_consolidated: report.notes_consolidated,
            notes_woven: report.notes_woven,
            notes_archived: report.notes_archived,
            // Rules that LANDED — not every feedback-distill row. See
            // `feedback_rules_landed`: this column is the Dreaming ×
            // correction Goodhart counter-metric, so counting rejected
            // and skipped actions inverts the very signal it exists for.
            feedback_distilled: feedback_rules_landed(report),
            // rust-doctor-disable-next-line excessive-clone
            errors: report.errors.clone(),
            namespace: namespace.to_string(),
            // Serialize the SkillOpt gate verdict so the accept/reject
            // decision is queryable, not just buried in the event log.
            // A serialization failure degrades to NULL (non-fatal).
            evolution_json: report
                .evolution
                .as_ref()
                .and_then(|e| serde_json::to_string(e).ok()),
            decision_json: serde_json::to_string(&outcome.decision).ok(),
        };
        if let Err(e) = self.database.insert_dream_report(&persisted) {
            warn!(error = %e, "failed to persist dream report row");
        }
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
            if should_skip_scheduled_run(&status, &run_date) {
                info!(
                    reason = "already_ran_today",
                    last_run_at = status.last_run_at,
                    last_status = status.last_status.as_deref().unwrap_or("unknown"),
                    "DreamDaemon tick: skipped"
                );
                return Ok(());
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

        // rust-doctor-disable-next-line excessive-clone
        let run_future = self.run_dream(run_start, run_date.clone(), false);
        let run_result = tokio::time::timeout(
            Duration::from_secs(u64::from(self.config.max_duration_seconds)),
            run_future,
        )
        .await;

        let duration_ms = (now_timestamp() - run_start).max(0) as u64 * 1000;

        match run_result {
            Ok(Ok(outcome)) => {
                info!(
                    notes_consolidated = outcome.report.notes_consolidated,
                    synthesis_count = outcome.report.synthesis_count,
                    notes_archived = outcome.report.notes_archived,
                    "DreamDaemon completed"
                );

                if let Err(e) = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some(outcome.status.as_str().to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await
                {
                    tracing::warn!(error = %e, "failed to persist dream status (completed)");
                }

                self.persist_run_row(&outcome, DEFAULT_AGENT_ID, run_start, duration_ms);
            }
            Ok(Err(err)) => {
                warn!(error = %err, "DreamDaemon run failed");
                if let Err(e) = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some("error".to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await
                {
                    tracing::warn!(error = %e, "failed to persist dream status (error)");
                }
            }
            Err(_) => {
                warn!("DreamDaemon run timed out");
                if let Err(e) = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some("timeout".to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await
                {
                    tracing::warn!(error = %e, "failed to persist dream status (timeout)");
                }
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

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn run_dream(
        &self,
        run_start: i64,
        _run_date: String,
        force: bool,
    ) -> Result<DreamCycleOutcome, AlephError> {
        // --- Phase 0: Resolve note memory dir + load the note index ---
        // rust-doctor-disable-next-line excessive-clone
        let memory_dir = self.note_memory_dir.clone().unwrap_or_else(|| {
            crate::utils::paths::get_note_memory_dir()
                .unwrap_or_else(|_| PathBuf::from(".aleph/data/memory"))
        });
        let note_index = self
            .database
            .list_notes(DEFAULT_AGENT_ID)
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "failed to list notes for base agent, proceeding with empty index");
                Vec::new()
            });

        // --- Phase 1: Rehydrate cross-cycle state, then collect signals ---
        // One read of the persisted event log serves three consumers:
        //
        //  * `prior_report` — the previous cycle's LLM-produced rot counts
        //    (contradictions / stale marks / merged duplicates), which the
        //    signal collector normalises into the rot rates;
        //  * `MutationGate` — the churn-detection windows and the conserve
        //    cooldown;
        //  * `StrategySelector` — the validation pass-rate personality window.
        //
        // The latter two used to be in-process accumulators, which meant they
        // could only ever see cycles that happened since the last restart.
        // Cycles run at most once per day, so a five-cycle detector needed five
        // consecutive days of uptime to arm and the cooldown evaporated on
        // reboot — while this exact history sat unread on disk. Deriving them
        // here makes the daemon's decision state reconstructible from a single
        // persistent source (constitution A3), the same shape evolver uses
        // (`analyzeRecentHistory` is a pure fold over its event log).
        //
        // Read failure (fresh install, corrupt log) degrades to an empty
        // history: zero rot rates, disarmed detectors, neutral personality —
        // never an aborted cycle.
        let history = EventLog::new(memory_dir.join(DEFAULT_AGENT_ID))
            .read_last(DREAM_HISTORY_WINDOW)
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "failed to read dream event log; cross-cycle state starts empty");
                Vec::new()
            });
        let prior_report = history.last().map(|ev| ev.report.clone());
        let raw_metrics = compute_raw_metrics(
            &note_index,
            self.database.as_ref(),
            DEFAULT_AGENT_ID,
            prior_report.as_ref(),
        )
        .await;
        let signal_snapshot = SignalSnapshot::from_metrics(&raw_metrics);

        // --- Phase 2: Mutation gate evaluation ---
        let gate_decision =
            MutationGate::from_reports(history.iter().map(|ev| &ev.report)).evaluate();

        // --- Phase 3: Strategy selection ---
        let selector =
            StrategySelector::from_outcomes(history.iter().map(|ev| ev.validation.overall_ok()));
        let selection = selector.select(&signal_snapshot, &gate_decision);

        let strategy = selection.strategy;
        info!(strategy = %strategy, rationale = %selection.rationale, "Dream strategy selected");

        // --- Phase 4: Build and run the consolidation pipeline ---
        let pipeline = DreamPipeline::from_strategy(strategy, &self.config, &self.decay_policy);
        // rust-doctor-disable-next-line excessive-clone
        let (mut report, run_status) = match (self.provider.clone(), self.embedder.clone()) {
            (Some(provider), Some(embedder)) => {
                // rust-doctor-disable-next-line excessive-clone
                let mut indexer = NoteIndexer::new(memory_dir.clone(), self.database.clone());
                // Embed-on-write: distilled / rewritten / renamed notes get a
                // fresh vector immediately instead of waiting for reembed_all.
                // rust-doctor-disable-next-line excessive-clone
                indexer = indexer.with_embedder(embedder.clone());
                let notes: Vec<NoteEntry> =
                    note_index.iter().map(NoteEntry::from_index_entry).collect();
                // Scheduled cycles yield to fresh user activity; forced cycles
                // (run_now / E2E harness) run to completion.
                let activity_checker: Arc<dyn Fn() -> bool + Send + Sync> = if force {
                    Arc::new(|| false)
                } else {
                    let threshold = i64::from(self.config.idle_threshold_seconds);
                    Arc::new(move || idle_seconds() < threshold)
                };
                let ctx = DreamContext {
                    notes,
                    note_contents: HashMap::new(),
                    agent_id: DEFAULT_AGENT_ID.to_string(),
                    // rust-doctor-disable-next-line excessive-clone
                    database: self.database.clone(),
                    indexer,
                    // rust-doctor-disable-next-line excessive-clone
                    provider: provider.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    embedder: embedder.clone(),
                    report: DreamReport {
                        pipeline_type: strategy.to_string(),
                        started_at: run_start,
                        ..Default::default()
                    },
                    pipeline_type: strategy.to_string(),
                    activity_checker: activity_checker.clone(),
                    strategy,
                    // rust-doctor-disable-next-line excessive-clone
                    orientation: self.orientation.clone(),
                    evolution_budget: EditBudget::default(),
                };
                let mut report = pipeline.run(ctx).await?;

                // Per-corpus maintenance. The base agent ran the full pipeline
                // above; **every other corpus on disk** gets the
                // note-maintenance subset so its notes are
                // linted/consolidated/synthesised too. The global-only stages
                // (feedback floor, skill lifecycle, daily digest) are
                // excluded — those stay cross-corpus.
                //
                // The enumeration is `list_note_corpora`, the single source for
                // "which corpora exist", minus the base agent. It used to be
                // `list_scoped_agent_ids(.., DEFAULT_AGENT_ID)` behind an
                // `if self.project_scoped` gate, and both halves were wrong in
                // the same direction: `__u-*` / `__p-*` partitions are produced
                // by *session scope*, which never consults `project_scoped`, and
                // a non-default agent is not a sibling of `main` at all. With
                // the flag at its default (false) that meant every personal
                // partition, every room partition and every non-default agent's
                // notes went unmaintained from the day they were created — no
                // error, no empty result, just a branch that was never taken.
                //
                // Each corpus governs *itself*: it folds its own event log into
                // its own gate, personality and best-health checkpoint, and
                // appends its own event — it does not inherit this cycle's
                // strategy and its churn never joins the base agent's history.
                // See `project_cycle` for why joining them would be worse than
                // the drop it replaces. Per-corpus failures are logged, never
                // aborting the base cycle.
                let corpora = maintenance_corpora(&memory_dir, DEFAULT_AGENT_ID);
                let deps = project_cycle::ProjectCycleDeps {
                    memory_dir: &memory_dir,
                    database: &self.database,
                    provider: &provider,
                    embedder: &embedder,
                    config: &self.config,
                    decay_policy: &self.decay_policy,
                    orientation: &self.orientation,
                    activity_checker: &activity_checker,
                };
                // Nightly budget. The fan-out's cost scales with the number of
                // corpora, which scales with users, rooms and agents — an axis
                // nothing else in this file bounds. Corpora that skip (unchanged
                // since their last cycle) cost no LLM call and so do not spend
                // budget; only cycles that actually ran do.
                //
                // Read from config, not from `MAX_CORPUS_CYCLES_PER_NIGHT`: the
                // right ceiling depends on how many partitions an install has
                // and how much the operator is willing to spend, and neither is
                // knowable here. The constant remains that knob's default.
                let budget = self.config.max_corpus_cycles_per_night;
                let mut spent = 0usize;
                for ns in &corpora {
                    if spent >= budget {
                        warn!(
                            budget,
                            remaining = corpora.len() - spent,
                            "nightly per-corpus dream budget exhausted; \
                             remaining corpora wait for the next window"
                        );
                        break;
                    }
                    match project_cycle::run_namespace_cycle(&deps, ns).await {
                        Ok(project_cycle::NamespaceCycle::Idle) => {}
                        Ok(project_cycle::NamespaceCycle::Ran(outcome)) => {
                            spent += 1;
                            let r = &outcome.report;
                            let interrupted = r.status == DreamReportStatus::Interrupted;
                            info!(
                                agent = %ns,
                                stages = ?r.stages_executed,
                                interrupted,
                                "corpus dream complete"
                            );
                            // The audit row the operator reads. Its absence is
                            // why a non-base corpus's nightly history was
                            // legible to the model — which reads that corpus's
                            // own event log — and to no one else. Same writer as
                            // the base cycle, so the two can't drift; the
                            // sub-cycle's own clock, because it is its own run.
                            if !r.is_vacuous_interruption() {
                                self.persist_run_row(&outcome, ns, r.started_at, r.duration_ms);
                            }
                            // The activity checker fired: the user is back.
                            // Walking the remaining corpora would only produce a
                            // burst of cycles that interrupt at their first
                            // stage — stop, because stopping is what yielding
                            // means.
                            if interrupted {
                                break;
                            }
                        }
                        Err(e) => warn!(
                            agent = %ns,
                            error = %e,
                            "corpus dream failed"
                        ),
                    }
                }

                report.finished_at = now_timestamp();
                report.duration_ms = ((report.finished_at - run_start).max(0) as u64) * 1000;
                (report, DreamRunStatus::Success)
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

        // --- Phase 5.5: Evolution gate (memory-health before/after) ---
        // SkillOpt discipline at cycle granularity: score the corpus before and
        // after this cycle's edits, accept-track the best, and conserve (rather
        // than compound) when a cycle degrades health.
        let baseline_health = memory_health_score(&signal_snapshot);
        let post_index = self
            .database
            .list_notes(DEFAULT_AGENT_ID)
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "failed to list notes for post-cycle health check, proceeding with empty index");
                Vec::new()
            });

        // --- Phase 5: Validation (L2 consistency, deterministic) ---
        // L2 (duplicate content-hash) runs cheaply from the index — no file
        // reads. L1 (format) needs full note markdown; re-reading the whole
        // corpus every cycle is too costly, so it stays a vacuous pass and
        // `overall_ok()` gates on L2 alone. Wiring a real L2 revives the strategy
        // selector's personality loop (validation pass-rate over the window):
        // duplicate-hash rot now tightens the synthesize threshold instead of
        // every cycle rubber-stamping `passed` with zero checks run.
        let l2_pairs: Vec<(String, String)> = post_index
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .map(|n| (n.path.clone(), n.content_hash.clone()))
            .collect();
        let validation_report = DreamValidationReport {
            l1_format: l1_over_corpus(&memory_dir, DEFAULT_AGENT_ID, &post_index).await,
            l2_consistency: validation::run_l2_validation(&l2_pairs),
            l3_semantic: None,
            l4_retrospective: None,
        };

        // Same prior report as the baseline: the rot term is a lagging signal, so
        // both sides of the evolution gate carry the identical penalty and it
        // cancels in the health *delta* — the gate still judges on this cycle's
        // recall change, while `best_health` now tracks an honest absolute level.
        let post_metrics = compute_raw_metrics(
            &post_index,
            self.database.as_ref(),
            DEFAULT_AGENT_ID,
            prior_report.as_ref(),
        )
        .await;
        // MutationGate's wasted-distillation detector compares the same mature
        // cohort on both sides: of the skill notes old enough to have had a
        // recall chance, how many actually got recalled. Feeding it this-cycle's
        // fresh produce is why it misfired on cold start; feeding the recall side
        // the whole-corpus stock is why it went toothless once anything was ever
        // recalled. Both now come from the mature cohort.
        report.distill_produced = post_metrics.mature_skill_total;
        report.distill_recalled = post_metrics.mature_skill_recalled;
        let candidate_health = memory_health_score(&SignalSnapshot::from_metrics(&post_metrics));
        let best_before = *self.best_health.lock().unwrap_or_else(|e| e.into_inner());
        let gate_outcome = evaluate_gate(
            candidate_health,
            baseline_health,
            best_before,
            evolution::HEALTH_GATE_EPSILON,
        );
        let new_best = if gate_outcome == GateOutcome::AcceptNewBest {
            candidate_health
        } else {
            best_before
        };
        *self.best_health.lock().unwrap_or_else(|e| e.into_inner()) = new_best;
        // Persist the checkpoint only when it actually advanced, so the honest
        // best survives a restart. A write failure is non-fatal (the in-memory
        // value still governs this process's remaining cycles).
        if gate_outcome == GateOutcome::AcceptNewBest {
            if let Err(e) = self.database.set_best_health(DEFAULT_AGENT_ID, new_best) {
                warn!(error = %e, "failed to persist best_health checkpoint");
            }
        }
        report.evolution = Some(EvolutionOutcome {
            baseline: baseline_health,
            candidate: candidate_health,
            best: new_best,
            outcome: gate_outcome,
            merges_rejected: report.merges_rejected,
        });
        // The cooldown itself is not armed here: it is *derived* next cycle from
        // this outcome (`EvolutionOutcome::degraded` — the same predicate), so a
        // restart between the two cycles cannot swallow it. The warning stays
        // because a degrading night is something an operator should see now.
        if report
            .evolution
            .as_ref()
            .is_some_and(EvolutionOutcome::degraded)
        {
            warn!(
                baseline = baseline_health,
                candidate = candidate_health,
                cooldown_cycles = mutation_gate::DEGRADED_HEALTH_COOLDOWN_CYCLES,
                "Dream cycle degraded memory health — next cycles will conserve"
            );
        }

        // --- Phase 6: Solidify (event log) ---
        //
        // This append is what makes the *next* cycle's gate and personality
        // possible: it is the single persistent source they fold over. A write
        // failure is therefore not merely a lost audit line — the next cycle
        // will not see this one at all — so it is logged at ERROR.
        let agent_dir = memory_dir.join(DEFAULT_AGENT_ID);
        let event_log = EventLog::new(&agent_dir);
        let cycle = event_log.next_cycle().await.unwrap_or(1);

        let validation_passed = validation_report.overall_ok();
        let decision = CycleDecision {
            strategy,
            // rust-doctor-disable-next-line excessive-clone
            rationale: selection.rationale.clone(),
            personality_adjustment: selection.personality_adjustment,
            // rust-doctor-disable-next-line excessive-clone
            gate: gate_decision.clone(),
            // rust-doctor-disable-next-line excessive-clone
            stages: report.stages_executed.clone(),
            validation_passed,
        };

        let event = DreamEvent {
            id: format!("dream_{run_start}_{cycle}"),
            cycle,
            strategy,
            selection,
            gate_decision,
            // rust-doctor-disable-next-line excessive-clone
            report: report.clone(),
            validation: validation_report,
            duration_ms: ((now_timestamp() - run_start).max(0) as u64) * 1000,
            created_at: now_timestamp(),
        };

        if let Err(e) = event_log.append(&event).await {
            tracing::error!(
                error = %e,
                "Failed to write dream event log — the next cycle's churn gate \
                 and personality window will not see this cycle"
            );
        }

        Ok(DreamCycleOutcome {
            status: run_status,
            report,
            decision,
        })
    }
}

/// Run L1 format validation over a bounded, newest-first sample of one agent's
/// corpus: read the notes off disk and check frontmatter / category / non-empty
/// body.
///
/// Bounded by `L1_MAX_NOTES` so a large corpus can't turn the nightly cycle into
/// thousands of reads; newest-first because a malformed note is almost always
/// one this cycle just wrote. A failed L1 only nudges the selector's personality
/// pass-rate down (non-destructive), matching L2's contract.
///
/// Shared by the base cycle and every project sub-cycle. Two copies would drift
/// silently in the direction that hurts: a namespace whose L1 quietly degraded
/// to a vacuous pass would rubber-stamp `passed` forever and freeze its
/// personality — the exact failure this validation was wired to end.
async fn l1_over_corpus(
    memory_dir: &Path,
    agent_id: &str,
    index: &[NoteIndexEntry],
) -> ValidationTier {
    /// Cap on notes read off disk per cycle, per agent.
    const L1_MAX_NOTES: usize = 200;

    let mut by_recency: Vec<&NoteIndexEntry> = index.iter().collect();
    by_recency.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
    let mut contents: HashMap<String, String> = HashMap::new();
    for entry in by_recency.into_iter().take(L1_MAX_NOTES) {
        let file_path = memory_dir
            .join(&entry.agent_id)
            .join(&entry.category)
            .join(format!("{}.md", entry.filename));
        if let Ok(content) = tokio::fs::read_to_string(&file_path).await {
            // rust-doctor-disable-next-line excessive-clone
            contents.insert(entry.path.clone(), content);
        }
    }

    let tier = validation::run_l1_validation(&contents);
    if !tier.passed {
        warn!(
            agent = %agent_id,
            checks_run = tier.checks_run,
            issues = tier.issues.len(),
            "Dream L1 validation found malformed notes"
        );
    }
    tier
}

/// Skill notes younger than this are excluded from `MutationGate`'s
/// wasted-distillation cohort — they haven't had a fair chance to be recalled.
/// Mirrors `NoteDecay`'s 7-day "too new to touch" protection window.
const MATURE_SKILL_DAYS: i64 = 7;

/// Compute a `RawMetrics` snapshot for the Dream cycle.
///
/// Pulls notes (count + 24h growth) from the in-memory note index and folds in
/// per-note recall counts from `recall_signals` so the recall-derived signals
/// (`note_hit_rate` / `never_recalled_ratio` / `skill_recall_rate`) carry real
/// values rather than the historical zeros.
///
/// The recall wiring is load-bearing: `skill_recall_rate` feeds the strategy
/// selector's `growth_pressure` (a false 0 perpetually inflated the synthesize
/// pressure), and the `mature_skill_total` / `mature_skill_recalled` pair feeds
/// `MutationGate`'s wasted-distillation detector — restricted to skill notes
/// old enough to have had a recall opportunity so a fresh cycle's produce can't
/// make it misfire. The recall query degrades to a warning + zeros on backend
/// failure — strategy selection runs on the surviving signals rather than
/// aborting.
async fn compute_raw_metrics(
    notes: &[NoteIndexEntry],
    store: &SqliteMemoryBackend,
    agent_id: &str,
    prior: Option<&DreamReport>,
) -> RawMetrics {
    let day_ago = now_timestamp() - 86_400;
    let notes_added_24h = notes.iter().filter(|n| n.created_at >= day_ago).count() as u32;
    // Skill notes created before this cutoff are "mature": old enough to have
    // had a recall opportunity, so their recall rate is a fair signal for
    // MutationGate's wasted-distillation detector.
    let mature_cutoff = now_timestamp() - MATURE_SKILL_DAYS * 86_400;

    // Fold in recall signals with a single batch query over every note path.
    // `recall_hit_counts` returns only the paths that have at least one recorded
    // recall, so its key set is exactly the recalled subset.
    let total_notes = notes.len() as u32;
    // rust-doctor-disable-next-line excessive-clone
    let all_paths: Vec<String> = notes.iter().map(|n| n.path.clone()).collect();
    // The mature-skill cohort (denominator/numerator of MutationGate's
    // wasted-distillation ratio) is derived from the same recall `hits`, so it
    // is accumulated inside the Ok arm; the Err path leaves both at zero.
    let mut mature_skill_total = 0u32;
    let mut mature_skill_recalled = 0u32;
    let (note_hit_rate, never_recalled_count, skill_notes_total, skill_notes_recalled) =
        match store.recall_hit_counts(agent_id, &all_paths).await {
            Ok(hits) => {
                let recalled_total = hits.len() as u32;
                let never = total_notes.saturating_sub(recalled_total);
                let skill_total = notes.iter().filter(|n| n.category == "skill").count() as u32;
                let skill_recalled = notes
                    .iter()
                    .filter(|n| n.category == "skill" && hits.contains_key(&n.path))
                    .count() as u32;
                mature_skill_total = notes
                    .iter()
                    .filter(|n| n.category == "skill" && n.created_at < mature_cutoff)
                    .count() as u32;
                mature_skill_recalled = notes
                    .iter()
                    .filter(|n| {
                        n.category == "skill"
                            && n.created_at < mature_cutoff
                            && hits.contains_key(&n.path)
                    })
                    .count() as u32;
                let hit_rate = if total_notes > 0 {
                    f64::from(recalled_total) / f64::from(total_notes)
                } else {
                    0.0
                };
                (hit_rate, never, skill_total, skill_recalled)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    agent = agent_id,
                    "recall-signal aggregation failed; recall signals will read zero",
                );
                (0.0, 0, 0, 0)
            }
        };

    // Rot signals (contradiction / duplication / staleness) are *semantic*
    // judgements the pipeline's LLM stages already made last cycle — `note_drift`
    // counts contradictions & stale marks, consolidation counts merged duplicates
    // — and recorded in the persisted `DreamReport`. We read those counts back
    // rather than re-deriving them with rules (R7: no deterministic re-judgement).
    // This is a *lagging* signal, exactly as the `source: "dream_report"` label on
    // these signals always intended; the first-ever cycle has no prior and reads
    // zero (byte-compatible with the previous hardcoded-zero behaviour), then
    // self-corrects after one cycle. Normalised against corpus size so a handful
    // of contradictions in a large corpus is a small, honest penalty — not alarmist.
    let (duplication_rate, contradiction_rate, staleness_rate) = match prior {
        Some(r) if total_notes > 0 => {
            let n = f64::from(total_notes);
            (
                (f64::from(r.notes_consolidated) / n).clamp(0.0, 1.0),
                (f64::from(r.contradictions_found) / n).clamp(0.0, 1.0),
                (f64::from(r.notes_marked_stale) / n).clamp(0.0, 1.0),
            )
        }
        _ => (0.0, 0.0, 0.0),
    };

    RawMetrics {
        total_notes,
        notes_added_24h,
        note_hit_rate,
        never_recalled_count,
        skill_notes_total,
        skill_notes_recalled,
        mature_skill_total,
        mature_skill_recalled,
        duplication_rate,
        contradiction_rate,
        staleness_rate,
    }
}

fn parse_window(config: &ConfigDreamingConfig) -> Result<(NaiveTime, NaiveTime), AlephError> {
    let start = NaiveTime::parse_from_str(&config.window_start_local, "%H:%M").map_err(|e| {
        AlephError::config(format!(
            "Invalid dreaming.window_start_local '{}', expected HH:MM: {e}",
            config.window_start_local
        ))
    })?;
    let end = NaiveTime::parse_from_str(&config.window_end_local, "%H:%M").map_err(|e| {
        AlephError::config(format!(
            "Invalid dreaming.window_end_local '{}', expected HH:MM: {e}",
            config.window_end_local
        ))
    })?;
    Ok((start, end))
}

/// Scratch directories for this subsystem's tests — see
/// [`crate::utils::scratch::scratch_root`] for why the path is a child of the
/// guard and why the guard must be bound by the caller.
#[cfg(test)]
pub(crate) use crate::utils::scratch::scratch_root;

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // should_skip_scheduled_run — the once-per-day guard
    // -----------------------------------------------------------------------

    /// Build a status whose `last_run_at` is `hours_ago` before now.
    fn status_from(hours_ago: i64, last_status: Option<&str>) -> DreamStatus {
        DreamStatus {
            last_run_at: Some(Local::now().timestamp() - hours_ago * 3600),
            last_status: last_status.map(str::to_string),
            last_duration_ms: None,
        }
    }

    fn today() -> String {
        Local::now().format("%Y-%m-%d").to_string()
    }

    #[test]
    fn skips_when_todays_run_succeeded() {
        assert!(should_skip_scheduled_run(
            &status_from(0, Some("success")),
            &today()
        ));
    }

    /// Regression: a timed-out run used to leave `last_status = "timeout"`,
    /// which the old guard did not treat as "ran today" — so the daemon
    /// restarted a full cycle on every 60s tick for the rest of the window.
    #[test]
    fn skips_when_todays_run_timed_out() {
        assert!(
            should_skip_scheduled_run(&status_from(0, Some("timeout")), &today()),
            "a timed-out cycle must cost one attempt, not restart every tick"
        );
    }

    /// Regression: same defect via the error path — a 403/quota failure left
    /// `last_status = "error"` and the daemon kept relaunching cycles.
    #[test]
    fn skips_when_todays_run_errored() {
        assert!(
            should_skip_scheduled_run(&status_from(0, Some("error")), &today()),
            "an errored cycle must not restart every tick"
        );
    }

    /// A stale `running` row is left behind when the process dies mid-cycle.
    /// Treat it as spent: erring toward "no dream today" is safe; erring toward
    /// "retry" risks a crash-loop burning provider quota.
    #[test]
    fn skips_when_todays_run_left_stale_running_marker() {
        assert!(should_skip_scheduled_run(
            &status_from(0, Some("running")),
            &today()
        ));
    }

    /// `cancelled` is the one status that earns a retry — a legacy row from a
    /// pre-idle-cut cycle that yielded to user activity before doing any work.
    #[test]
    fn retries_when_todays_run_was_cancelled_by_user_activity() {
        assert!(!should_skip_scheduled_run(
            &status_from(0, Some("cancelled")),
            &today()
        ));
    }

    #[test]
    fn runs_when_last_run_was_a_previous_day() {
        assert!(!should_skip_scheduled_run(
            &status_from(48, Some("success")),
            &today()
        ));
    }

    #[test]
    fn runs_when_there_is_no_prior_run() {
        assert!(!should_skip_scheduled_run(
            &DreamStatus::default(),
            &today()
        ));
    }

    // -----------------------------------------------------------------------
    // feedback_rules_landed — the Goodhart counter-metric must not invert
    // -----------------------------------------------------------------------

    fn rule(title: &str) -> DistillAction {
        DistillAction::New {
            title: title.into(),
            rule: "r".into(),
            confidence: 0.9,
            severity: crate::memory::notes::Severity::High,
            source_facts: vec![],
        }
    }

    fn report_with(records: Vec<DistillActionRecord>) -> DreamReport {
        DreamReport {
            distill_actions: records,
            ..Default::default()
        }
    }

    /// A cycle where every action was turned away — gate-invalid, hallucinated
    /// target, recall-evidence reject, apply error — distilled ZERO rules.
    /// Counting those rows made the column read healthiest exactly when the
    /// gates were rejecting hardest, inverting the audit signal that pairs it
    /// against the user's correction count.
    #[test]
    fn rejected_and_errored_actions_are_not_distilled_rules() {
        let report = report_with(vec![
            DistillActionRecord::from_action(
                "feedback_distill",
                &rule("a"),
                DistillOutcome::FilteredInvalid,
                Some("title is empty".into()),
            ),
            DistillActionRecord::from_action(
                "feedback_distill",
                &DistillAction::Strengthen {
                    existing_note_path: "feedback/ghost".into(),
                    source_facts: vec![],
                },
                DistillOutcome::FilteredNonCandidate,
                None,
            ),
            DistillActionRecord::from_action(
                "feedback_distill",
                &rule("b"),
                DistillOutcome::FilteredEvidence,
                Some("recall support wins".into()),
            ),
            DistillActionRecord::from_action(
                "feedback_distill",
                &rule("c"),
                DistillOutcome::Error,
                Some("disk full".into()),
            ),
        ]);
        assert_eq!(feedback_rules_landed(&report), 0);
    }

    /// `Skip` is a no-op in `apply_distill_action`, so an `Applied` skip is a
    /// decision the LLM made — not a rule on disk. Only real writes count, and
    /// `skill_distill` rows share the same vec and must not leak in.
    #[test]
    fn only_applied_non_skip_feedback_actions_are_counted() {
        let report = report_with(vec![
            DistillActionRecord::from_action(
                "feedback_distill",
                &rule("landed"),
                DistillOutcome::Applied,
                None,
            ),
            DistillActionRecord::from_action(
                "feedback_distill",
                &DistillAction::Skip {
                    source_fact: "F1".into(),
                    reason: "transient".into(),
                },
                DistillOutcome::Applied,
                None,
            ),
            DistillActionRecord::from_action(
                "skill_distill",
                &rule("other-stage"),
                DistillOutcome::Applied,
                None,
            ),
        ]);
        assert_eq!(feedback_rules_landed(&report), 1);
    }

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
        let decay = MemoryDecayPolicy::default();
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Consolidate, &cfg, &decay);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "feedback_distill",
                "tool_failure_distill",
                "note_drift",
                "index_refresher",
                "co_recall_edges",
                "graph_recompute",
                "note_weave",
                "mention_weave",
                "note_decay",
                "skill_lifecycle",
                "goal_lessons_promote",
            ]
        );
    }

    #[test]
    fn pipeline_from_strategy_synthesize() {
        let cfg = crate::config::types::memory::DreamingConfig::default();
        let decay = MemoryDecayPolicy::default();
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Synthesize, &cfg, &decay);
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
                "tool_failure_distill",
                "workflow_proposal",
                "corpus_narrative",
                "daily_digest"
            ]
        );
    }

    #[test]
    fn retain_project_stages_drops_global_only_stages() {
        let cfg = crate::config::types::memory::DreamingConfig::default();
        // Synthesize is the richest pipeline — it contains all three
        // global-only stages plus the note-maintenance subset.
        let decay = MemoryDecayPolicy::default();
        let project = DreamPipeline::from_strategy(DreamStrategy::Synthesize, &cfg, &decay)
            .retain_project_stages();
        let names: Vec<&str> = project.stages.iter().map(|s| s.name()).collect();
        // The global-only stages must be gone...
        for global in DreamPipeline::GLOBAL_ONLY_STAGES {
            assert!(
                !names.contains(global),
                "{global} must not run per project namespace"
            );
        }
        // ...and the per-corpus subset must remain, in order. `feedback_distill`
        // belongs here: corrections are filed per agent by
        // `flag_user_correction`, and this fan-out is their only consumer.
        // `tool_failure_distill` belongs here for the same reason: tool
        // invocations are recorded under the turn's agent, so a sub-agent's
        // failures only ever reach a distiller through this fan-out.
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "note_synthesis",
                "skill_distill",
                "feedback_distill",
                "tool_failure_distill",
            ]
        );
    }

    #[test]
    fn pipeline_synthesize_runs_feedback_after_skill_distill() {
        // Phase 3: ensure FeedbackDistill is scheduled directly after
        // SkillDistill so a single dream cycle can pick up both.
        let cfg = crate::config::types::memory::DreamingConfig::default();
        let decay = MemoryDecayPolicy::default();
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Synthesize, &cfg, &decay);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        let skill_pos = names.iter().position(|n| *n == "skill_distill").unwrap();
        let feedback_pos = names.iter().position(|n| *n == "feedback_distill").unwrap();
        assert_eq!(feedback_pos, skill_pos + 1);
    }

    #[test]
    fn pipeline_from_strategy_conserve() {
        let cfg = crate::config::types::memory::DreamingConfig::default();
        let decay = MemoryDecayPolicy::default();
        let pipeline = DreamPipeline::from_strategy(DreamStrategy::Conserve, &cfg, &decay);
        let names: Vec<&str> = pipeline.stages.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_review",
                "index_refresher",
                "co_recall_edges",
                "graph_recompute"
            ]
        );
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
    async fn from_config_reloads_persisted_best_health() {
        // The evolution gate's best-ever checkpoint must survive a restart:
        // a fresh daemon built from the same store reloads the persisted value
        // instead of resetting to 0 (which would let a worse-than-historical
        // cycle masquerade as a new best).
        let (_temp_guard, temp) = scratch_root();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());

        // First run: no persisted value → 0.0.
        let cold = DreamDaemon::from_config(store.clone(), &MemoryConfig::default())
            .expect("valid config");
        assert_eq!(cold.best_health_for_test(), 0.0);

        // Persist a best, then a "restart" (fresh from_config) reloads it.
        store.set_best_health(DEFAULT_AGENT_ID, 0.62).unwrap();
        let warm = DreamDaemon::from_config(store, &MemoryConfig::default()).expect("valid config");
        assert!((warm.best_health_for_test() - 0.62).abs() < 1e-9);
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
        let (_temp_guard, temp) = scratch_root();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let m = compute_raw_metrics(&notes, store.as_ref(), DEFAULT_AGENT_ID, None).await;
        assert_eq!(m.total_notes, 3);
        assert_eq!(m.notes_added_24h, 2);
    }

    #[tokio::test]
    async fn compute_raw_metrics_folds_in_recall_signals() {
        let (_temp_guard, temp) = scratch_root();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());

        let now = now_timestamp();
        let note = |path: &str, category: &str| NoteIndexEntry {
            path: path.into(),
            filename: path.rsplit('/').next().unwrap_or(path).into(),
            agent_id: DEFAULT_AGENT_ID.into(),
            category: category.into(),
            tags: vec![],
            link_count: 0,
            created_at: now,
            updated_at: now,
            content_hash: "h".into(),
        };
        // Two skill notes (one recalled, one cold) + one reference note (cold).
        let notes = vec![
            note("skill/async-errors", "skill"),
            note("skill/never-used", "skill"),
            note("reference/api", "reference"),
        ];

        // Record a recall hit for exactly one skill note.
        store
            .record_recall_hits(
                "how to handle async errors",
                "auto-recall",
                &[("skill/async-errors".to_string(), 0.9)],
                DEFAULT_AGENT_ID,
            )
            .await
            .unwrap();

        let m = compute_raw_metrics(&notes, store.as_ref(), DEFAULT_AGENT_ID, None).await;
        // Skill recall: 1 of 2 skill notes recalled → skill_recall_rate = 0.5,
        // feeding the strategy selector's growth_pressure with a real value
        // instead of the historical structural zero.
        assert_eq!(m.skill_notes_total, 2);
        assert_eq!(m.skill_notes_recalled, 1);
        // 3 notes, 1 recalled → 2 never recalled.
        assert_eq!(m.never_recalled_count, 2);
        assert!((m.note_hit_rate - 1.0 / 3.0).abs() < 1e-9);

        // The derived skill_recall_rate signal must now be non-zero.
        let snap = SignalSnapshot::from_metrics(&m);
        assert!((snap.score("skill_recall_rate") - 0.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn compute_raw_metrics_mature_skill_cohort_excludes_fresh() {
        let (_temp_guard, temp) = scratch_root();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());

        let now = now_timestamp();
        let old = now - 30 * 86_400; // mature: older than MATURE_SKILL_DAYS
        let skill = |path: &str, created: i64| NoteIndexEntry {
            path: path.into(),
            filename: path.rsplit('/').next().unwrap_or(path).into(),
            agent_id: DEFAULT_AGENT_ID.into(),
            category: "skill".into(),
            tags: vec![],
            link_count: 0,
            created_at: created,
            updated_at: created,
            content_hash: "h".into(),
        };
        // Two mature skills (one recalled) + one freshly-distilled skill.
        let notes = vec![
            skill("skill/mature-recalled", old),
            skill("skill/mature-cold", old),
            skill("skill/fresh", now),
        ];

        store
            .record_recall_hits(
                "q",
                "auto-recall",
                &[("skill/mature-recalled".to_string(), 0.9)],
                DEFAULT_AGENT_ID,
            )
            .await
            .unwrap();

        let m = compute_raw_metrics(&notes, store.as_ref(), DEFAULT_AGENT_ID, None).await;
        // All three count toward the whole-corpus skill stats...
        assert_eq!(m.skill_notes_total, 3);
        // ...but only the two mature notes form the wasted-distillation cohort;
        // the fresh note (no recall opportunity yet) is excluded, so it can't
        // drag the ratio toward a false Conserve.
        assert_eq!(m.mature_skill_total, 2);
        assert_eq!(m.mature_skill_recalled, 1);
    }

    #[tokio::test]
    async fn compute_raw_metrics_folds_in_prior_report_rot() {
        use crate::memory::dreaming::selector::{GateDecision, StrategySelector};

        let (_temp_guard, temp) = scratch_root();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let now = now_timestamp();
        let note = |path: &str| NoteIndexEntry {
            path: path.into(),
            filename: path.rsplit('/').next().unwrap_or(path).into(),
            agent_id: DEFAULT_AGENT_ID.into(),
            category: "reference".into(),
            tags: vec![],
            link_count: 0,
            created_at: now,
            updated_at: now,
            content_hash: "h".into(),
        };
        let notes: Vec<NoteIndexEntry> =
            (0..10).map(|i| note(&format!("reference/n{i}"))).collect();

        // No prior report → rot rates read a structural zero (self-evolution's
        // pre-fix behaviour) and the selector's stability gate is wide open.
        let without = compute_raw_metrics(&notes, store.as_ref(), DEFAULT_AGENT_ID, None).await;
        assert_eq!(without.contradiction_rate, 0.0);
        assert_eq!(without.duplication_rate, 0.0);
        assert_eq!(without.staleness_rate, 0.0);
        let stability_open = SignalSnapshot::from_metrics(&without);
        assert_eq!(stability_open.score("high_contradiction_rate"), 0.0);

        // Prior cycle's LLM stages found rot: 4 contradictions, 5 merged
        // duplicates, 2 stale marks over a 10-note corpus.
        let prior = DreamReport {
            contradictions_found: 4,
            notes_consolidated: 5,
            notes_marked_stale: 2,
            ..Default::default()
        };
        let with =
            compute_raw_metrics(&notes, store.as_ref(), DEFAULT_AGENT_ID, Some(&prior)).await;
        assert!((with.contradiction_rate - 0.4).abs() < 1e-9);
        assert!((with.duplication_rate - 0.5).abs() < 1e-9);
        assert!((with.staleness_rate - 0.2).abs() < 1e-9);

        // The revived rot signals must actually close the selector's stability
        // gate: with contradiction 0.4 + duplication 0.5, stability drops to
        // 1 - (0.4 + 0.5)/2 = 0.55 — still above MIN_STABILITY here, but the
        // health score must now carry a real penalty (was structurally inflated).
        let snap = SignalSnapshot::from_metrics(&with);
        assert!((snap.score("high_contradiction_rate") - 0.4).abs() < 1e-9);
        assert!((snap.score("high_duplication_rate") - 0.5).abs() < 1e-9);
        let healthy = memory_health_score(&stability_open);
        let rotten = memory_health_score(&snap);
        assert!(
            rotten < healthy,
            "rot penalty must lower health: rotten={rotten} healthy={healthy}"
        );

        // Sanity: a heavily-rotten prior actually forces Consolidate over
        // Synthesize even when growth pressure is high (the dead-gate bug).
        let mut hot = notes.clone();
        for (i, n) in hot.iter_mut().enumerate() {
            n.created_at = now; // all fresh → high growth pressure
            n.path = format!("reference/hot{i}");
        }
        let rotten_prior = DreamReport {
            contradictions_found: 9,
            notes_consolidated: 9,
            ..Default::default()
        };
        let hot_m =
            compute_raw_metrics(&hot, store.as_ref(), DEFAULT_AGENT_ID, Some(&rotten_prior)).await;
        let decision = StrategySelector::new()
            .select(&SignalSnapshot::from_metrics(&hot_m), &GateDecision::Allow);
        assert_eq!(
            decision.strategy,
            DreamStrategy::Consolidate,
            "high rot must veto synthesis via stability gate, got {:?}",
            decision.strategy
        );
    }

    #[tokio::test]
    async fn run_dream_executes_pipeline_not_stub() {
        let (_temp_guard, temp) = scratch_root();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let daemon = test_daemon(store, temp.clone());

        let outcome = daemon
            .run_dream(now_timestamp(), "2026-05-21".to_string(), true)
            .await
            .expect("run_dream succeeds");
        let report = &outcome.report;

        assert_eq!(outcome.status, DreamRunStatus::Success);
        // The historical stub returned an empty `stages_executed`. A real
        // pipeline run always executes `note_lint` (first stage of every
        // strategy, no `should_run` override).
        assert!(
            report.stages_executed.contains(&"note_lint".to_string()),
            "pipeline did not execute — stages_executed = {:?}",
            report.stages_executed
        );
        assert!(report.finished_at >= report.started_at);
        // The stage list must reach the persisted decision — it is `serde(skip)`
        // on the report itself, so `CycleDecision` is its only route to any
        // observer outside this process.
        assert_eq!(
            outcome.decision.stages, report.stages_executed,
            "the cycle decision must carry the executed stages"
        );
        assert_eq!(outcome.decision.strategy.to_string(), report.pipeline_type);
    }

    /// The whole point of deriving the churn gate from the event log: a brand
    /// new daemon (i.e. after a restart) must still honour a cooldown armed by
    /// a cycle the previous process ran. With the old in-RAM accumulator this
    /// conserve window silently evaporated on every reboot — and since cycles
    /// run at most once a day, "every reboot" was usually "before it ever
    /// applied".
    #[tokio::test]
    async fn a_restarted_daemon_still_honours_a_cooldown_from_the_log() {
        let (_temp_guard, temp) = scratch_root();
        // Create the note-memory dir up front: `SqliteMemoryBackend::new` only
        // treats its argument as a directory if it already exists, otherwise the
        // path becomes the DB *file* and the agent dir can never be created.
        std::fs::create_dir_all(&temp).unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let agent_dir = temp.join(DEFAULT_AGENT_ID);
        let log = EventLog::new(&agent_dir);

        // A previous process recorded a cycle that lowered memory health.
        let degrading = DreamReport {
            evolution: Some(EvolutionOutcome {
                baseline: 0.70,
                candidate: 0.30,
                best: 0.70,
                outcome: GateOutcome::Reject,
                merges_rejected: 0,
            }),
            ..Default::default()
        };
        log.append(&DreamEvent {
            id: "dream_1_1".into(),
            cycle: 1,
            strategy: DreamStrategy::Synthesize,
            selection: SelectionDecision {
                strategy: DreamStrategy::Synthesize,
                rationale: "prior".into(),
                personality_adjustment: 0.0,
            },
            gate_decision: GateDecision::Allow,
            report: degrading,
            validation: DreamValidationReport {
                l1_format: ValidationTier {
                    passed: true,
                    checks_run: 1,
                    checks_passed: 1,
                    issues: vec![],
                },
                l2_consistency: ValidationTier {
                    passed: true,
                    checks_run: 1,
                    checks_passed: 1,
                    issues: vec![],
                },
                l3_semantic: None,
                l4_retrospective: None,
            },
            duration_ms: 1,
            created_at: now_timestamp(),
        })
        .await
        .expect("append prior cycle");

        // Fresh daemon = fresh process. It must read that cycle back and
        // conserve rather than starting from a blank gate.
        let daemon = test_daemon(store, temp.clone());
        let outcome = daemon
            .run_dream(now_timestamp(), "2026-05-21".to_string(), true)
            .await
            .expect("run_dream succeeds");

        assert_eq!(
            outcome.decision.strategy,
            DreamStrategy::Conserve,
            "a degrading prior cycle must force Conserve after a restart"
        );
        assert!(
            matches!(outcome.decision.gate, GateDecision::Conserve { cooldown_remaining, .. } if cooldown_remaining > 0),
            "expected an active cooldown, got {:?}",
            outcome.decision.gate
        );
    }

    /// Forced cycles (`dreaming.run_now`) are real cycles: they must land in the
    /// `dream_reports` audit table like scheduled ones. This writer used to live
    /// only on the scheduled path, so a forced consolidation night was invisible
    /// to both the Panel's run history and the governance audit's activity probe.
    #[tokio::test]
    async fn run_now_persists_an_audit_row_with_its_decision() {
        let (_temp_guard, temp) = scratch_root();
        std::fs::create_dir_all(&temp).unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let daemon = test_daemon(store.clone(), temp.clone());

        daemon.run_now().await.expect("forced cycle succeeds");

        let rows = store
            .recent_dream_reports(Some(DEFAULT_AGENT_ID), 10)
            .expect("query audit table");
        assert_eq!(rows.len(), 1, "a forced cycle must be recorded");
        assert_eq!(
            rows[0].namespace, DEFAULT_AGENT_ID,
            "the base cycle files under the base agent, not a placeholder"
        );
        let decision_json = rows[0]
            .decision_json
            .as_deref()
            .expect("the row must carry the cycle decision");
        let decision: CycleDecision =
            serde_json::from_str(decision_json).expect("decision round-trips");
        assert!(
            !decision.rationale.is_empty(),
            "the decision must explain itself"
        );
        assert_eq!(decision.strategy.to_string(), rows[0].pipeline_type);
    }

    #[tokio::test]
    async fn run_dream_skips_gracefully_without_provider() {
        let (_temp_guard, temp) = scratch_root();
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        // No provider / embedder → DreamContext cannot be built; the daemon
        // must skip the pipeline gracefully rather than panic.
        let daemon = DreamDaemon::from_config(store, &MemoryConfig::default())
            .unwrap()
            .with_note_memory_dir(temp.clone());

        let outcome = daemon
            .run_dream(now_timestamp(), "2026-05-21".to_string(), true)
            .await
            .expect("run_dream succeeds without a provider");

        assert_eq!(outcome.status, DreamRunStatus::Success);
        assert!(outcome.report.stages_executed.is_empty());
    }

    // -----------------------------------------------------------------------
    // Per-project-namespace sub-cycles
    // -----------------------------------------------------------------------

    /// The owned half of a project sub-cycle's dependencies. `ProjectCycleDeps`
    /// borrows all of it, so it has to outlive the call in the test's frame.
    struct ProjectFixture {
        /// Owns the scratch tree. Dropping it deletes `dir`, so it must live in
        /// the fixture, not in the constructor's frame — a `let _guard = ...`
        /// there is dropped on `return`, deleting the directory before the test
        /// has touched it. That failure is nearly invisible: SQLite holds an
        /// open fd, so the store keeps working on an unlinked file and the test
        /// still passes.
        _guard: tempfile::TempDir,
        dir: PathBuf,
        store: Arc<SqliteMemoryBackend>,
        provider: Arc<dyn AiProvider>,
        embedder: Arc<dyn EmbeddingProvider>,
        cfg: MemoryConfig,
        orientation: Option<Arc<dyn crate::memory::notes::orientation::NoteOrientation>>,
        activity: Arc<dyn Fn() -> bool + Send + Sync>,
    }

    impl ProjectFixture {
        /// `user_active` drives the activity checker: `true` makes every stage
        /// yield, which is how an interrupted cycle is forced deterministically.
        fn new(tag: &str, user_active: bool) -> Self {
            let guard = tempfile::tempdir().expect("tempdir");
            // `tag` names the directory so a `KEEP`-style post-mortem can tell
            // the four project fixtures apart.
            let dir = guard.path().join(tag);
            std::fs::create_dir_all(&dir).unwrap();
            let store = Arc::new(SqliteMemoryBackend::new(&dir).unwrap());
            Self {
                _guard: guard,
                dir,
                store,
                provider: Arc::new(MockProvider::new("")),
                embedder: Arc::new(StubEmbedder),
                cfg: MemoryConfig::default(),
                orientation: None,
                activity: Arc::new(move || user_active),
            }
        }

        fn deps(&self) -> project_cycle::ProjectCycleDeps<'_> {
            project_cycle::ProjectCycleDeps {
                memory_dir: &self.dir,
                database: &self.store,
                provider: &self.provider,
                embedder: &self.embedder,
                config: &self.cfg.dreaming,
                decay_policy: &self.cfg.memory_decay,
                orientation: &self.orientation,
                activity_checker: &self.activity,
            }
        }

        fn event_log_path(&self, agent_id: &str) -> PathBuf {
            self.dir.join(agent_id).join("dream_events.jsonl")
        }

        /// Give a corpus one note so the nightly budget gate admits it.
        async fn seed_note(&self, agent_id: &str) {
            seed_corpus_note(&self.dir, &self.store, agent_id).await;
        }
    }

    /// Write one note into `agent_id`'s corpus.
    ///
    /// Every sub-cycle test needs this now: a corpus with no notes and no
    /// pending reviews has nothing for the maintenance subset to do, so the
    /// budget gate skips it before a single stage starts. Tests about what a
    /// cycle *records* therefore have to start from a corpus that has something
    /// to maintain — which is also the only shape that exists in production.
    async fn seed_corpus_note(dir: &Path, store: &Arc<SqliteMemoryBackend>, agent_id: &str) {
        // rust-doctor-disable-next-line excessive-clone
        NoteIndexer::new(dir.to_path_buf(), store.clone())
            .write_note(
                agent_id,
                "reference",
                &crate::memory::notes::KnowledgeNote {
                    title: "seed".into(),
                    category: "reference".into(),
                    body: Some("seed body".into()),
                    content_hash: "seed-hash".into(),
                    ..Default::default()
                },
            )
            .await
            .expect("seed note written");
    }

    /// Unwrap a sub-cycle that must have run. `Idle` here means the test forgot
    /// to seed the corpus, which is worth saying out loud rather than silently
    /// asserting against a default report.
    fn ran(cycle: project_cycle::NamespaceCycle) -> DreamCycleOutcome {
        match cycle {
            project_cycle::NamespaceCycle::Ran(outcome) => *outcome,
            project_cycle::NamespaceCycle::Idle => {
                panic!("the corpus was seeded, so the sub-cycle must have run")
            }
        }
    }

    /// A project namespace's cycle lands in **its own** event log, and never in
    /// the base agent's.
    ///
    /// Both halves matter. Before this, the sub-pipeline's report was
    /// `info!`-logged and dropped, so a project corpus could merge A→B and B→A
    /// every night with nothing able to see it. But appending to the *base* log
    /// would have been worse than the drop: a note `path` is relative within an
    /// agent, so `proj-a`'s `skill/foo` and the base agent's `skill/foo` are the
    /// same string and the base gate would inherit phantom merge cycles for
    /// notes it does not own.
    #[tokio::test]
    async fn project_namespace_cycle_writes_only_its_own_event_log() {
        let fx = ProjectFixture::new("dream_proj_log", false);
        let ns = format!("{DEFAULT_AGENT_ID}__proj-abc123");
        fx.seed_note(&ns).await;

        project_cycle::run_namespace_cycle(&fx.deps(), &ns)
            .await
            .expect("project sub-cycle succeeds");

        let events = EventLog::new(fx.dir.join(&ns))
            .read_last(10)
            .await
            .expect("the namespace log is readable");
        assert_eq!(
            events.len(),
            1,
            "the namespace must record its own cycle: this log is what its next \
             gate and personality fold over"
        );

        assert!(
            !fx.event_log_path(DEFAULT_AGENT_ID).exists(),
            "a project namespace must not write into the base agent's history"
        );
    }

    /// A namespace's gate is folded from the namespace's own log — not the base
    /// agent's, and not a process-local accumulator.
    ///
    /// Seeded with one degrading cycle, the next sub-cycle must open in
    /// Conserve. This is the whole reason the namespaces get their own logs
    /// rather than staying ungoverned: the cooldown has to survive the restart
    /// that a once-a-day cycle almost always straddles.
    #[tokio::test]
    async fn project_namespace_gate_folds_its_own_history() {
        let fx = ProjectFixture::new("dream_proj_gate", false);
        let ns = format!("{DEFAULT_AGENT_ID}__proj-degraded");
        fx.seed_note(&ns).await;
        let log = EventLog::new(fx.dir.join(&ns));

        // A prior cycle that degraded this namespace's memory health.
        let report = DreamReport {
            evolution: Some(EvolutionOutcome {
                baseline: 0.8,
                candidate: 0.4,
                best: 0.8,
                outcome: GateOutcome::Reject,
                merges_rejected: 0,
            }),
            ..Default::default()
        };
        assert!(
            report
                .evolution
                .as_ref()
                .is_some_and(EvolutionOutcome::degraded),
            "fixture must actually be a degrading cycle"
        );
        log.append(&DreamEvent {
            id: "dream_seed_1".into(),
            cycle: 1,
            strategy: DreamStrategy::Consolidate,
            selection: SelectionDecision {
                strategy: DreamStrategy::Consolidate,
                rationale: "seed".into(),
                personality_adjustment: 0.0,
            },
            gate_decision: GateDecision::Allow,
            report,
            validation: DreamValidationReport {
                l1_format: ValidationTier {
                    passed: true,
                    checks_run: 1,
                    checks_passed: 1,
                    issues: vec![],
                },
                l2_consistency: ValidationTier {
                    passed: true,
                    checks_run: 1,
                    checks_passed: 1,
                    issues: vec![],
                },
                l3_semantic: None,
                l4_retrospective: None,
            },
            duration_ms: 0,
            created_at: now_timestamp(),
        })
        .await
        .expect("seed append succeeds");

        project_cycle::run_namespace_cycle(&fx.deps(), &ns)
            .await
            .expect("project sub-cycle succeeds");

        let events = log.read_last(10).await.expect("log is readable");
        assert_eq!(events.len(), 2, "the new cycle appends to the same log");
        assert!(
            matches!(events[1].gate_decision, GateDecision::Conserve { .. }),
            "the degrading seed must arm this namespace's cooldown, got {:?}",
            events[1].gate_decision
        );
    }

    /// A sub-cycle that yielded before running a single stage records nothing.
    ///
    /// The gate window is only a few cycles deep. An evening where the user
    /// stays active would otherwise append one empty event per namespace and
    /// push real churn history out of range — disarming the detectors exactly
    /// when the corpus is being touched most.
    #[tokio::test]
    async fn an_immediately_interrupted_project_cycle_spends_no_window_slot() {
        let fx = ProjectFixture::new("dream_proj_intr", true);
        let ns = format!("{DEFAULT_AGENT_ID}__proj-busy");
        fx.seed_note(&ns).await;

        let outcome = ran(project_cycle::run_namespace_cycle(&fx.deps(), &ns)
            .await
            .expect("an interrupted sub-cycle is not an error"));

        assert_eq!(outcome.report.status, DreamReportStatus::Interrupted);
        assert!(outcome.report.stages_executed.is_empty(), "nothing ran");
        assert!(
            !fx.event_log_path(&ns).exists(),
            "an empty night must not consume one of the gate's window slots"
        );
        // The same predicate the caller reads before writing an audit row: the
        // two skips have to agree, and a row with no matching event (or the
        // reverse) is silent.
        assert!(
            outcome.report.is_vacuous_interruption(),
            "the caller decides whether to persist by reading this"
        );
    }

    /// The sub-cycle hands its decision back, which is what makes a project
    /// corpus's history writable to the audit table at all.
    ///
    /// It used to be computed inside the sub-cycle and dropped: the strategy,
    /// its rationale, the churn gate's verdict and the stage list all existed,
    /// went into the namespace's own event log, and reached no operator-facing
    /// surface, because the surface is fed by the caller and the caller had
    /// nothing to write.
    #[tokio::test]
    async fn a_project_sub_cycle_returns_its_decision() {
        let fx = ProjectFixture::new("dream_proj_decision", false);
        let ns = format!("{DEFAULT_AGENT_ID}__proj-decide");
        fx.seed_note(&ns).await;

        let outcome = ran(project_cycle::run_namespace_cycle(&fx.deps(), &ns)
            .await
            .expect("project sub-cycle succeeds"));

        assert_eq!(outcome.status, DreamRunStatus::Success);
        assert_eq!(
            outcome.decision.strategy.to_string(),
            outcome.report.pipeline_type,
            "the decision must describe the pipeline that actually ran"
        );
        assert_eq!(
            outcome.decision.stages, outcome.report.stages_executed,
            "the stage list is the one the pipeline executed, not a plan"
        );
        assert!(
            !outcome.decision.rationale.is_empty(),
            "the rationale is the operator's answer to 'why did it do that'"
        );
    }

    /// A project namespace's cycle lands in the `dream_reports` audit table
    /// under **its own** namespace.
    ///
    /// This is the operator-facing half of per-namespace self-governance. The
    /// model could already read a project corpus's history (it reads that
    /// namespace's event log directly via `note_manage(action="evolution")`);
    /// the Panel reads this table, and nothing wrote to it, so a project corpus
    /// dreamed every night invisibly.
    #[tokio::test]
    async fn a_project_sub_cycle_lands_in_the_audit_table_under_its_namespace() {
        let (_dir_guard, dir) = scratch_root();
        let ns = format!("{DEFAULT_AGENT_ID}__proj-audited");
        // `list_note_corpora` discovers corpora by directory listing.
        std::fs::create_dir_all(dir.join(&ns)).unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&dir).unwrap());
        seed_corpus_note(&dir, &store, &ns).await;

        // `project_scoped` is left at its default (false) on purpose: the
        // fan-out must not be gated on it. Personal and room partitions are
        // produced by session scope, which never reads that flag.
        let cfg = MemoryConfig::default();
        let daemon = DreamDaemon::from_config(store.clone(), &cfg)
            .unwrap()
            .with_provider(Arc::new(MockProvider::new("")))
            .with_embedder(Arc::new(StubEmbedder))
            .with_note_memory_dir(dir.clone());

        daemon
            .run_dream(now_timestamp(), "2026-08-04".to_string(), true)
            .await
            .expect("run_dream succeeds");

        let rows = store
            .recent_dream_reports(Some(&ns), 10)
            .expect("audit table is queryable");
        assert_eq!(
            rows.len(),
            1,
            "the project namespace's cycle must be visible to the operator"
        );
        assert_eq!(rows[0].namespace, ns);
        assert!(
            rows[0].decision_json.is_some(),
            "the 'why' travels with the row — that is the whole point of it"
        );

        // The base agent's own row is written by `run_dream`'s caller, so it is
        // absent here. What must never happen is a sub-cycle filing under the
        // base namespace: that would put a project corpus's counters into the
        // base agent's history.
        assert!(store
            .recent_dream_reports(Some(DEFAULT_AGENT_ID), 10)
            .expect("audit table is queryable")
            .is_empty());
    }

    // -----------------------------------------------------------------------
    // The nightly fan-out's enumeration and budget
    // -----------------------------------------------------------------------

    /// The fan-out covers **every** corpus family, under the default config.
    ///
    /// It used to enumerate `list_scoped_agent_ids(.., "main")` behind an
    /// `if self.project_scoped` gate. Both halves were wrong in the same
    /// direction and the combination was total: `project_scoped` defaults to
    /// false, while `__u-*` (personal) and `__p-*` (room) partitions are
    /// produced by session scope, which never consults that flag, and a
    /// non-default agent is not a sibling of `main` at all. So on a default
    /// install every one of these corpora went unmaintained from the day it was
    /// created — silently, because an untaken branch reports nothing.
    #[tokio::test]
    async fn every_corpus_family_is_maintained_under_the_default_config() {
        let (_dir_guard, dir) = scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&dir).unwrap());

        let personal = format!("{DEFAULT_AGENT_ID}__u-alice");
        let room = format!("{DEFAULT_AGENT_ID}__p-room7");
        let other_agent = "researcher".to_string();
        for ns in [&personal, &room, &other_agent] {
            seed_corpus_note(&dir, &store, ns).await;
        }

        let daemon = DreamDaemon::from_config(store.clone(), &MemoryConfig::default())
            .unwrap()
            .with_provider(Arc::new(MockProvider::new("")))
            .with_embedder(Arc::new(StubEmbedder))
            .with_note_memory_dir(dir.clone());

        daemon
            .run_dream(now_timestamp(), "2026-08-08".to_string(), true)
            .await
            .expect("run_dream succeeds");

        for ns in [&personal, &room, &other_agent] {
            let rows = store
                .recent_dream_reports(Some(ns), 10)
                .expect("audit table is queryable");
            assert_eq!(rows.len(), 1, "{ns} must have been maintained");
        }
    }

    /// `maintenance_corpora` is the enumeration, and it excludes exactly one
    /// thing: the base agent, which ran the full pipeline itself.
    #[test]
    fn maintenance_corpora_lists_every_corpus_except_the_base() {
        let (_dir_guard, dir) = scratch_root();
        for name in [
            DEFAULT_AGENT_ID,
            "main__u-alice",
            "main__p-room7",
            "main__proj-abc",
            "researcher",
            ".obsidian",
        ] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }

        assert_eq!(
            maintenance_corpora(&dir, DEFAULT_AGENT_ID),
            vec![
                "main__p-room7".to_string(),
                "main__proj-abc".to_string(),
                "main__u-alice".to_string(),
                "researcher".to_string(),
            ],
            "dot-directories are not corpora and the base agent is already done"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A corpus nothing has touched since its own last cycle is skipped before
    /// a single stage — and therefore before a single token — is spent.
    #[test]
    fn an_unchanged_corpus_is_skipped_before_any_llm_call() {
        let note = |updated_at: i64| NoteIndexEntry {
            path: "reference/x".into(),
            filename: "x".into(),
            agent_id: "main__u-alice".into(),
            category: "reference".into(),
            tags: vec![],
            link_count: 0,
            created_at: 100,
            updated_at,
            content_hash: "h".into(),
        };

        // Nothing on disk at all.
        assert!(!project_cycle::corpus_needs_maintenance(
            &[],
            1_000,
            0,
            false,
            false
        ));
        // Notes, none touched since the last cycle started.
        assert!(!project_cycle::corpus_needs_maintenance(
            &[note(500), note(999)],
            1_000,
            0,
            false,
            false
        ));
        // One note written during the last cycle: a capped stage may still have
        // backlog behind it, so the corpus gets one more night.
        assert!(project_cycle::corpus_needs_maintenance(
            &[note(500), note(1_000)],
            1_000,
            0,
            false,
            false
        ));
        // Never dreamed.
        assert!(project_cycle::corpus_needs_maintenance(
            &[note(500)],
            0,
            0,
            false,
            false
        ));
        // A pending review moves no `updated_at`, and `note_review` is the
        // queue's only consumer — a content-only gate would strand it forever.
        assert!(project_cycle::corpus_needs_maintenance(
            &[],
            1_000,
            1,
            false,
            false
        ));
        // An undistilled correction moves no `updated_at` and enqueues no
        // review either. `feedback_distill` is its only consumer and it runs
        // inside this cycle, so a gate blind to it strands every correction
        // filed against a non-base agent.
        assert!(project_cycle::corpus_needs_maintenance(
            &[],
            1_000,
            0,
            true,
            false,
        ));
        // Same for tool failures: `tool_failure_distill` is their only consumer
        // and they move no `updated_at` either. Without this leg a sub-agent
        // that fails at the same tool every day would never be woken to learn
        // from it — the note-content gate cannot see a raw_memory row.
        assert!(project_cycle::corpus_needs_maintenance(
            &[],
            1_000,
            0,
            false,
            true,
        ));
    }

    /// The fan-out's cost scales with the number of corpora, so it is bounded —
    /// and the bound the operator configured is the one that bites.
    ///
    /// Driven with a budget DIFFERENT from `MAX_CORPUS_CYCLES_PER_NIGHT` on
    /// purpose. At the default value this test would stay green even if the
    /// loop still read the constant and ignored `[memory.dreaming]` entirely —
    /// the "the config field exists, nothing reads it" shape, which no test
    /// written at the default value can see.
    #[tokio::test]
    async fn the_nightly_fan_out_stops_at_the_configured_budget() {
        const CONFIGURED_BUDGET: usize = 3;
        assert_ne!(
            CONFIGURED_BUDGET, MAX_CORPUS_CYCLES_PER_NIGHT,
            "this test is only meaningful when it disagrees with the default"
        );

        let (_dir_guard, dir) = scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&dir).unwrap());

        let corpora: Vec<String> = (0..CONFIGURED_BUDGET + 3)
            .map(|i| format!("{DEFAULT_AGENT_ID}__u-user{i:02}"))
            .collect();
        for ns in &corpora {
            seed_corpus_note(&dir, &store, ns).await;
        }

        let mut memory_config = MemoryConfig::default();
        memory_config.dreaming.max_corpus_cycles_per_night = CONFIGURED_BUDGET;
        let daemon = DreamDaemon::from_config(store.clone(), &memory_config)
            .unwrap()
            .with_provider(Arc::new(MockProvider::new("")))
            .with_embedder(Arc::new(StubEmbedder))
            .with_note_memory_dir(dir.clone());

        daemon
            .run_dream(now_timestamp(), "2026-08-08".to_string(), true)
            .await
            .expect("run_dream succeeds");

        let maintained = corpora
            .iter()
            .filter(|ns| {
                !store
                    .recent_dream_reports(Some(ns), 10)
                    .expect("audit table is queryable")
                    .is_empty()
            })
            .count();
        assert_eq!(
            maintained, CONFIGURED_BUDGET,
            "the night's budget is the CONFIGURED ceiling, not the compiled-in default"
        );
    }
}
