//! P2/P3 Feature FFI Types
//!
//! Contains advanced feature FFI types:
//! - P2: Prompt Analysis (LanguageFFI, ReasoningLevelFFI, ContextSizeFFI, DomainFFI, PromptFeaturesFFI)
//! - P2: Semantic Cache (CacheHitTypeFFI, CacheStatsFFI)
//! - P3: A/B Testing (ExperimentStatusFFI, ExperimentSummaryFFI, VariantSummaryFFI, etc.)
//! - P3: Ensemble (EnsembleModeFFI, QualityMetricFFI, EnsembleConfigSummaryFFI, etc.)

// ============================================================================
// P2: Prompt Analysis FFI Types
// ============================================================================

/// Detected language for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageFFI {
    English,
    Chinese,
    Japanese,
    Korean,
    Mixed,
    Unknown,
}

impl From<crate::dispatcher::model_router::Language> for LanguageFFI {
    fn from(lang: crate::dispatcher::model_router::Language) -> Self {
        match lang {
            crate::dispatcher::model_router::Language::English => LanguageFFI::English,
            crate::dispatcher::model_router::Language::Chinese => LanguageFFI::Chinese,
            crate::dispatcher::model_router::Language::Japanese => LanguageFFI::Japanese,
            crate::dispatcher::model_router::Language::Korean => LanguageFFI::Korean,
            crate::dispatcher::model_router::Language::Mixed => LanguageFFI::Mixed,
            crate::dispatcher::model_router::Language::Unknown => LanguageFFI::Unknown,
        }
    }
}

/// Reasoning level for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningLevelFFI {
    Low,
    Medium,
    High,
}

impl From<crate::dispatcher::model_router::ReasoningLevel> for ReasoningLevelFFI {
    fn from(level: crate::dispatcher::model_router::ReasoningLevel) -> Self {
        match level {
            crate::dispatcher::model_router::ReasoningLevel::Low => ReasoningLevelFFI::Low,
            crate::dispatcher::model_router::ReasoningLevel::Medium => ReasoningLevelFFI::Medium,
            crate::dispatcher::model_router::ReasoningLevel::High => ReasoningLevelFFI::High,
        }
    }
}

/// Suggested context size for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSizeFFI {
    Small,
    Medium,
    Large,
}

impl From<crate::dispatcher::model_router::ContextSize> for ContextSizeFFI {
    fn from(size: crate::dispatcher::model_router::ContextSize) -> Self {
        match size {
            crate::dispatcher::model_router::ContextSize::Small => ContextSizeFFI::Small,
            crate::dispatcher::model_router::ContextSize::Medium => ContextSizeFFI::Medium,
            crate::dispatcher::model_router::ContextSize::Large => ContextSizeFFI::Large,
        }
    }
}

/// Detected domain for FFI (simplified)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainFFI {
    General,
    Creative,
    Conversational,
    TechnicalProgramming,
    TechnicalMathematics,
    TechnicalScience,
    TechnicalEngineering,
    TechnicalDataScience,
    TechnicalOther,
}

impl From<crate::dispatcher::model_router::Domain> for DomainFFI {
    fn from(domain: crate::dispatcher::model_router::Domain) -> Self {
        match domain {
            crate::dispatcher::model_router::Domain::General => DomainFFI::General,
            crate::dispatcher::model_router::Domain::Creative => DomainFFI::Creative,
            crate::dispatcher::model_router::Domain::Conversational => DomainFFI::Conversational,
            crate::dispatcher::model_router::Domain::Technical(tech) => match tech {
                crate::dispatcher::model_router::TechnicalDomain::Programming => {
                    DomainFFI::TechnicalProgramming
                }
                crate::dispatcher::model_router::TechnicalDomain::Mathematics => {
                    DomainFFI::TechnicalMathematics
                }
                crate::dispatcher::model_router::TechnicalDomain::Science => {
                    DomainFFI::TechnicalScience
                }
                crate::dispatcher::model_router::TechnicalDomain::Engineering => {
                    DomainFFI::TechnicalEngineering
                }
                crate::dispatcher::model_router::TechnicalDomain::DataScience => {
                    DomainFFI::TechnicalDataScience
                }
                crate::dispatcher::model_router::TechnicalDomain::Other(_) => {
                    DomainFFI::TechnicalOther
                }
            },
        }
    }
}

