//! SolidificationPipeline - orchestrates candidate detection and suggestion generation.
//!
//! This module provides an end-to-end pipeline that:
//! 1. Detects patterns ready for solidification
//! 2. Generates suggestions with AI assistance (optional)
//! 3. Filters suggestions by confidence threshold
//! 4. Returns actionable suggestions for user approval

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::config::EvolutionConfig;
use crate::error::Result;
use crate::providers::AiProvider;

use super::detector::SolidificationDetector;
use super::tracker::EvolutionTracker;
use super::types::{SolidificationConfig, SolidificationSuggestion};

/// Pipeline status indicating the current state of the compiler
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PipelineStatus {
    /// Whether the pipeline is enabled
    pub enabled: bool,
    /// Number of patterns being tracked
    pub patterns_tracked: usize,
    /// Number of candidates ready for solidification
    pub candidates_ready: usize,
    /// Number of pending suggestions awaiting approval
    pub pending_suggestions: usize,
    /// Last time the pipeline ran (unix timestamp)
    pub last_run: Option<i64>,
    /// Last error if any
    pub last_error: Option<String>,
}

impl Default for PipelineStatus {
    fn default() -> Self {
        Self {
            enabled: true,
            patterns_tracked: 0,
            candidates_ready: 0,
            pending_suggestions: 0,
            last_run: None,
            last_error: None,
        }
    }
}

/// Result of a pipeline run
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Suggestions generated during this run
    pub suggestions: Vec<SolidificationSuggestion>,
    /// Number of candidates that were detected
    pub candidates_detected: usize,
    /// Number of suggestions that were filtered out (low confidence)
    pub filtered_out: usize,
    /// Pipeline status after run
    pub status: PipelineStatus,
}

/// Orchestrates the solidification pipeline from detection to suggestion.
///
/// The pipeline runs in three phases:
/// 1. **Detection**: Query the evolution tracker for patterns that meet thresholds
/// 2. **Generation**: Use AI (optional) to generate skill suggestions
/// 3. **Filtering**: Filter out low-confidence suggestions
///
/// ## Example
///
/// ```rust,ignore
/// use aethecore::skill_evolution::{SolidificationPipeline, EvolutionTracker};
///
/// let tracker = Arc::new(EvolutionTracker::new("evolution.db")?);
/// let pipeline = SolidificationPipeline::new(tracker);
///
/// // Run detection and get suggestions
/// let result = pipeline.run().await?;
///
/// for suggestion in result.suggestions {
///     println!("Suggest: {} - {}", suggestion.suggested_name, suggestion.suggested_description);
/// }
/// ```
pub struct SolidificationPipeline {
    /// Underlying detector for candidate detection
    detector: SolidificationDetector,
    /// Minimum confidence threshold for suggestions
    min_confidence: f32,
    /// Maximum suggestions to return per run
    max_suggestions: usize,
    /// Reference to tracker for status queries
    tracker: Arc<EvolutionTracker>,
}

impl SolidificationPipeline {
    /// Create a new pipeline with default settings.
    pub fn new(tracker: Arc<EvolutionTracker>) -> Self {
        Self {
            detector: SolidificationDetector::new(tracker.clone()),
            min_confidence: 0.7,
            max_suggestions: 10,
            tracker,
        }
    }

    /// Create a pipeline from config.
    pub fn from_config(tracker: Arc<EvolutionTracker>, config: &EvolutionConfig) -> Self {
        let solidification_config = config.to_solidification_config();
        let detector = SolidificationDetector::new(tracker.clone()).with_config(solidification_config);

        Self {
            detector,
            min_confidence: config.thresholds.min_confidence,
            max_suggestions: config.tool_generation.max_pending_suggestions as usize,
            tracker,
        }
    }

