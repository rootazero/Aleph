//! DreamDaemon: background memory consolidation and graph decay.
//!
//! This module implements a staged dream pipeline architecture.
//! Each stage implements the `DreamStage` trait and operates on a shared
//! `DreamContext` that flows through the pipeline.

pub mod gate;
pub mod report;
pub mod stages;

use crate::config::{
    DreamingConfig as ConfigDreamingConfig, GraphDecayPolicy, MemoryConfig, MemoryDecayPolicy,
};
use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryEntry, MemoryFact, MemoryTier};
use crate::memory::decay::DecayConfig;
use crate::memory::graph::{GraphDecayConfig, GraphDecayReport, GraphStore};
use crate::memory::store::{DreamStore, MemoryBackend};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, AtomicI64, Ordering};
use chrono::{Local, NaiveTime, TimeZone};
use once_cell::sync::{Lazy, OnceCell};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{info, warn};

// Re-export stage types for backward compatibility
pub use stages::decay::MemoryDecayReport;
pub use stages::{
    ClusterStage, CollectStage, ConsolidateStage, DecayStage, DeepSynthesisStage,
    DriftDetectStage, SummarizeStage,
};
pub use stages::{DreamStage, DriftAction, MemoryCluster, MetadataGroupKey};

// Re-export gate types
pub use gate::{BlockReason, DreamGate, DreamGateConfig, GateResult};

// Re-export report types
pub use report::{DreamReport, DreamReportStatus, DreamRunMetadata, DreamRunType};

// ---------------------------------------------------------------------------
// DreamContext — shared state flowing through the pipeline
// ---------------------------------------------------------------------------

/// Shared context passed between dream pipeline stages.
pub struct DreamContext {
    pub memories: Vec<MemoryEntry>,
    pub clusters: Vec<MemoryCluster>,
    pub new_facts: Vec<MemoryFact>,
    pub drift_resolutions: Vec<DriftAction>,
    pub config: ConfigDreamingConfig,
    pub run_metadata: DreamRunMetadata,
    pub activity_checker: Arc<dyn Fn() -> bool + Send + Sync>,
    pub synthesis_insights_count: usize,
    pub database: MemoryBackend,
    pub graph_store: GraphStore,
    pub graph_decay_config: GraphDecayConfig,
    pub memory_decay_config: DecayConfig,
    pub command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
    /// Optional AI provider for LLM-powered stages (e.g. drift arbitration).
    pub provider: Option<Arc<dyn AiProvider>>,
    /// Output: graph decay report populated by DecayStage.
    pub graph_decay_report: Option<GraphDecayReport>,
    /// Output: memory decay report populated by DecayStage.
    pub memory_decay_report: Option<MemoryDecayReport>,
}

// ---------------------------------------------------------------------------
// DreamPipeline — stage executor
// ---------------------------------------------------------------------------

/// Executes a sequence of `DreamStage` implementations.
pub struct DreamPipeline {
    stages: Vec<Box<dyn DreamStage>>,
}

impl DreamPipeline {
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
        }
    }

    /// Append a stage to the pipeline (builder pattern).
    pub fn stage<S: DreamStage + 'static>(mut self, stage: S) -> Self {
        self.stages.push(Box::new(stage));
        self
    }

    /// Build the standard daily pipeline (6 stages).
    pub fn daily() -> Self {
        Self::new()
            .stage(CollectStage)
            .stage(ClusterStage)
            .stage(SummarizeStage)
            .stage(DriftDetectStage)
            .stage(ConsolidateStage)
            .stage(DecayStage)
    }

    /// Build the weekly pipeline (daily + deep synthesis).
    pub fn weekly() -> Self {
        Self::daily().stage(DeepSynthesisStage)
    }

    /// Run the pipeline, returning a `DreamReport`.
    pub async fn run(&self, mut ctx: DreamContext) -> Result<DreamReport, AlephError> {
        let mut executed: Vec<String> = Vec::new();

        for stage in &self.stages {
            if !stage.should_run(&ctx).await {
                continue;
            }
            // Check for user activity before each stage
            if (ctx.activity_checker)() {
                let mut report = DreamReport::interrupted(&ctx, stage.name());
                report.stages_executed = executed;
                return Ok(report);
            }
            ctx = stage.execute(ctx).await?;
            executed.push(stage.name().to_string());
        }

        let mut report = DreamReport::completed_default(&ctx);
        report.stages_executed = executed;
        Ok(report)
    }
}

