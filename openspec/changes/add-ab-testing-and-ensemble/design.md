# Design: A/B Testing Framework and Multi-Model Ensemble

## Overview

This document details the architectural design for P3 Model Router enhancements: A/B Testing Framework and Multi-Model Ensemble. These features enable data-driven routing optimization and improved response reliability through model diversity.

## Architecture

### System Context

```
┌────────────────────────────────────────────────────────────────────────────────┐
│                          Aether Model Router Evolution                          │
├────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  P0 (Foundation)          P1 (Resilience)         P2 (Intelligence)            │
│  ┌─────────────────┐      ┌─────────────────┐      ┌─────────────────┐         │
│  │ ModelMatcher    │      │ RetryOrchestrator│     │ PromptAnalyzer  │         │
│  │ ModelProfiles   │      │ FailoverChain   │      │ SemanticCache   │         │
│  │ MetricsCollector│      │ BudgetManager   │      │ P2Router        │         │
│  │ HealthMonitor   │      │ OrchestratedRtr │      │                 │         │
│  └─────────────────┘      └─────────────────┘      └─────────────────┘         │
│           │                       │                        │                    │
│           └───────────────────────┼────────────────────────┘                    │
│                                   ↓                                             │
│                         P3 (Experimentation & Reliability)                      │
│                         ┌─────────────────────────────────┐                     │
│                         │  ABTestingEngine                │                     │
│                         │  EnsembleEngine                 │                     │
│                         │  P3IntelligentRouter            │                     │
│                         └─────────────────────────────────┘                     │
│                                                                                 │
└────────────────────────────────────────────────────────────────────────────────┘
```

### Module Structure

```
core/src/dispatcher/model_router/
├── mod.rs                      # Module exports (add ab_testing, ensemble, p3_router)
├── ab_testing.rs               # NEW: A/B testing engine
├── ensemble.rs                 # NEW: Multi-model ensemble engine
├── p3_router.rs                # NEW: P3 intelligent router (integrates all)
└── ... (existing P0/P1/P2 modules)
```

---

## Component Design

### 1. A/B Testing Engine

#### 1.1 Core Types

```rust
/// Unique identifier for an experiment
pub type ExperimentId = String;

/// Unique identifier for a variant within an experiment
pub type VariantId = String;

/// Configuration for a single A/B experiment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentConfig {
    /// Unique experiment identifier
    pub id: ExperimentId,
    /// Human-readable name
    pub name: String,
    /// Whether the experiment is currently running
    pub enabled: bool,
    /// Percentage of total traffic to include (0-100)
    pub traffic_percentage: u8,
    /// Variants with their configurations
    pub variants: Vec<VariantConfig>,
    /// Optional filter: only include specific TaskIntent
    pub target_intent: Option<TaskIntent>,
    /// Optional filter: minimum complexity score
    pub min_complexity: Option<f64>,
    /// Metrics to track for analysis
    pub tracked_metrics: Vec<TrackedMetric>,
    /// Start time (optional, defaults to now)
    pub start_time: Option<SystemTime>,
    /// End time (optional, runs indefinitely if None)
    pub end_time: Option<SystemTime>,
}

/// Configuration for a single variant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantConfig {
    /// Variant identifier
    pub id: VariantId,
    /// Display name
    pub name: String,
    /// Weight for traffic allocation (relative to other variants)
    pub weight: u32,
    /// Routing override: specific model to use
    pub model_override: Option<String>,
    /// Routing override: cost strategy
    pub cost_strategy_override: Option<CostStrategy>,
    /// Custom parameters (JSON blob for flexibility)
    pub parameters: Option<serde_json::Value>,
}

/// Metrics that can be tracked per variant
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TrackedMetric {
    LatencyMs,
    CostUsd,
    InputTokens,
    OutputTokens,
    SuccessRate,
    CacheHitRate,
    RetryCount,
    UserRating,  // Requires external feedback
    Custom(String),
}

/// Result of variant assignment
#[derive(Debug, Clone)]
pub struct VariantAssignment {
    pub experiment_id: ExperimentId,
    pub experiment_name: String,
    pub variant_id: VariantId,
    pub variant_name: String,
    pub model_override: Option<String>,
    pub cost_strategy_override: Option<CostStrategy>,
    pub is_control: bool,
}
```