/// Prompt features extracted by PromptAnalyzer for FFI
#[derive(Debug, Clone)]
pub struct PromptFeaturesFFI {
    /// Estimated token count
    pub estimated_tokens: u32,
    /// Complexity score (0.0 - 1.0)
    pub complexity_score: f64,
    /// Primary detected language
    pub primary_language: LanguageFFI,
    /// Confidence in language detection (0.0 - 1.0)
    pub language_confidence: f64,
    /// Ratio of code content (0.0 - 1.0)
    pub code_ratio: f64,
    /// Detected reasoning level
    pub reasoning_level: ReasoningLevelFFI,
    /// Detected domain
    pub domain: DomainFFI,
    /// Suggested context size for model selection
    pub suggested_context_size: ContextSizeFFI,
    /// Analysis time in microseconds
    pub analysis_time_us: u64,
    /// Whether prompt contains code blocks
    pub has_code_blocks: bool,
    /// Number of questions detected
    pub question_count: u32,
    /// Number of imperative statements detected
    pub imperative_count: u32,
}

impl From<crate::dispatcher::model_router::PromptFeatures> for PromptFeaturesFFI {
    fn from(features: crate::dispatcher::model_router::PromptFeatures) -> Self {
        Self {
            estimated_tokens: features.estimated_tokens,
            complexity_score: features.complexity_score,
            primary_language: features.primary_language.into(),
            language_confidence: features.language_confidence,
            code_ratio: features.code_ratio,
            reasoning_level: features.reasoning_level.into(),
            domain: features.domain.into(),
            suggested_context_size: features.suggested_context_size.into(),
            analysis_time_us: features.analysis_time_us,
            has_code_blocks: features.has_code_blocks,
            question_count: features.question_count,
            imperative_count: features.imperative_count,
        }
    }
}

// ============================================================================
// P2: Semantic Cache FFI Types
// ============================================================================

/// Cache hit type for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheHitTypeFFI {
    /// Exact hash match
    Exact,
    /// Semantic similarity match
    Semantic,
}

impl From<crate::dispatcher::model_router::CacheHitType> for CacheHitTypeFFI {
    fn from(hit_type: crate::dispatcher::model_router::CacheHitType) -> Self {
        match hit_type {
            crate::dispatcher::model_router::CacheHitType::Exact => CacheHitTypeFFI::Exact,
            crate::dispatcher::model_router::CacheHitType::Semantic => CacheHitTypeFFI::Semantic,
        }
    }
}

/// Cache statistics for FFI
#[derive(Debug, Clone)]
pub struct CacheStatsFFI {
    /// Total number of entries in cache
    pub total_entries: u64,
    /// Total size in bytes
    pub total_size_bytes: u64,
    /// Number of cache hits
    pub hit_count: u64,
    /// Number of cache misses
    pub miss_count: u64,
    /// Hit rate (0.0 - 1.0)
    pub hit_rate: f64,
    /// Number of exact hash hits
    pub exact_hits: u64,
    /// Number of semantic similarity hits
    pub semantic_hits: u64,
    /// Total number of evictions
    pub evictions: u64,
}

impl From<crate::dispatcher::model_router::CacheStats> for CacheStatsFFI {
    fn from(stats: crate::dispatcher::model_router::CacheStats) -> Self {
        Self {
            total_entries: stats.total_entries as u64,
            total_size_bytes: stats.total_size_bytes as u64,
            hit_count: stats.hit_count,
            miss_count: stats.miss_count,
            hit_rate: stats.hit_rate,
            exact_hits: stats.exact_hits,
            semantic_hits: stats.semantic_hits,
            evictions: stats.evictions,
        }
    }
}

impl CacheStatsFFI {
    /// Create empty cache stats (when cache is disabled)
    pub fn empty() -> Self {
        Self {
            total_entries: 0,
            total_size_bytes: 0,
            hit_count: 0,
            miss_count: 0,
            hit_rate: 0.0,
            exact_hits: 0,
            semantic_hits: 0,
            evictions: 0,
        }
    }
}

// ============================================================================
// P3: A/B Testing FFI Types
// ============================================================================

/// Experiment status for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentStatusFFI {
    Running,
    Paused,
    Completed,
    InsufficientData,
}