impl Default for DreamPipeline {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Original DreamDaemon code (preserved unchanged)
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

    let daemon = if let Some(p) = provider {
        Arc::new(daemon_builder.with_provider(p))
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

struct DreamRunReport {
    status: DreamRunStatus,
    insight: Option<DailyInsight>,
    graph_decay: GraphDecayReport,
    memory_decay: MemoryDecayReport,
    memory_count: usize,
}

/// DreamDaemon orchestrates idle-time consolidation and decay.
pub struct DreamDaemon {
    database: MemoryBackend,
    graph_store: GraphStore,
    config: ConfigDreamingConfig,
    graph_decay: GraphDecayConfig,
    memory_decay: DecayConfig,
    window_start: NaiveTime,
    window_end: NaiveTime,
    is_running: AtomicBool,
    /// Optional event-sourcing command handler. When present, decay mutations
    /// are recorded as `StrengthDecayed` events in addition to the direct
    /// LanceDB update path.
    command_handler: Option<Arc<crate::memory::events::handler::MemoryCommandHandler>>,
    /// Optional AI provider for LLM-powered dream stages (e.g. drift arbitration).
    provider: Option<Arc<dyn AiProvider>>,
}

impl DreamDaemon {
    pub fn from_config(database: MemoryBackend, config: &MemoryConfig) -> Result<Self, AlephError> {
        let (window_start, window_end) = parse_window(&config.dreaming)?;
        let graph_decay = graph_decay_from_policy(&config.graph_decay);
        let memory_decay = decay_config_from_policy(&config.memory_decay);

        Ok(Self {
            graph_store: GraphStore::new(database.clone()),
            database,
            config: config.dreaming.clone(),
            graph_decay,
            memory_decay,
            window_start,
            window_end,
            is_running: AtomicBool::new(false),
            command_handler: None,
            provider: None,
        })
    }

    /// Attach an AI provider for LLM-powered dream stages.
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Attach an event-sourcing command handler so that decay mutations are
    /// also recorded as `StrengthDecayed` events.
    pub fn with_command_handler(
        mut self,
        handler: Arc<crate::memory::events::handler::MemoryCommandHandler>,
    ) -> Self {
        self.command_handler = Some(handler);
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

        // RAII guard: ensures is_running is reset even on early ? returns
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
            Ok(Ok(report)) => {
                if let Some(insight) = report.insight.clone() {
                    let _ = self.database.upsert_daily_insight(insight).await;
                }

                if report.status == DreamRunStatus::Cancelled {
                    info!(
                        memories = report.memory_count,
                        pruned_nodes = report.graph_decay.pruned_nodes,
                        pruned_edges = report.graph_decay.pruned_edges,
                        pruned_facts = report.memory_decay.pruned_facts,
                        "DreamDaemon cancelled due to activity"
                    );
                } else {
                    info!(
                        memories = report.memory_count,
                        pruned_nodes = report.graph_decay.pruned_nodes,
                        pruned_edges = report.graph_decay.pruned_edges,
                        pruned_facts = report.memory_decay.pruned_facts,
                        "DreamDaemon completed"
                    );
                }

                let _ = self
                    .database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some(report.status.as_str().to_string()),
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

        // _guard dropped here, resetting is_running to false
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

    /// Determine whether to run a daily or weekly dream cycle.
    async fn determine_run_type(&self) -> DreamRunType {
        if !self.config.weekly_enabled {
            return DreamRunType::Daily;
        }
        // Check last_weekly_at from DreamStatus
        if let Ok(status) = self.database.get_dream_status().await {
            if let Some(last_run) = status.last_run_at {
                // Simple heuristic: if last run was > weekly_interval_days ago, do weekly
                let days_since = (now_timestamp() - last_run) / 86400;
                if days_since >= self.config.weekly_interval_days as i64 {
                    return DreamRunType::Weekly;
                }
            }
        }
        DreamRunType::Daily
    }

    async fn run_dream(
        &self,
        run_start: i64,
        run_date: String,
    ) -> Result<DreamRunReport, AlephError> {
        let activity_snapshot = last_activity_timestamp().max(run_start);

        let run_type = self.determine_run_type().await;

        let pipeline = match run_type {
            DreamRunType::Daily => DreamPipeline::daily(),
            DreamRunType::Weekly => DreamPipeline::weekly(),
        };

        let ctx = DreamContext {
            memories: Vec::new(),
            clusters: Vec::new(),
            new_facts: Vec::new(),
            drift_resolutions: Vec::new(),
            config: self.config.clone(),
            run_metadata: DreamRunMetadata {
                run_type,
                run_date: run_date.clone(),
                run_start_ts: run_start,
            },
            activity_checker: Arc::new(move || last_activity_timestamp() > activity_snapshot),
            synthesis_insights_count: 0,
            database: self.database.clone(),
            graph_store: self.graph_store.clone(),
            graph_decay_config: self.graph_decay.clone(),
            memory_decay_config: self.memory_decay.clone(),
            command_handler: self.command_handler.clone(),
            provider: self.provider.clone(),
            graph_decay_report: None,
            memory_decay_report: None,
        };

        let report = pipeline.run(ctx).await?;

        // Convert DreamReport to legacy DreamRunReport
        Ok(DreamRunReport {
            status: match report.status {
                DreamReportStatus::Completed => DreamRunStatus::Success,
                DreamReportStatus::Interrupted => DreamRunStatus::Cancelled,
                DreamReportStatus::Failed => DreamRunStatus::Cancelled,
            },
            insight: None, // SummarizeStage already persisted the insight
            graph_decay: report.graph_decay_report.unwrap_or_default(),
            memory_decay: report.memory_decay_report.unwrap_or_default(),
            memory_count: report.memory_count,
        })
    }
}

/// Configuration for the STM->LTM consolidation pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationPipelineConfig {
    /// Whether consolidation is enabled
    pub enabled: bool,
    /// STM facts need this strength to be considered for consolidation
    pub strength_threshold: f32,
    /// Facts below this strength are permanently deleted
    pub pruning_threshold: f32,
    /// Max facts to process per Dream cycle
    pub max_facts_per_run: usize,
    /// Minimum days between consolidation checks for the same fact
    pub cooldown_days: u32,
}

impl Default for ConsolidationPipelineConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strength_threshold: 0.6,
            pruning_threshold: 0.1,
            max_facts_per_run: 50,
            cooldown_days: 1,
        }
    }
}