#### 1.2 Traffic Splitting

```rust
/// Strategy for assigning traffic to experiments/variants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AssignmentStrategy {
    /// Hash user_id for consistent assignment
    UserId,
    /// Hash session_id (new assignment per session)
    SessionId,
    /// Random per request (no consistency)
    RequestId,
    /// Feature-based: use prompt features for assignment
    FeatureBased { feature: String },
}

/// Traffic split manager using consistent hashing
pub struct TrafficSplitManager {
    /// Active experiments indexed by ID
    experiments: HashMap<ExperimentId, ExperimentConfig>,
    /// Assignment strategy
    strategy: AssignmentStrategy,
    /// Hash seed for reproducibility
    hash_seed: u64,
}

impl TrafficSplitManager {
    /// Assign a request to an experiment variant (or None if not in experiment)
    pub fn assign(
        &self,
        user_id: Option<&str>,
        session_id: Option<&str>,
        request_id: &str,
        intent: &TaskIntent,
        features: Option<&PromptFeatures>,
    ) -> Option<VariantAssignment> {
        // 1. Filter experiments by enabled status and time window
        // 2. Filter by target_intent if specified
        // 3. Filter by min_complexity if specified
        // 4. Compute hash of assignment key (user_id, session_id, or request_id)
        // 5. Check if hash falls within traffic_percentage
        // 6. If in experiment, assign to variant based on weighted distribution
        // 7. Return VariantAssignment or None
    }

    /// Consistent hash function (SipHash for security + determinism)
    fn compute_hash(&self, key: &str, experiment_id: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        use siphasher::sip::SipHasher24;

        let mut hasher = SipHasher24::new_with_keys(self.hash_seed, 0);
        key.hash(&mut hasher);
        experiment_id.hash(&mut hasher);
        hasher.finish()
    }
}
```

#### 1.3 Outcome Tracking

```rust
/// Single outcome record for an experiment
#[derive(Debug, Clone)]
pub struct ExperimentOutcome {
    pub experiment_id: ExperimentId,
    pub variant_id: VariantId,
    pub timestamp: SystemTime,
    pub metrics: HashMap<TrackedMetric, f64>,
    pub request_id: String,
    pub model_used: String,
}

/// Aggregated statistics per variant
#[derive(Debug, Clone, Default)]
pub struct VariantStats {
    pub sample_count: u64,
    pub metrics: HashMap<TrackedMetric, MetricStats>,
}

#[derive(Debug, Clone, Default)]
pub struct MetricStats {
    pub count: u64,
    pub sum: f64,
    pub sum_sq: f64,  // For variance calculation
    pub min: f64,
    pub max: f64,
}

impl MetricStats {
    pub fn mean(&self) -> f64 { self.sum / self.count as f64 }
    pub fn variance(&self) -> f64 {
        (self.sum_sq / self.count as f64) - self.mean().powi(2)
    }
    pub fn std_dev(&self) -> f64 { self.variance().sqrt() }
}

/// Outcome tracker with thread-safe storage
pub struct OutcomeTracker {
    /// Per-experiment, per-variant statistics
    stats: RwLock<HashMap<ExperimentId, HashMap<VariantId, VariantStats>>>,
    /// Raw outcomes for detailed analysis (bounded buffer)
    raw_outcomes: RwLock<VecDeque<ExperimentOutcome>>,
    /// Maximum raw outcomes to retain
    max_raw_outcomes: usize,
}

impl OutcomeTracker {
    pub fn record(&self, outcome: ExperimentOutcome) {
        // Update aggregated stats
        // Add to raw outcomes (with eviction)
    }

    pub fn get_stats(&self, experiment_id: &str) -> Option<HashMap<VariantId, VariantStats>> {
        // Return clone of stats for experiment
    }
}
```

#### 1.4 Statistical Analysis