impl From<crate::dispatcher::model_router::ExperimentStatus> for ExperimentStatusFFI {
    fn from(status: crate::dispatcher::model_router::ExperimentStatus) -> Self {
        match status {
            crate::dispatcher::model_router::ExperimentStatus::Running => {
                ExperimentStatusFFI::Running
            }
            crate::dispatcher::model_router::ExperimentStatus::Paused => {
                ExperimentStatusFFI::Paused
            }
            crate::dispatcher::model_router::ExperimentStatus::Completed => {
                ExperimentStatusFFI::Completed
            }
            crate::dispatcher::model_router::ExperimentStatus::InsufficientData => {
                ExperimentStatusFFI::InsufficientData
            }
        }
    }
}

/// A/B experiment summary for FFI (simplified view for UI)
#[derive(Debug, Clone)]
pub struct ExperimentSummaryFFI {
    /// Unique experiment ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Current status
    pub status: ExperimentStatusFFI,
    /// Status as display string
    pub status_display: String,
    /// Whether the experiment is enabled
    pub enabled: bool,
    /// Traffic percentage (0-100)
    pub traffic_percentage: u8,
    /// Number of variants
    pub variant_count: u32,
    /// Total samples collected
    pub total_samples: u64,
    /// Duration in seconds since start
    pub duration_secs: u64,
    /// Target intent filter (if any)
    pub target_intent: Option<String>,
}

impl From<&crate::dispatcher::model_router::ExperimentReport> for ExperimentSummaryFFI {
    fn from(report: &crate::dispatcher::model_router::ExperimentReport) -> Self {
        Self {
            id: report.experiment_id.clone(),
            name: report.experiment_name.clone(),
            status: report.status.into(),
            status_display: report.status.display_name().to_string(),
            enabled: report.status != crate::dispatcher::model_router::ExperimentStatus::Paused,
            traffic_percentage: 0, // Not available in report, would need config
            variant_count: report.variant_summaries.len() as u32,
            total_samples: report.total_samples,
            duration_secs: report.duration_secs,
            target_intent: None, // Not available in report
        }
    }
}

/// Variant summary for FFI
#[derive(Debug, Clone)]
pub struct VariantSummaryFFI {
    /// Variant ID
    pub id: String,
    /// Variant name
    pub name: String,
    /// Sample count
    pub sample_count: u64,
    /// Sample percentage of total
    pub sample_percentage: f64,
    /// Mean latency (if tracked)
    pub mean_latency_ms: Option<f64>,
    /// Mean cost (if tracked)
    pub mean_cost_usd: Option<f64>,
    /// Success rate (if tracked)
    pub success_rate: Option<f64>,
}

impl From<&crate::dispatcher::model_router::VariantSummary> for VariantSummaryFFI {
    fn from(summary: &crate::dispatcher::model_router::VariantSummary) -> Self {
        let mean_latency = summary
            .metrics
            .get(&crate::dispatcher::model_router::TrackedMetric::LatencyMs)
            .map(|m| m.mean);
        let mean_cost = summary
            .metrics
            .get(&crate::dispatcher::model_router::TrackedMetric::CostUsd)
            .map(|m| m.mean);
        let success_rate = summary
            .metrics
            .get(&crate::dispatcher::model_router::TrackedMetric::SuccessRate)
            .map(|m| m.mean);

        Self {
            id: summary.variant_id.clone(),
            name: summary.variant_name.clone(),
            sample_count: summary.sample_count,
            sample_percentage: summary.sample_percentage,
            mean_latency_ms: mean_latency,
            mean_cost_usd: mean_cost,
            success_rate,
        }
    }
}

/// Significance test result for FFI
#[derive(Debug, Clone)]
pub struct SignificanceResultFFI {
    /// Metric being compared
    pub metric_name: String,
    /// Control variant ID
    pub control_id: String,
    /// Control mean value
    pub control_mean: f64,
    /// Treatment variant ID
    pub treatment_id: String,
    /// Treatment mean value
    pub treatment_mean: f64,
    /// P-value from t-test
    pub p_value: f64,
    /// Whether result is statistically significant
    pub is_significant: bool,
    /// Relative change percentage
    pub relative_change_percent: f64,
    /// Effect size (Cohen's d)
    pub effect_size: f64,
}

