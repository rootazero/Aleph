//! DreamReport and run metadata for the dream pipeline.

use serde::{Deserialize, Serialize};

/// The type of dream run (daily vs weekly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamRunType {
    Daily,
    Weekly,
}

/// Metadata about a dream pipeline run.
#[derive(Debug, Clone)]
pub struct DreamRunMetadata {
    pub run_type: DreamRunType,
    pub run_date: String,
    pub run_start_ts: i64,
}

/// Final report produced by the dream pipeline.
#[derive(Debug, Clone)]
pub struct DreamReport {
    pub run_type: DreamRunType,
    pub run_date: String,
    pub status: DreamReportStatus,
    pub stages_executed: Vec<String>,
    pub interrupted_at_stage: Option<String>,
    pub memory_count: usize,
    pub clusters_count: usize,
    pub new_facts_count: usize,
    pub drift_resolutions_count: usize,
    pub synthesis_insights_count: usize,
}

/// Status of a completed dream pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamReportStatus {
    Completed,
    Interrupted,
    Failed,
}

impl DreamReport {
    /// Create a report for an interrupted pipeline run.
    pub fn interrupted(ctx: &super::DreamContext, stage_name: &str) -> Self {
        Self {
            run_type: ctx.run_metadata.run_type,
            run_date: ctx.run_metadata.run_date.clone(),
            status: DreamReportStatus::Interrupted,
            stages_executed: Vec::new(),
            interrupted_at_stage: Some(stage_name.to_string()),
            memory_count: ctx.memories.len(),
            clusters_count: ctx.clusters.len(),
            new_facts_count: ctx.new_facts.len(),
            drift_resolutions_count: ctx.drift_resolutions.len(),
            synthesis_insights_count: ctx.synthesis_insights_count,
        }
    }

    /// Create a default completed report from the final pipeline context.
    pub fn completed_default(ctx: &super::DreamContext) -> Self {
        Self {
            run_type: ctx.run_metadata.run_type,
            run_date: ctx.run_metadata.run_date.clone(),
            status: DreamReportStatus::Completed,
            stages_executed: Vec::new(),
            interrupted_at_stage: None,
            memory_count: ctx.memories.len(),
            clusters_count: ctx.clusters.len(),
            new_facts_count: ctx.new_facts.len(),
            drift_resolutions_count: ctx.drift_resolutions.len(),
            synthesis_insights_count: ctx.synthesis_insights_count,
        }
    }
}