```rust
/// Result of significance test between two variants
#[derive(Debug, Clone)]
pub struct SignificanceResult {
    /// The metric being compared
    pub metric: TrackedMetric,
    /// Control variant stats
    pub control_mean: f64,
    pub control_std_dev: f64,
    pub control_n: u64,
    /// Treatment variant stats
    pub treatment_mean: f64,
    pub treatment_std_dev: f64,
    pub treatment_n: u64,
    /// Statistical test results
    pub t_statistic: f64,
    pub p_value: f64,
    pub is_significant: bool,  // p < 0.05
    /// Effect size
    pub relative_change: f64,  // (treatment - control) / control
    pub cohens_d: f64,         // Standardized effect size
}

pub struct SignificanceCalculator;

impl SignificanceCalculator {
    /// Two-sample t-test for comparing means
    pub fn t_test(
        control: &MetricStats,
        treatment: &MetricStats,
    ) -> SignificanceResult {
        // Welch's t-test (unequal variances assumed)
        let t = (treatment.mean() - control.mean())
            / ((treatment.variance() / treatment.count as f64)
               + (control.variance() / control.count as f64)).sqrt();

        // Degrees of freedom (Welch-Satterthwaite)
        let df = welch_df(control, treatment);

        // P-value from t-distribution (two-tailed)
        let p_value = 2.0 * (1.0 - t_cdf(t.abs(), df));

        SignificanceResult {
            metric: TrackedMetric::Custom("unknown".into()),
            control_mean: control.mean(),
            control_std_dev: control.std_dev(),
            control_n: control.count,
            treatment_mean: treatment.mean(),
            treatment_std_dev: treatment.std_dev(),
            treatment_n: treatment.count,
            t_statistic: t,
            p_value,
            is_significant: p_value < 0.05,
            relative_change: (treatment.mean() - control.mean()) / control.mean(),
            cohens_d: cohens_d(control, treatment),
        }
    }
}
```

#### 1.5 Experiment Report

```rust
/// Human-readable experiment report
#[derive(Debug, Clone, Serialize)]
pub struct ExperimentReport {
    pub experiment_id: ExperimentId,
    pub experiment_name: String,
    pub status: ExperimentStatus,
    pub duration: Duration,
    pub total_samples: u64,
    pub variant_summaries: Vec<VariantSummary>,
    pub significance_tests: Vec<SignificanceResult>,
    pub recommendation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub enum ExperimentStatus {
    Running,
    Paused,
    Completed,
    InsufficientData,
}

#[derive(Debug, Clone, Serialize)]
pub struct VariantSummary {
    pub variant_id: VariantId,
    pub variant_name: String,
    pub sample_count: u64,
    pub sample_percentage: f64,
    pub metrics: HashMap<TrackedMetric, MetricSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricSummary {
    pub mean: f64,
    pub std_dev: f64,
    pub median: Option<f64>,  // Requires raw data
    pub p95: Option<f64>,
}
```

---

### 2. Multi-Model Ensemble Engine

#### 2.1 Core Types

```rust
/// Ensemble execution strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EnsembleMode {
    /// Disabled - single model routing
    Disabled,
    /// Run N models, return best response by quality score
    BestOfN { n: usize },
    /// Run all models, aggregate by voting
    Voting,
    /// Run all models, require consensus
    Consensus { min_agreement: f64 },
    /// Run models in priority order until quality threshold met
    Cascade { quality_threshold: f64 },
}

/// Configuration for ensemble execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleConfig {
    /// Ensemble mode
    pub mode: EnsembleMode,
    /// Models to include in ensemble (profile IDs)
    pub models: Vec<String>,
    /// Maximum wait time for all models
    pub timeout_ms: u64,
    /// Quality scoring method
    pub quality_metric: QualityMetric,
    /// Whether to use budget-aware model selection
    pub budget_aware: bool,
    /// Maximum cost multiplier (e.g., 3.0 = max 3x single model cost)
    pub max_cost_multiplier: f64,
}

/// Quality scoring method for response evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QualityMetric {
    /// Response length (longer often better for explanations)
    Length,
    /// Structured response detection (code blocks, lists, etc.)
    Structure,
    /// Combined length and structure
    LengthAndStructure,
    /// Confidence markers in response ("I'm confident", etc.)
    ConfidenceMarkers,
    /// Semantic similarity to prompt (relevance)
    Relevance,
    /// Custom scoring function name
    Custom(String),
}

/// Ensemble task-to-config mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleStrategy {
    /// Default mode when no specific strategy matches
    pub default: EnsembleMode,
    /// Per-intent strategies
    pub intent_strategies: HashMap<TaskIntent, EnsembleConfig>,
    /// Per-complexity threshold strategies
    pub complexity_threshold: Option<f64>,
    pub high_complexity_config: Option<EnsembleConfig>,
}
```