impl From<&crate::dispatcher::model_router::SignificanceResult> for SignificanceResultFFI {
    fn from(result: &crate::dispatcher::model_router::SignificanceResult) -> Self {
        Self {
            metric_name: result.metric.display_name(),
            control_id: result.control_id.clone(),
            control_mean: result.control_mean,
            treatment_id: result.treatment_id.clone(),
            treatment_mean: result.treatment_mean,
            p_value: result.p_value,
            is_significant: result.is_significant,
            relative_change_percent: result.relative_change * 100.0,
            effect_size: result.cohens_d,
        }
    }
}

/// Full experiment report for FFI
#[derive(Debug, Clone)]
pub struct ExperimentReportFFI {
    /// Basic experiment info
    pub summary: ExperimentSummaryFFI,
    /// Per-variant summaries
    pub variants: Vec<VariantSummaryFFI>,
    /// Significance test results
    pub significance_tests: Vec<SignificanceResultFFI>,
    /// Automated recommendation (if any)
    pub recommendation: Option<String>,
}

impl From<&crate::dispatcher::model_router::ExperimentReport> for ExperimentReportFFI {
    fn from(report: &crate::dispatcher::model_router::ExperimentReport) -> Self {
        Self {
            summary: report.into(),
            variants: report
                .variant_summaries
                .iter()
                .map(VariantSummaryFFI::from)
                .collect(),
            significance_tests: report
                .significance_tests
                .iter()
                .map(SignificanceResultFFI::from)
                .collect(),
            recommendation: report.recommendation.clone(),
        }
    }
}

/// A/B testing overview for FFI
#[derive(Debug, Clone)]
pub struct ABTestingStatusFFI {
    /// Whether A/B testing is enabled
    pub enabled: bool,
    /// Total number of experiments
    pub total_experiments: u32,
    /// Number of active experiments
    pub active_experiments: u32,
    /// List of experiment summaries
    pub experiments: Vec<ExperimentSummaryFFI>,
    /// Status emoji for quick display
    pub status_emoji: String,
    /// Human-readable status message
    pub status_message: String,
}

impl ABTestingStatusFFI {
    /// Create a disabled A/B testing status
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            total_experiments: 0,
            active_experiments: 0,
            experiments: Vec::new(),
            status_emoji: "⚫".to_string(),
            status_message: "A/B testing disabled".to_string(),
        }
    }

    /// Create from experiment reports
    pub fn from_reports(reports: &[crate::dispatcher::model_router::ExperimentReport]) -> Self {
        if reports.is_empty() {
            return Self {
                enabled: true,
                total_experiments: 0,
                active_experiments: 0,
                experiments: Vec::new(),
                status_emoji: "⚪".to_string(),
                status_message: "No experiments configured".to_string(),
            };
        }

        let active_count = reports
            .iter()
            .filter(|r| r.status == crate::dispatcher::model_router::ExperimentStatus::Running)
            .count() as u32;

        let (emoji, message) = if active_count > 0 {
            (
                "🧪".to_string(),
                format!("{} experiment(s) running", active_count),
            )
        } else {
            ("⏸️".to_string(), "No active experiments".to_string())
        };

        Self {
            enabled: true,
            total_experiments: reports.len() as u32,
            active_experiments: active_count,
            experiments: reports.iter().map(ExperimentSummaryFFI::from).collect(),
            status_emoji: emoji,
            status_message: message,
        }
    }
}

// ============================================================================
// P3: Ensemble FFI Types
// ============================================================================

/// Ensemble mode for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsembleModeFFI {
    Disabled,
    BestOfN,
    Voting,
    Consensus,
    Cascade,
}

impl From<crate::dispatcher::model_router::EnsembleMode> for EnsembleModeFFI {
    fn from(mode: crate::dispatcher::model_router::EnsembleMode) -> Self {
        match mode {
            crate::dispatcher::model_router::EnsembleMode::Disabled => EnsembleModeFFI::Disabled,
            crate::dispatcher::model_router::EnsembleMode::BestOfN { .. } => {
                EnsembleModeFFI::BestOfN
            }
            crate::dispatcher::model_router::EnsembleMode::Voting => EnsembleModeFFI::Voting,
            crate::dispatcher::model_router::EnsembleMode::Consensus { .. } => {
                EnsembleModeFFI::Consensus
            }
            crate::dispatcher::model_router::EnsembleMode::Cascade { .. } => {
                EnsembleModeFFI::Cascade
            }
        }
    }
}

