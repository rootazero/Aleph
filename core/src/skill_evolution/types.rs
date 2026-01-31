//! Core types for skill evolution system.
//!
//! Tracks skill executions and metrics to enable automatic solidification.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Status of a skill execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Success,
    PartialSuccess,
    Failed,
    Error,
}

/// A single skill execution record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecution {
    /// Unique execution ID
    pub id: String,
    /// Skill ID (or pattern hash for ad-hoc patterns)
    pub skill_id: String,
    /// Session ID where execution occurred
    pub session_id: String,
    /// Unix timestamp when invoked
    pub invoked_at: i64,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Execution status
    pub status: ExecutionStatus,
    /// User satisfaction score (0.0-1.0) if feedback provided
    pub satisfaction: Option<f32>,
    /// Context description (what was the user trying to do)
    pub context: String,
    /// Input summary (truncated)
    pub input_summary: String,
    /// Output length in characters
    pub output_length: u32,
}

impl SkillExecution {
    /// Create a new successful execution
    pub fn success(
        skill_id: impl Into<String>,
        session_id: impl Into<String>,
        context: impl Into<String>,
        input_summary: impl Into<String>,
        duration_ms: u64,
        output_length: u32,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            skill_id: skill_id.into(),
            session_id: session_id.into(),
            invoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            duration_ms,
            status: ExecutionStatus::Success,
            satisfaction: None,
            context: context.into(),
            input_summary: input_summary.into(),
            output_length,
        }
    }

    /// Create a failed execution
    pub fn failed(
        skill_id: impl Into<String>,
        session_id: impl Into<String>,
        context: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            skill_id: skill_id.into(),
            session_id: session_id.into(),
            invoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            duration_ms: 0,
            status: ExecutionStatus::Failed,
            satisfaction: None,
            context: context.into(),
            input_summary: String::new(),
            output_length: 0,
        }
    }

    /// Set user satisfaction
    pub fn with_satisfaction(mut self, score: f32) -> Self {
        self.satisfaction = Some(score.clamp(0.0, 1.0));
        self
    }
}

/// Aggregated metrics for a skill or pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetrics {
    /// Skill ID or pattern hash
    pub skill_id: String,
    /// Total number of executions
    pub total_executions: u64,
    /// Number of successful executions
    pub successful_executions: u64,
    /// Average duration in milliseconds
    pub avg_duration_ms: f32,
    /// Average satisfaction score (if feedback exists)
    pub avg_satisfaction: Option<f32>,
    /// Failure rate (0.0-1.0)
    pub failure_rate: f32,
    /// Last execution timestamp
    pub last_used: i64,
    /// First execution timestamp
    pub first_used: i64,
    /// Context frequency map (context -> count)
    pub context_frequency: HashMap<String, u32>,
}

impl SkillMetrics {
    /// Create empty metrics
    pub fn new(skill_id: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            skill_id: skill_id.into(),
            total_executions: 0,
            successful_executions: 0,
            avg_duration_ms: 0.0,
            avg_satisfaction: None,
            failure_rate: 0.0,
            last_used: now,
            first_used: now,
            context_frequency: HashMap::new(),
        }
    }

    /// Success rate (0.0-1.0)
    pub fn success_rate(&self) -> f32 {
        if self.total_executions == 0 {
            0.0
        } else {
            self.successful_executions as f32 / self.total_executions as f32
        }
    }

    /// Check if metrics meet solidification threshold
    pub fn meets_threshold(&self, config: &SolidificationConfig) -> bool {
        self.successful_executions >= config.min_success_count as u64
            && self.success_rate() >= config.min_success_rate
    }
}

/// Configuration for solidification detection
#[derive(Debug, Clone)]
pub struct SolidificationConfig {
    /// Minimum successful executions before suggesting solidification
    pub min_success_count: u32,
    /// Minimum success rate (0.0-1.0)
    pub min_success_rate: f32,
    /// Minimum days since first use
    pub min_age_days: u32,
    /// Maximum days since last use (to avoid stale patterns)
    pub max_idle_days: u32,
}

impl Default for SolidificationConfig {
    fn default() -> Self {
        Self {
            min_success_count: 3,
            min_success_rate: 0.8,
            min_age_days: 1,
            max_idle_days: 30,
        }
    }
}

/// A suggestion to solidify a pattern into a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolidificationSuggestion {
    /// Pattern hash or temporary skill ID
    pub pattern_id: String,
    /// Suggested skill name
    pub suggested_name: String,
    /// Suggested description
    pub suggested_description: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Metrics that triggered this suggestion
    pub metrics: SkillMetrics,
    /// Sample contexts where this pattern was used
    pub sample_contexts: Vec<String>,
    /// Generated instructions preview
    pub instructions_preview: String,
}

/// Result of skill generation
#[derive(Debug, Clone)]
pub enum GenerationResult {
    /// Successfully generated skill
    Generated {
        skill_id: String,
        file_path: String,
        diff_preview: String,
    },
    /// Skill already exists
    AlreadyExists { skill_id: String },
    /// Generation failed
    Failed { reason: String },
}

/// Result of git commit operation
#[derive(Debug, Clone)]
pub enum CommitResult {
    /// Successfully committed
    Committed {
        commit_hash: String,
        files_changed: Vec<String>,
    },
    /// Nothing to commit
    NothingToCommit,
    /// Commit failed
    Failed { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_execution_success() {
        let exec = SkillExecution::success(
            "test-skill",
            "session-1",
            "refactoring code",
            "refactor the auth module",
            1500,
            2000,
        );
        assert_eq!(exec.status, ExecutionStatus::Success);
        assert_eq!(exec.skill_id, "test-skill");
    }

    #[test]
    fn test_skill_metrics_success_rate() {
        let mut metrics = SkillMetrics::new("test");
        metrics.total_executions = 10;
        metrics.successful_executions = 8;
        assert_eq!(metrics.success_rate(), 0.8);
    }

    #[test]
    fn test_solidification_threshold() {
        let config = SolidificationConfig::default();
        let mut metrics = SkillMetrics::new("test");

        // Not enough executions
        metrics.total_executions = 2;
        metrics.successful_executions = 2;
        assert!(!metrics.meets_threshold(&config));

        // Meets threshold
        metrics.total_executions = 4;
        metrics.successful_executions = 4;
        assert!(metrics.meets_threshold(&config));
    }
}