#### 2.2 Parallel Executor

```rust
/// Manages parallel execution of multiple models
pub struct ParallelExecutor {
    /// Timeout for entire ensemble
    timeout: Duration,
    /// Maximum concurrent requests
    max_concurrency: usize,
}

/// Single model execution result
#[derive(Debug, Clone)]
pub struct ModelExecutionResult {
    pub model_id: String,
    pub response: Option<String>,
    pub error: Option<String>,
    pub latency_ms: u64,
    pub tokens_used: TokenUsage,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl ParallelExecutor {
    /// Execute request across multiple models concurrently
    pub async fn execute_parallel<F, Fut>(
        &self,
        models: &[ModelProfile],
        request: &ExecutionRequest,
        executor_fn: F,
    ) -> Vec<ModelExecutionResult>
    where
        F: Fn(&ModelProfile, &ExecutionRequest) -> Fut,
        Fut: Future<Output = Result<String, ExecutionError>>,
    {
        use tokio::time::timeout;
        use futures::future::join_all;

        let futures: Vec<_> = models.iter()
            .take(self.max_concurrency)
            .map(|model| {
                let start = Instant::now();
                async move {
                    match timeout(self.timeout, executor_fn(model, request)).await {
                        Ok(Ok(response)) => ModelExecutionResult {
                            model_id: model.id.clone(),
                            response: Some(response),
                            error: None,
                            latency_ms: start.elapsed().as_millis() as u64,
                            tokens_used: TokenUsage::default(),  // Filled by callback
                            cost_usd: 0.0,
                        },
                        Ok(Err(e)) => ModelExecutionResult {
                            model_id: model.id.clone(),
                            response: None,
                            error: Some(e.to_string()),
                            latency_ms: start.elapsed().as_millis() as u64,
                            tokens_used: TokenUsage::default(),
                            cost_usd: 0.0,
                        },
                        Err(_timeout) => ModelExecutionResult {
                            model_id: model.id.clone(),
                            response: None,
                            error: Some("Timeout".to_string()),
                            latency_ms: self.timeout.as_millis() as u64,
                            tokens_used: TokenUsage::default(),
                            cost_usd: 0.0,
                        },
                    }
                }
            })
            .collect();

        join_all(futures).await
    }
}
```

#### 2.3 Response Aggregator

```rust
/// Aggregates multiple model responses into a single result
pub struct ResponseAggregator {
    quality_scorer: Box<dyn QualityScorer>,
    consensus_threshold: f64,
}

/// Quality scoring trait
pub trait QualityScorer: Send + Sync {
    fn score(&self, response: &str, prompt: &str) -> f64;
}

/// Built-in quality scorers
pub struct LengthAndStructureScorer;

impl QualityScorer for LengthAndStructureScorer {
    fn score(&self, response: &str, _prompt: &str) -> f64 {
        let length_score = (response.len() as f64 / 1000.0).min(1.0);

        let structure_score = {
            let has_code_blocks = response.contains("```");
            let has_lists = response.contains("\n- ") || response.contains("\n* ");
            let has_headers = response.contains("\n## ") || response.contains("\n### ");
            let has_paragraphs = response.matches("\n\n").count() >= 2;

            let mut score = 0.0;
            if has_code_blocks { score += 0.3; }
            if has_lists { score += 0.25; }
            if has_headers { score += 0.25; }
            if has_paragraphs { score += 0.2; }
            score.min(1.0)
        };

        // Weighted combination
        0.4 * length_score + 0.6 * structure_score
    }
}

