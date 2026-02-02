//! DreamDaemon: background memory consolidation and graph decay.

use crate::config::{DreamingConfig as ConfigDreamingConfig, GraphDecayPolicy, MemoryConfig, MemoryDecayPolicy};
use crate::error::AetherError;
use crate::memory::context::{FactType, MemoryEntry};
use crate::memory::database::VectorDatabase;
use crate::memory::decay::DecayConfig;
use crate::memory::graph::{GraphDecayConfig, GraphDecayReport, GraphStore};
use chrono::{Local, NaiveTime, TimeZone};
use once_cell::sync::{Lazy, OnceCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::{info, warn};

const DEFAULT_LOOKBACK_HOURS: i64 = 24;
const DEFAULT_MAX_MEMORIES: u32 = 500;
const DEFAULT_CHECK_INTERVAL_SECONDS: u64 = 60;

static LAST_ACTIVITY_TS: Lazy<AtomicI64> = Lazy::new(|| AtomicI64::new(now_timestamp()));
static DREAM_DAEMON: OnceCell<Arc<DreamDaemon>> = OnceCell::new();

fn now_timestamp() -> i64 {
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
pub fn ensure_dream_daemon(database: Arc<VectorDatabase>, config: Arc<MemoryConfig>) {
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

    let daemon = match DreamDaemon::from_config(database, &config) {
        Ok(daemon) => Arc::new(daemon),
        Err(err) => {
            warn!(error = %err, "DreamDaemon not started: invalid config");
            return;
        }
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

/// Memory decay summary.
#[derive(Debug, Clone, Default)]
pub struct MemoryDecayReport {
    pub updated_facts: u64,
    pub pruned_facts: u64,
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

#[derive(Debug, Clone)]
struct DreamCluster {
    app_bundle_id: String,
    window_title: String,
    memories: Vec<MemoryEntry>,
}

/// DreamDaemon orchestrates idle-time consolidation and decay.
pub struct DreamDaemon {
    database: Arc<VectorDatabase>,
    graph_store: GraphStore,
    config: ConfigDreamingConfig,
    graph_decay: GraphDecayConfig,
    memory_decay: DecayConfig,
    window_start: NaiveTime,
    window_end: NaiveTime,
    is_running: AtomicBool,
}

impl DreamDaemon {
    pub fn from_config(
        database: Arc<VectorDatabase>,
        config: &MemoryConfig,
    ) -> Result<Self, AetherError> {
        let (window_start, window_end) = parse_window(&config.dreaming)?;
        let graph_decay = graph_decay_from_policy(&config.graph_decay);
        let memory_decay = decay_config_from_policy(&config.memory_decay);

        Ok(Self {
            graph_store: GraphStore::new(Arc::clone(&database)),
            database,
            config: config.dreaming.clone(),
            graph_decay,
            memory_decay,
            window_start,
            window_end,
            is_running: AtomicBool::new(false),
        })
    }

    /// Start background scheduling task.
    pub fn start_background_task(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            self.run_scheduler().await;
        })
    }

    /// Start background task using an existing Tokio runtime handle.
    pub fn start_background_task_with_handle(self: Arc<Self>, handle: tokio::runtime::Handle) -> JoinHandle<()> {
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

    async fn check_and_run(&self) -> Result<(), AetherError> {
        if !self.config.enabled {
            return Ok(());
        }

        if !self.is_within_window() {
            return Ok(());
        }

        if idle_seconds() < self.config.idle_threshold_seconds as i64 {
            return Ok(());
        }

        if !self
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Ok(());
        }

        let run_start = now_timestamp();
        let run_date = Local::now().format("%Y-%m-%d").to_string();

        if let Ok(status) = self.database.get_dream_status().await {
            if let Some(last_run_at) = status.last_run_at {
                let last_date = Local.timestamp_opt(last_run_at, 0).single();
                if let Some(last_date) = last_date {
                    if last_date.format("%Y-%m-%d").to_string() == run_date
                        && status.last_status.as_deref() == Some("success")
                    {
                        self.is_running.store(false, Ordering::SeqCst);
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
                    self.database.upsert_daily_insight(insight).await?;
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

                self.database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some(report.status.as_str().to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await?;
            }
            Ok(Err(err)) => {
                warn!(error = %err, "DreamDaemon run failed");
                self.database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some("error".to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await?;
            }
            Err(_) => {
                warn!("DreamDaemon run timed out");
                self.database
                    .set_dream_status(DreamStatus {
                        last_run_at: Some(run_start),
                        last_status: Some("timeout".to_string()),
                        last_duration_ms: Some(duration_ms),
                    })
                    .await?;
            }
        }

        self.is_running.store(false, Ordering::SeqCst);
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
        run_date: String,
    ) -> Result<DreamRunReport, AetherError> {
        let activity_snapshot = last_activity_timestamp().max(run_start);

        let since = run_start - DEFAULT_LOOKBACK_HOURS * 3600;
        let memories = self
            .database
            .get_memories_since(since, DEFAULT_MAX_MEMORIES)
            .await?;

        let mut report = DreamRunReport {
            status: DreamRunStatus::Success,
            insight: None,
            graph_decay: GraphDecayReport::default(),
            memory_decay: MemoryDecayReport::default(),
            memory_count: memories.len(),
        };

        if activity_detected(activity_snapshot) {
            report.status = DreamRunStatus::Cancelled;
            report.insight = Some(DailyInsight::new(
                run_date.clone(),
                "Dream cancelled: user activity detected".to_string(),
                memories.len() as u32,
            ));
            return Ok(report);
        }

        let clusters = cluster_memories(memories);
        let summary = build_summary(&clusters, &run_date);

        report.insight = Some(DailyInsight::new(
            run_date.clone(),
            summary.clone(),
            report.memory_count as u32,
        ));

        if activity_detected(activity_snapshot) {
            report.status = DreamRunStatus::Cancelled;
            return Ok(report);
        }

        let entities = GraphStore::extract_entities_from_text(&summary);
        if !entities.is_empty() {
            let context_key = format!("dream:{}", run_date);
            let insight_aliases = vec![run_date.clone()];
            let insight_node = self
                .graph_store
                .upsert_node(
                    &format!("Daily Insight {}", run_date),
                    "insight",
                    &insight_aliases,
                    None,
                )
                .await?;

            for entity in entities {
                let node = self
                    .graph_store
                    .upsert_node(&entity, "concept", &[], None)
                    .await?;
                let _ = self
                    .graph_store
                    .upsert_edge(
                        &insight_node.id,
                        &node.id,
                        "mentions",
                        &context_key,
                        0.6,
                        1.0,
                    )
                    .await?;
            }
        }

        if activity_detected(activity_snapshot) {
            report.status = DreamRunStatus::Cancelled;
            return Ok(report);
        }

        report.graph_decay = self.graph_store.apply_decay(&self.graph_decay).await?;
        report.memory_decay = self.database.apply_fact_decay(&self.memory_decay).await?;

        if activity_detected(activity_snapshot) {
            report.status = DreamRunStatus::Cancelled;
        }

        Ok(report)
    }
}

fn activity_detected(snapshot: i64) -> bool {
    last_activity_timestamp() > snapshot
}

fn parse_window(config: &ConfigDreamingConfig) -> Result<(NaiveTime, NaiveTime), AetherError> {
    let start = NaiveTime::parse_from_str(&config.window_start_local, "%H:%M").map_err(|_| {
        AetherError::config(format!(
            "Invalid dreaming.window_start_local '{}', expected HH:MM",
            config.window_start_local
        ))
    })?;
    let end = NaiveTime::parse_from_str(&config.window_end_local, "%H:%M").map_err(|_| {
        AetherError::config(format!(
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
            let fact_type = FactType::from_str(entry);
            if !config.protected_types.contains(&fact_type) {
                config.protected_types.push(fact_type);
            }
        }
    }

    config
}

fn cluster_memories(memories: Vec<MemoryEntry>) -> Vec<DreamCluster> {
    let mut buckets: HashMap<String, DreamCluster> = HashMap::new();

    for memory in memories {
        let key = format!("{}::{}", memory.context.app_bundle_id, memory.context.window_title);
        if let Some(cluster) = buckets.get_mut(&key) {
            cluster.memories.push(memory);
        } else {
            buckets.insert(
                key,
                DreamCluster {
                    app_bundle_id: memory.context.app_bundle_id.clone(),
                    window_title: memory.context.window_title.clone(),
                    memories: vec![memory],
                },
            );
        }
    }

    let mut clusters: Vec<DreamCluster> = buckets.into_values().collect();
    clusters.sort_by_key(|cluster| std::cmp::Reverse(cluster.memories.len()));
    clusters
}

fn build_summary(clusters: &[DreamCluster], date: &str) -> String {
    if clusters.is_empty() {
        return format!("Daily Insight ({})\nNo recent memories recorded.", date);
    }

    let mut summary = String::new();
    summary.push_str(&format!("Daily Insight ({})\n", date));

    for cluster in clusters.iter().take(10) {
        let label = if cluster.window_title.is_empty() {
            cluster.app_bundle_id.clone()
        } else {
            format!("{} / {}", cluster.app_bundle_id, cluster.window_title)
        };

        let mut samples: Vec<String> = Vec::new();
        for memory in cluster.memories.iter().take(3) {
            let snippet = truncate_text(&memory.user_input, 80);
            if !snippet.is_empty() {
                samples.push(snippet);
            }
        }

        if samples.is_empty() {
            summary.push_str(&format!(
                "- {}: {} memories\n",
                label,
                cluster.memories.len()
            ));
        } else {
            summary.push_str(&format!(
                "- {}: {} memories. Examples: {}\n",
                label,
                cluster.memories.len(),
                samples.join("; ")
            ));
        }
    }

    summary.trim_end().to_string()
}

fn truncate_text(text: &str, max_len: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut chars = trimmed.chars();
    let truncated: String = chars.by_ref().take(max_len).collect();
    if chars.next().is_some() {
        format!("{}...", truncated)
    } else {
        truncated
    }
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
}