/// Quality metric for FFI
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QualityMetricFFI {
    Length,
    Structure,
    LengthAndStructure,
    ConfidenceMarkers,
    Relevance,
    Custom { name: String },
}

impl From<&crate::dispatcher::model_router::QualityMetric> for QualityMetricFFI {
    fn from(metric: &crate::dispatcher::model_router::QualityMetric) -> Self {
        match metric {
            crate::dispatcher::model_router::QualityMetric::Length => QualityMetricFFI::Length,
            crate::dispatcher::model_router::QualityMetric::Structure => {
                QualityMetricFFI::Structure
            }
            crate::dispatcher::model_router::QualityMetric::LengthAndStructure => {
                QualityMetricFFI::LengthAndStructure
            }
            crate::dispatcher::model_router::QualityMetric::ConfidenceMarkers => {
                QualityMetricFFI::ConfidenceMarkers
            }
            crate::dispatcher::model_router::QualityMetric::Relevance => {
                QualityMetricFFI::Relevance
            }
            crate::dispatcher::model_router::QualityMetric::Custom(name) => {
                QualityMetricFFI::Custom { name: name.clone() }
            }
        }
    }
}

/// Ensemble configuration summary for FFI
#[derive(Debug, Clone)]
pub struct EnsembleConfigSummaryFFI {
    /// Whether ensemble is enabled
    pub enabled: bool,
    /// Current ensemble mode
    pub mode: EnsembleModeFFI,
    /// Mode as display string
    pub mode_display: String,
    /// Models configured for ensemble
    pub models: Vec<String>,
    /// Default quality metric
    pub quality_metric: QualityMetricFFI,
    /// Default timeout in milliseconds
    pub timeout_ms: u64,
    /// Whether high complexity triggers ensemble
    pub high_complexity_enabled: bool,
    /// Complexity threshold for auto-triggering
    pub complexity_threshold: f64,
}

impl EnsembleConfigSummaryFFI {
    /// Create a disabled ensemble config summary
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            mode: EnsembleModeFFI::Disabled,
            mode_display: "Disabled".to_string(),
            models: Vec::new(),
            quality_metric: QualityMetricFFI::LengthAndStructure,
            timeout_ms: 30000,
            high_complexity_enabled: false,
            complexity_threshold: 0.8,
        }
    }
}

/// Ensemble execution statistics for FFI
#[derive(Debug, Clone)]
pub struct EnsembleStatsFFI {
    /// Total ensemble executions
    pub total_executions: u64,
    /// Number of successful executions
    pub successful_executions: u64,
    /// Average latency in milliseconds
    pub avg_latency_ms: f64,
    /// Average cost in USD
    pub avg_cost_usd: f64,
    /// Average confidence score
    pub avg_confidence: f64,
    /// Number of high consensus results
    pub high_consensus_count: u64,
    /// Number of low consensus results
    pub low_consensus_count: u64,
    /// Most used aggregation method
    pub most_used_method: String,
}

impl EnsembleStatsFFI {
    /// Create empty ensemble stats
    pub fn empty() -> Self {
        Self {
            total_executions: 0,
            successful_executions: 0,
            avg_latency_ms: 0.0,
            avg_cost_usd: 0.0,
            avg_confidence: 0.0,
            high_consensus_count: 0,
            low_consensus_count: 0,
            most_used_method: "none".to_string(),
        }
    }
}

/// Overall ensemble status for FFI
#[derive(Debug, Clone)]
pub struct EnsembleStatusFFI {
    /// Configuration summary
    pub config: EnsembleConfigSummaryFFI,
    /// Execution statistics
    pub stats: EnsembleStatsFFI,
    /// Status emoji for quick display
    pub status_emoji: String,
    /// Human-readable status message
    pub status_message: String,
}

impl EnsembleStatusFFI {
    /// Create a disabled ensemble status
    pub fn disabled() -> Self {
        Self {
            config: EnsembleConfigSummaryFFI::disabled(),
            stats: EnsembleStatsFFI::empty(),
            status_emoji: "⚫".to_string(),
            status_message: "Ensemble disabled".to_string(),
        }
    }
}