/// Ensemble result after aggregation
#[derive(Debug, Clone)]
pub struct EnsembleResult {
    /// Final aggregated response
    pub response: String,
    /// Model that produced the selected response
    pub selected_model: String,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// All model results
    pub all_results: Vec<ModelExecutionResult>,
    /// Aggregation method used
    pub aggregation_method: String,
    /// Total cost of ensemble
    pub total_cost_usd: f64,
    /// Total latency (wall clock, not sum)
    pub total_latency_ms: u64,
    /// Consensus level if applicable
    pub consensus_level: Option<f64>,
}

impl ResponseAggregator {
    /// Aggregate results using BestOfN strategy
    pub fn best_of_n(&self, results: Vec<ModelExecutionResult>, prompt: &str) -> EnsembleResult {
        let successful: Vec<_> = results.iter()
            .filter(|r| r.response.is_some())
            .collect();

        if successful.is_empty() {
            return self.fallback_error(results);
        }

        // Score each response
        let scored: Vec<_> = successful.iter()
            .map(|r| {
                let score = self.quality_scorer.score(r.response.as_ref().unwrap(), prompt);
                (r, score)
            })
            .collect();

        // Select best
        let (best, best_score) = scored.iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();

        EnsembleResult {
            response: best.response.clone().unwrap(),
            selected_model: best.model_id.clone(),
            confidence: *best_score,
            all_results: results,
            aggregation_method: "best_of_n".to_string(),
            total_cost_usd: successful.iter().map(|r| r.cost_usd).sum(),
            total_latency_ms: successful.iter().map(|r| r.latency_ms).max().unwrap_or(0),
            consensus_level: None,
        }
    }

    /// Aggregate results using consensus detection
    pub fn consensus(&self, results: Vec<ModelExecutionResult>, prompt: &str) -> EnsembleResult {
        let successful: Vec<_> = results.iter()
            .filter(|r| r.response.is_some())
            .collect();

        if successful.len() < 2 {
            return self.best_of_n(results, prompt);
        }

        // Calculate pairwise similarity
        let similarities = self.calculate_similarities(&successful);
        let consensus_level = similarities.iter().sum::<f64>() / similarities.len() as f64;

        // If consensus is below threshold, select best individual response
        let mut result = self.best_of_n(results, prompt);
        result.consensus_level = Some(consensus_level);
        result.aggregation_method = "consensus".to_string();

        if consensus_level < self.consensus_threshold {
            result.confidence *= 0.7;  // Reduce confidence for low consensus
        }

        result
    }

    /// Calculate semantic similarity between responses
    fn calculate_similarities(&self, results: &[&ModelExecutionResult]) -> Vec<f64> {
        // Simple: Jaccard similarity of word sets
        // Advanced: Use embeddings if available
        let mut similarities = Vec::new();

        for i in 0..results.len() {
            for j in (i + 1)..results.len() {
                let r1 = results[i].response.as_ref().unwrap();
                let r2 = results[j].response.as_ref().unwrap();
                similarities.push(jaccard_similarity(r1, r2));
            }
        }

        similarities
    }
}

fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let words_a: HashSet<_> = a.split_whitespace().collect();
    let words_b: HashSet<_> = b.split_whitespace().collect();

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 { 0.0 } else { intersection as f64 / union as f64 }
}
```

---

### 3. P3 Intelligent Router

#### 3.1 Integration Layer

```rust
/// P3 Router integrating A/B testing and ensemble capabilities
pub struct P3IntelligentRouter {
    /// P2 router (includes prompt analysis, semantic cache)
    p2_router: Arc<P2IntelligentRouter>,
    /// A/B testing engine
    ab_engine: Option<Arc<ABTestingEngine>>,
    /// Ensemble engine
    ensemble_engine: Option<Arc<EnsembleEngine>>,
    /// Configuration
    config: P3RouterConfig,
}

#[derive(Debug, Clone)]
pub struct P3RouterConfig {
    pub ab_testing_enabled: bool,
    pub ensemble_enabled: bool,
    pub default_user_id_header: Option<String>,
}

/// Extended routing decision with P3 metadata
#[derive(Debug, Clone)]
pub struct P3RoutingDecision {
    /// Base routing decision
    pub base_decision: RoutingDecision,
    /// A/B experiment assignment (if any)
    pub experiment_assignment: Option<VariantAssignment>,
    /// Ensemble result (if ensemble was used)
    pub ensemble_result: Option<EnsembleResult>,
    /// Whether response came from cache
    pub cached: bool,
    /// Total decision latency
    pub decision_latency_ms: u64,
}