/// Check if a STM fact qualifies for consolidation into LTM.
/// Only ShortTerm facts with sufficient strength are candidates.
pub fn should_consolidate(fact: &MemoryFact, strength_threshold: f32) -> bool {
    fact.tier == MemoryTier::ShortTerm && fact.strength >= strength_threshold
}

/// Check if a fact should be pruned (deleted permanently).
/// Core tier facts are never pruned regardless of strength.
pub fn should_prune(fact: &MemoryFact, pruning_threshold: f32) -> bool {
    fact.tier != MemoryTier::Core && fact.strength < pruning_threshold
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

fn graph_decay_from_policy(policy: &GraphDecayPolicy) -> GraphDecayConfig {
    GraphDecayConfig {
        node_decay_per_day: policy.node_decay_per_day,
        edge_decay_per_day: policy.edge_decay_per_day,
        min_score: policy.min_score,
    }
}

fn decay_config_from_policy(policy: &MemoryDecayPolicy) -> DecayConfig {
    let mut config = DecayConfig {
        half_life_days: policy.half_life_days,
        access_boost: policy.access_boost,
        min_strength: policy.min_strength,
        protected_types: Vec::new(),
    };

    if policy.protected_types.is_empty() {
        config.protected_types.push(FactType::Personal);
    } else {
        for entry in &policy.protected_types {
            let fact_type = FactType::from_str_or_other(entry);
            if !config.protected_types.contains(&fact_type) {
                config.protected_types.push(fact_type);
            }
        }
    }

    config
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
    fn test_pipeline_builder() {
        let pipeline = DreamPipeline::daily();
        assert_eq!(pipeline.stages.len(), 6);
    }

    #[test]
    fn test_pipeline_weekly_has_seven_stages() {
        let pipeline = DreamPipeline::weekly();
        assert_eq!(pipeline.stages.len(), 7);
    }
}

#[cfg(test)]
mod consolidation_tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact, MemoryTier};

    #[test]
    fn test_should_consolidate_stm_high_strength() {
        let mut fact = MemoryFact::new("test".into(), FactType::Learning, vec![]);
        fact.tier = MemoryTier::ShortTerm;
        fact.strength = 0.7;
        assert!(should_consolidate(&fact, 0.6));
    }

    #[test]
    fn test_should_not_consolidate_low_strength() {
        let mut fact = MemoryFact::new("test".into(), FactType::Learning, vec![]);
        fact.tier = MemoryTier::ShortTerm;
        fact.strength = 0.4;
        assert!(!should_consolidate(&fact, 0.6));
    }

    #[test]
    fn test_should_not_consolidate_non_stm() {
        let mut fact = MemoryFact::new("test".into(), FactType::Learning, vec![]);
        fact.tier = MemoryTier::LongTerm;
        fact.strength = 0.9;
        assert!(!should_consolidate(&fact, 0.6));
    }

    #[test]
    fn test_should_not_consolidate_core() {
        let mut fact = MemoryFact::new("test".into(), FactType::Personal, vec![]);
        fact.tier = MemoryTier::Core;
        fact.strength = 0.9;
        assert!(!should_consolidate(&fact, 0.6));
    }

    #[test]
    fn test_should_prune_low_strength() {
        let mut fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
        fact.tier = MemoryTier::ShortTerm;
        fact.strength = 0.05;
        assert!(should_prune(&fact, 0.1));
    }

    #[test]
    fn test_should_not_prune_core() {
        let mut fact = MemoryFact::new("test".into(), FactType::Personal, vec![]);
        fact.tier = MemoryTier::Core;
        fact.strength = 0.01;
        assert!(!should_prune(&fact, 0.1));
    }

    #[test]
    fn test_should_not_prune_above_threshold() {
        let mut fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
        fact.tier = MemoryTier::ShortTerm;
        fact.strength = 0.5;
        assert!(!should_prune(&fact, 0.1));
    }

    #[test]
    fn test_consolidation_pipeline_config_defaults() {
        let config = ConsolidationPipelineConfig::default();
        assert!(config.enabled);
        assert!((config.strength_threshold - 0.6).abs() < f32::EPSILON);
        assert!((config.pruning_threshold - 0.1).abs() < f32::EPSILON);
        assert_eq!(config.max_facts_per_run, 50);
        assert_eq!(config.cooldown_days, 1);
    }
}