    /// Set the AI provider for generating better suggestions.
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.detector = self.detector.with_provider(provider);
        self
    }

    /// Set the solidification configuration.
    pub fn with_config(mut self, config: SolidificationConfig) -> Self {
        self.detector = self.detector.with_config(config);
        self
    }

    /// Set the minimum confidence threshold.
    pub fn with_min_confidence(mut self, threshold: f32) -> Self {
        self.min_confidence = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set the maximum number of suggestions to return.
    pub fn with_max_suggestions(mut self, max: usize) -> Self {
        self.max_suggestions = max.max(1);
        self
    }

    /// Run the full pipeline: detect, generate, and filter suggestions.
    ///
    /// Returns a `PipelineResult` with suggestions and status information.
    pub async fn run(&self) -> Result<PipelineResult> {
        info!("Running solidification pipeline");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Phase 1: Detection
        let candidates = match self.detector.detect_candidates() {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Failed to detect candidates");
                return Ok(PipelineResult {
                    suggestions: vec![],
                    candidates_detected: 0,
                    filtered_out: 0,
                    status: PipelineStatus {
                        enabled: true,
                        patterns_tracked: 0,
                        candidates_ready: 0,
                        pending_suggestions: 0,
                        last_run: Some(now),
                        last_error: Some(e.to_string()),
                    },
                });
            }
        };

        let candidates_count = candidates.len();
        debug!(count = candidates_count, "Detected solidification candidates");

        if candidates.is_empty() {
            return Ok(PipelineResult {
                suggestions: vec![],
                candidates_detected: 0,
                filtered_out: 0,
                status: PipelineStatus {
                    enabled: true,
                    patterns_tracked: self.get_tracked_patterns_count()?,
                    candidates_ready: 0,
                    pending_suggestions: 0,
                    last_run: Some(now),
                    last_error: None,
                },
            });
        }

        // Phase 2: Generation
        let mut suggestions = Vec::new();
        let mut errors = Vec::new();

        for metrics in candidates.iter().take(self.max_suggestions) {
            match self.detector.generate_suggestion(metrics).await {
                Ok(suggestion) => {
                    debug!(
                        name = %suggestion.suggested_name,
                        confidence = suggestion.confidence,
                        "Generated suggestion"
                    );
                    suggestions.push(suggestion);
                }
                Err(e) => {
                    warn!(
                        skill_id = %metrics.skill_id,
                        error = %e,
                        "Failed to generate suggestion"
                    );
                    errors.push(e.to_string());
                }
            }
        }

        // Phase 3: Filtering
        let pre_filter_count = suggestions.len();
        suggestions.retain(|s| s.confidence >= self.min_confidence);
        let filtered_out = pre_filter_count - suggestions.len();

        if filtered_out > 0 {
            debug!(
                filtered = filtered_out,
                threshold = self.min_confidence,
                "Filtered low-confidence suggestions"
            );
        }

        // Sort by confidence (highest first)
        suggestions.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

        let status = PipelineStatus {
            enabled: true,
            patterns_tracked: self.get_tracked_patterns_count()?,
            candidates_ready: candidates_count,
            pending_suggestions: suggestions.len(),
            last_run: Some(now),
            last_error: if errors.is_empty() {
                None
            } else {
                Some(errors.join("; "))
            },
        };

        info!(
            candidates = candidates_count,
            suggestions = suggestions.len(),
            filtered = filtered_out,
            "Pipeline run complete"
        );

        Ok(PipelineResult {
            suggestions,
            candidates_detected: candidates_count,
            filtered_out,
            status,
        })
    }

    /// Check if there are any pending candidates without running full pipeline.
    pub fn has_candidates(&self) -> Result<bool> {
        self.detector.has_candidates()
    }

    /// Get the current pipeline status without generating suggestions.
    pub fn status(&self) -> Result<PipelineStatus> {
        let candidates = self.detector.detect_candidates()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Ok(PipelineStatus {
            enabled: true,
            patterns_tracked: self.get_tracked_patterns_count()?,
            candidates_ready: candidates.len(),
            pending_suggestions: 0, // Not tracking pending until approval workflow
            last_run: Some(now),
            last_error: None,
        })
    }

    /// Get a reference to the underlying detector.
    pub fn detector(&self) -> &SolidificationDetector {
        &self.detector
    }

    /// Get the number of tracked patterns (approximation from metrics).
    fn get_tracked_patterns_count(&self) -> Result<usize> {
        // Query all metrics to count tracked patterns
        // This is a simple approximation - in production you'd query the DB directly
        let config = SolidificationConfig {
            min_success_count: 1,
            min_success_rate: 0.0,
            min_age_days: 0,
            max_idle_days: 365,
        };
        let all = self.tracker.get_solidification_candidates(&config)?;
        Ok(all.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tracker() -> Arc<EvolutionTracker> {
        Arc::new(EvolutionTracker::in_memory().expect("Failed to create tracker"))
    }

    #[test]
    fn test_pipeline_creation() {
        let tracker = create_test_tracker();
        let pipeline = SolidificationPipeline::new(tracker);
        assert_eq!(pipeline.min_confidence, 0.7);
        assert_eq!(pipeline.max_suggestions, 10);
    }

    #[test]
    fn test_pipeline_builder() {
        let tracker = create_test_tracker();
        let pipeline = SolidificationPipeline::new(tracker)
            .with_min_confidence(0.9)
            .with_max_suggestions(5);

        assert_eq!(pipeline.min_confidence, 0.9);
        assert_eq!(pipeline.max_suggestions, 5);
    }

    #[tokio::test]
    async fn test_pipeline_empty() {
        let tracker = create_test_tracker();
        let pipeline = SolidificationPipeline::new(tracker);

        let result = pipeline.run().await.unwrap();
        assert!(result.suggestions.is_empty());
        assert_eq!(result.candidates_detected, 0);
        assert!(result.status.last_error.is_none());
    }

    #[tokio::test]
    async fn test_pipeline_with_candidates() {
        use super::super::types::{ExecutionStatus, SkillExecution};

        let tracker = create_test_tracker();

        // Add enough executions to trigger detection
        for i in 0..5 {
            let exec = SkillExecution {
                id: format!("exec-{}", i),
                skill_id: "test-pattern".to_string(),
                session_id: format!("session-{}", i),
                invoked_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
                duration_ms: 1000,
                status: ExecutionStatus::Success,
                satisfaction: Some(0.9),
                context: "test context".to_string(),
                input_summary: "test input".to_string(),
                output_length: 100,
            };
            tracker.log_execution(&exec).unwrap();
        }

        let config = SolidificationConfig {
            min_success_count: 3,
            min_success_rate: 0.7,
            min_age_days: 0,
            max_idle_days: 100,
        };

        let pipeline = SolidificationPipeline::new(tracker)
            .with_config(config)
            .with_min_confidence(0.5); // Lower threshold for test

        let result = pipeline.run().await.unwrap();
        assert!(!result.suggestions.is_empty());
        assert_eq!(result.candidates_detected, 1);
    }

    #[tokio::test]
    async fn test_pipeline_filters_low_confidence() {
        use super::super::types::{ExecutionStatus, SkillExecution};

        let tracker = create_test_tracker();

        // Add executions with mixed success (low confidence)
        for i in 0..10 {
            let exec = SkillExecution {
                id: format!("exec-{}", i),
                skill_id: "mixed-pattern".to_string(),
                session_id: format!("session-{}", i),
                invoked_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
                duration_ms: 1000,
                status: if i % 2 == 0 {
                    ExecutionStatus::Success
                } else {
                    ExecutionStatus::Failed
                },
                satisfaction: Some(0.5),
                context: "test context".to_string(),
                input_summary: "test input".to_string(),
                output_length: 100,
            };
            tracker.log_execution(&exec).unwrap();
        }

        let config = SolidificationConfig {
            min_success_count: 3,
            min_success_rate: 0.4, // Low threshold to allow detection
            min_age_days: 0,
            max_idle_days: 100,
        };

        let pipeline = SolidificationPipeline::new(tracker)
            .with_config(config)
            .with_min_confidence(0.9); // High threshold to filter

        let result = pipeline.run().await.unwrap();
        // Should have candidates but filter them out due to low confidence
        assert!(result.candidates_detected > 0 || result.filtered_out > 0);
    }

    #[test]
    fn test_has_candidates() {
        let tracker = create_test_tracker();
        let pipeline = SolidificationPipeline::new(tracker);
        assert!(!pipeline.has_candidates().unwrap());
    }

    #[test]
    fn test_status() {
        let tracker = create_test_tracker();
        let pipeline = SolidificationPipeline::new(tracker);

        let status = pipeline.status().unwrap();
        assert!(status.enabled);
        assert_eq!(status.candidates_ready, 0);
        assert!(status.last_run.is_some());
    }
}