impl P3IntelligentRouter {
    /// Main routing method
    pub async fn route(&self, request: &RoutingRequest) -> Result<P3RoutingDecision, RoutingError> {
        let start = Instant::now();

        // 1. Check semantic cache (via P2)
        if let Some(cached) = self.p2_router.check_cache(&request.prompt).await? {
            return Ok(P3RoutingDecision {
                base_decision: cached,
                experiment_assignment: None,
                ensemble_result: None,
                cached: true,
                decision_latency_ms: start.elapsed().as_millis() as u64,
            });
        }

        // 2. Analyze prompt (via P2)
        let features = self.p2_router.analyze_prompt(&request.prompt)?;

        // 3. Check A/B experiment assignment
        let experiment_assignment = if let Some(ab_engine) = &self.ab_engine {
            ab_engine.assign(
                request.user_id.as_deref(),
                request.session_id.as_deref(),
                &request.request_id,
                &request.intent,
                Some(&features),
            )
        } else {
            None
        };

        // 4. Determine model selection (with experiment override)
        let selected_model = if let Some(ref assignment) = experiment_assignment {
            if let Some(ref model_override) = assignment.model_override {
                self.p2_router.get_profile(model_override)
                    .ok_or(RoutingError::ModelNotFound(model_override.clone()))?
            } else {
                self.p2_router.route_by_features(&features, &request.intent)?
            }
        } else {
            self.p2_router.route_by_features(&features, &request.intent)?
        };

        // 5. Check if ensemble should be used
        let should_ensemble = self.should_use_ensemble(&request.intent, &features);

        // 6. Execute (single model or ensemble)
        let (response, ensemble_result) = if should_ensemble {
            let ensemble = self.ensemble_engine.as_ref().unwrap();
            let result = ensemble.execute(&request.prompt, &request.intent, &features).await?;
            (result.response.clone(), Some(result))
        } else {
            let response = self.p2_router.execute(&selected_model, &request.prompt).await?;
            (response, None)
        };

        // 7. Cache response (via P2)
        self.p2_router.cache_response(&request.prompt, &response, &selected_model.id).await?;

        // 8. Record experiment outcome if in experiment
        if let Some(ref assignment) = experiment_assignment {
            if let Some(ab_engine) = &self.ab_engine {
                ab_engine.record_outcome(ExperimentOutcome {
                    experiment_id: assignment.experiment_id.clone(),
                    variant_id: assignment.variant_id.clone(),
                    timestamp: SystemTime::now(),
                    metrics: self.collect_metrics(&request, &response, &ensemble_result),
                    request_id: request.request_id.clone(),
                    model_used: selected_model.id.clone(),
                });
            }
        }

        Ok(P3RoutingDecision {
            base_decision: RoutingDecision {
                model: selected_model,
                response: Some(response),
                ..Default::default()
            },
            experiment_assignment,
            ensemble_result,
            cached: false,
            decision_latency_ms: start.elapsed().as_millis() as u64,
        })
    }

    fn should_use_ensemble(&self, intent: &TaskIntent, features: &PromptFeatures) -> bool {
        if !self.config.ensemble_enabled || self.ensemble_engine.is_none() {
            return false;
        }

        let ensemble = self.ensemble_engine.as_ref().unwrap();
        let strategy = ensemble.get_strategy();

        // Check intent-specific strategy
        if strategy.intent_strategies.contains_key(intent) {
            return true;
        }

        // Check complexity threshold
        if let Some(threshold) = strategy.complexity_threshold {
            if features.complexity_score >= threshold && strategy.high_complexity_config.is_some() {
                return true;
            }
        }

        // Check default
        !matches!(strategy.default, EnsembleMode::Disabled)
    }
}
```

---

## Configuration Schema

### TOML Configuration

```toml
[cowork.model_routing.ab_testing]
# Whether A/B testing is enabled
enabled = true
# How to assign traffic: "user_id" | "session_id" | "request_id"
assignment_strategy = "user_id"
# Maximum number of concurrent experiments
max_experiments = 10
# Maximum raw outcomes to retain for analysis
max_raw_outcomes = 100000