#[cfg(test)]
mod pipeline_integration_tests {
    use super::*;
    use crate::memory::store::lance::LanceMemoryBackend;

    async fn create_test_context(
        activity_detected: bool,
    ) -> (DreamContext, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();
        let database: MemoryBackend = Arc::new(backend);
        let graph_store = GraphStore::new(database.clone());

        let ctx = DreamContext {
            memories: Vec::new(),
            clusters: Vec::new(),
            new_facts: Vec::new(),
            drift_resolutions: Vec::new(),
            config: ConfigDreamingConfig::default(),
            run_metadata: DreamRunMetadata {
                run_type: DreamRunType::Daily,
                run_date: "2026-04-03".to_string(),
                run_start_ts: now_timestamp(),
            },
            activity_checker: Arc::new(move || activity_detected),
            synthesis_insights_count: 0,
            database,
            graph_store,
            graph_decay_config: GraphDecayConfig::default(),
            memory_decay_config: DecayConfig::default(),
            command_handler: None,
            provider: None,
            graph_decay_report: None,
            memory_decay_report: None,
        };

        (ctx, tmp)
    }

    #[tokio::test]
    async fn daily_pipeline_runs_all_stages() {
        let pipeline = DreamPipeline::daily();
        let (ctx, _tmp) = create_test_context(false).await;
        let report = pipeline.run(ctx).await.unwrap();
        assert_eq!(report.status, DreamReportStatus::Completed);
    }

    #[tokio::test]
    async fn weekly_pipeline_runs_all_stages() {
        let pipeline = DreamPipeline::weekly();
        let (ctx, _tmp) = create_test_context(false).await;
        let report = pipeline.run(ctx).await.unwrap();
        assert_eq!(report.status, DreamReportStatus::Completed);
    }

    #[tokio::test]
    async fn pipeline_interrupts_on_activity() {
        let pipeline = DreamPipeline::daily();
        let (ctx, _tmp) = create_test_context(true).await;
        let report = pipeline.run(ctx).await.unwrap();
        assert_eq!(report.status, DreamReportStatus::Interrupted);
    }

    #[tokio::test]
    async fn daily_pipeline_has_six_stages() {
        let pipeline = DreamPipeline::daily();
        assert_eq!(pipeline.stages.len(), 6);
    }

    #[tokio::test]
    async fn weekly_pipeline_has_seven_stages() {
        let pipeline = DreamPipeline::weekly();
        assert_eq!(pipeline.stages.len(), 7);
    }
}