# Define experiments
[[cowork.model_routing.ab_testing.experiments]]
id = "gemini-reasoning-test"
name = "Gemini vs Claude for Reasoning Tasks"
enabled = true
traffic_percentage = 10
target_intent = "Reasoning"
tracked_metrics = ["latency_ms", "cost_usd", "output_tokens"]

[[cowork.model_routing.ab_testing.experiments.variants]]
id = "control"
name = "Claude (Control)"
weight = 50
model_override = "claude-sonnet"

[[cowork.model_routing.ab_testing.experiments.variants]]
id = "treatment"
name = "Gemini (Treatment)"
weight = 50
model_override = "gemini-pro"

# Another experiment
[[cowork.model_routing.ab_testing.experiments]]
id = "cost-strategy-test"
name = "Balanced vs Cheapest Strategy"
enabled = true
traffic_percentage = 20
tracked_metrics = ["latency_ms", "cost_usd", "success_rate"]

[[cowork.model_routing.ab_testing.experiments.variants]]
id = "control"
name = "Balanced (Control)"
weight = 50
cost_strategy_override = "balanced"

[[cowork.model_routing.ab_testing.experiments.variants]]
id = "treatment"
name = "Cheapest (Treatment)"
weight = 50
cost_strategy_override = "cheapest"

[cowork.model_routing.ensemble]
# Whether ensemble is enabled
enabled = true
# Default mode when no specific strategy matches
default_mode = "disabled"
# Maximum cost multiplier (e.g., 3.0 = max 3x single model cost)
max_cost_multiplier = 3.0
# Default timeout for ensemble execution
default_timeout_ms = 30000

# Per-intent ensemble strategies
[cowork.model_routing.ensemble.strategies.reasoning]
mode = "best_of_n"
n = 2
models = ["claude-opus", "gpt-4o"]
timeout_ms = 30000
quality_metric = "length_and_structure"

[cowork.model_routing.ensemble.strategies.code_generation]
mode = "consensus"
models = ["claude-sonnet", "gpt-4o", "gemini-pro"]
timeout_ms = 20000
min_agreement = 0.7

# Complexity-based ensemble
[cowork.model_routing.ensemble.high_complexity]
complexity_threshold = 0.8
mode = "best_of_n"
n = 2
models = ["claude-opus", "gemini-pro"]
```

### Configuration Types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ABTestingConfigToml {
    pub enabled: bool,
    pub assignment_strategy: String,
    pub max_experiments: usize,
    pub max_raw_outcomes: usize,
    pub experiments: Vec<ExperimentConfigToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentConfigToml {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub traffic_percentage: u8,
    pub target_intent: Option<String>,
    pub min_complexity: Option<f64>,
    pub tracked_metrics: Vec<String>,
    pub variants: Vec<VariantConfigToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantConfigToml {
    pub id: String,
    pub name: String,
    pub weight: u32,
    pub model_override: Option<String>,
    pub cost_strategy_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleConfigToml {
    pub enabled: bool,
    pub default_mode: String,
    pub max_cost_multiplier: f64,
    pub default_timeout_ms: u64,
    pub strategies: HashMap<String, EnsembleStrategyToml>,
    pub high_complexity: Option<HighComplexityEnsembleToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnsembleStrategyToml {
    pub mode: String,
    pub n: Option<usize>,
    pub models: Vec<String>,
    pub timeout_ms: u64,
    pub quality_metric: Option<String>,
    pub min_agreement: Option<f64>,
}
```

---

## Data Flow

### A/B Testing Flow

```
Request arrives
      │
      ▼
┌─────────────────────┐
│ 1. Check experiments│
│    - Is enabled?    │
│    - Time window ok?│
│    - Intent matches?│
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│ 2. Traffic sampling │
│    hash(user_id) %  │
│    100 < traffic%?  │
└─────────┬───────────┘
          │
     ┌────┴────┐
     │ In exp? │
     └────┬────┘
   No     │     Yes
    │     │      │
    │     │      ▼
    │     │  ┌───────────────┐
    │     │  │ 3. Variant    │
    │     │  │    assignment │
    │     │  │    (weighted) │
    │     │  └───────┬───────┘
    │     │          │
    ▼     ▼          ▼
┌─────────────────────────┐
│ 4. Apply variant config │
│    - Model override     │
│    - Strategy override  │
└─────────────────────────┘
          │
          ▼
┌─────────────────────────┐
│ 5. Execute request      │
└─────────────────────────┘
          │
          ▼
┌─────────────────────────┐
│ 6. Record outcome       │
│    - Metrics collected  │
│    - Stats updated      │
└─────────────────────────┘
```

### Ensemble Flow

```
Request arrives (high complexity or specific intent)
      │
      ▼
┌─────────────────────┐
│ 1. Select models    │
│    - From strategy  │
│    - Budget check   │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│ 2. Parallel execute │
│    - tokio::join!   │
│    - With timeout   │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│ 3. Collect results  │
│    - Filter success │
│    - Score quality  │
└─────────┬───────────┘
          │
          ▼
┌─────────────────────────────┐
│ 4. Aggregate                │
│    BestOfN: select highest  │
│    Voting: majority         │
│    Consensus: check agree   │
└─────────┬───────────────────┘
          │
          ▼
┌─────────────────────┐
│ 5. Return result    │
│    + Confidence     │
│    + Model source   │
└─────────────────────┘
```

---

## FFI Exports

```rust
// AB Testing exports
#[uniffi::export]
pub fn get_active_experiments() -> Vec<ExperimentSummary>;

#[uniffi::export]
pub fn get_experiment_stats(experiment_id: &str) -> Option<ExperimentReport>;

#[uniffi::export]
pub fn enable_experiment(experiment_id: &str) -> Result<(), String>;

#[uniffi::export]
pub fn disable_experiment(experiment_id: &str) -> Result<(), String>;

#[uniffi::export]
pub fn get_user_experiment_assignment(user_id: &str) -> Vec<VariantAssignment>;

// Ensemble exports
#[uniffi::export]
pub fn get_ensemble_config() -> EnsembleConfigSummary;

#[uniffi::export]
pub fn get_ensemble_stats() -> EnsembleSummaryStats;

#[derive(uniffi::Record)]
pub struct ExperimentSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub traffic_percentage: u8,
    pub variant_count: u32,
    pub total_samples: u64,
    pub start_time: Option<i64>,
}

#[derive(uniffi::Record)]
pub struct EnsembleSummaryStats {
    pub total_ensemble_calls: u64,
    pub avg_models_per_call: f64,
    pub avg_cost_multiplier: f64,
    pub consensus_rate: f64,
}
```

---

## Trade-offs and Decisions

### A/B Testing

| Decision | Chosen Approach | Alternative | Rationale |
|----------|-----------------|-------------|-----------|
| Hashing algorithm | SipHash | MurmurHash | Security + determinism |
| Stats storage | In-memory | SQLite | Simplicity, sufficient for typical use |
| Significance test | Welch's t-test | Z-test, Mann-Whitney | Handles unequal variances |
| Min sample size | 30 per variant | Power calculation | Good balance of speed vs accuracy |

### Ensemble

| Decision | Chosen Approach | Alternative | Rationale |
|----------|-----------------|-------------|-----------|
| Parallelism | tokio::join_all | Sequential | Latency optimization |
| Quality scoring | Heuristic (length+structure) | Model-based | No extra API calls |
| Consensus | Word Jaccard | Semantic embeddings | Fast, sufficient for most cases |
| Timeout | Per-ensemble | Per-model | Simpler to reason about |

---

## Testing Strategy

### Unit Tests

- `ab_testing.rs`: Hash consistency, variant assignment, stats aggregation
- `ensemble.rs`: Quality scoring, response aggregation, consensus detection
- `p3_router.rs`: Integration flow, cache interaction, experiment recording

### Integration Tests

- End-to-end A/B routing with mock executor
- Ensemble execution with simulated model responses
- Configuration loading and validation
- P2→P3 upgrade path (ensure P2 still works)

### Performance Tests

- A/B assignment: <1ms for 100 experiments
- Ensemble overhead: Only parallel execution time
- Memory: <50MB for 10 experiments with 100K outcomes each
