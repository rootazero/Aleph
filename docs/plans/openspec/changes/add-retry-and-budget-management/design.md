# Design: Intelligent Retry/Failover and Budget Management

## Overview

This document describes the technical design for two interconnected P1 features:
1. **RetryOrchestrator** - Resilient request execution with automatic retry and failover
2. **BudgetManager** - Cost control with real-time tracking and enforcement

Both components integrate with the existing P0 infrastructure (HealthManager, MetricsCollector, ModelMatcher).

---

## 1. Retry Orchestrator

### 1.1 Architecture

```
                                    ┌─────────────────────┐
                                    │  RetryOrchestrator  │
                                    └──────────┬──────────┘
                                               │
                    ┌──────────────────────────┼──────────────────────────┐
                    │                          │                          │
           ┌────────▼────────┐       ┌─────────▼─────────┐      ┌────────▼────────┐
           │   RetryPolicy   │       │  FailoverChain    │      │ BackoffStrategy │
           │                 │       │                   │      │                 │
           │ • max_attempts  │       │ • primary_model   │      │ • exponential   │
           │ • timeout/att.  │       │ • alternatives[]  │      │ • jitter        │
           │ • retryable[]   │       │ • selection_mode  │      │ • rate_limit    │
           └─────────────────┘       └───────────────────┘      └─────────────────┘
```

### 1.2 Core Types

```rust
/// Configuration for retry behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of attempts (including initial)
    pub max_attempts: u32,

    /// Timeout for each individual attempt
    pub attempt_timeout: Duration,

    /// Total timeout across all attempts
    pub total_timeout: Option<Duration>,

    /// Error types that trigger retry
    pub retryable_outcomes: Vec<CallOutcome>,

    /// Whether to use failover on non-retryable errors
    pub failover_on_non_retryable: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            attempt_timeout: Duration::from_secs(30),
            total_timeout: Some(Duration::from_secs(90)),
            retryable_outcomes: vec![
                CallOutcome::Timeout,
                CallOutcome::RateLimited,
                CallOutcome::NetworkError,
            ],
            failover_on_non_retryable: true,
        }
    }
}

/// Backoff calculation strategy
#[derive(Debug, Clone)]
pub enum BackoffStrategy {
    /// Fixed delay between attempts
    Constant { delay: Duration },

    /// Exponential backoff: base * 2^attempt
    Exponential {
        initial: Duration,
        max: Duration,
        multiplier: f64,
    },

    /// Exponential with random jitter
    ExponentialJitter {
        initial: Duration,
        max: Duration,
        jitter_factor: f64, // 0.0 - 1.0
    },

    /// Respect Retry-After header from rate limits
    RateLimitAware {
        fallback: Box<BackoffStrategy>,
    },
}

impl BackoffStrategy {
    /// Calculate delay for given attempt number (0-indexed)
    pub fn delay_for_attempt(&self, attempt: u32, rate_limit_hint: Option<Duration>) -> Duration {
        match self {
            Self::Constant { delay } => *delay,

            Self::Exponential { initial, max, multiplier } => {
                let delay = initial.mul_f64(multiplier.powi(attempt as i32));
                delay.min(*max)
            }

            Self::ExponentialJitter { initial, max, jitter_factor } => {
                let base = initial.mul_f64(2.0_f64.powi(attempt as i32));
                let jitter = base.mul_f64(rand::random::<f64>() * jitter_factor);
                (base + jitter).min(*max)
            }

            Self::RateLimitAware { fallback } => {
                rate_limit_hint.unwrap_or_else(|| fallback.delay_for_attempt(attempt, None))
            }
        }
    }
}
```

### 1.3 Failover Chain

```rust
/// Strategy for selecting failover models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailoverSelectionMode {
    /// Use models in configured order
    Ordered,

    /// Prefer healthiest model
    HealthPriority,

    /// Prefer cheapest healthy model
    CostPriority,

    /// Balance health and cost
    Balanced { health_weight: f64, cost_weight: f64 },
}

/// Chain of models for failover
#[derive(Debug, Clone)]
pub struct FailoverChain {
    /// Primary model ID
    pub primary: String,

    /// Alternative models in preference order
    pub alternatives: Vec<String>,

    /// How to select from alternatives
    pub selection_mode: FailoverSelectionMode,

    /// Capability requirements that alternatives must satisfy
    pub required_capabilities: Vec<Capability>,
}

impl FailoverChain {
    /// Build from ModelMatcher using same capabilities as primary
    pub fn from_matcher(
        matcher: &ModelMatcher,
        primary: &str,
        selection_mode: FailoverSelectionMode,
    ) -> Self {
        let profile = matcher.get_profile(primary);
        let capabilities = profile.map(|p| p.capabilities.clone()).unwrap_or_default();

        // Find alternatives with overlapping capabilities
        let alternatives = matcher
            .profiles()
            .filter(|p| p.id != primary)
            .filter(|p| capabilities.iter().any(|c| p.capabilities.contains(c)))
            .map(|p| p.id.clone())
            .collect();

        Self {
            primary: primary.to_string(),
            alternatives,
            selection_mode,
            required_capabilities: capabilities,
        }
    }

    /// Select next model to try after failure
    pub fn next_model(
        &self,
        failed_models: &[String],
        health_manager: &HealthManager,
        cost_strategy: CostStrategy,
    ) -> Option<String> {
        let candidates: Vec<_> = self.alternatives
            .iter()
            .filter(|m| !failed_models.contains(m))
            .filter(|m| health_manager.get_status(m).can_call())
            .collect();

        match &self.selection_mode {
            FailoverSelectionMode::Ordered => candidates.first().cloned().cloned(),
            FailoverSelectionMode::HealthPriority => {
                candidates.into_iter()
                    .min_by_key(|m| health_manager.get_status(m).priority())
                    .cloned()
            }
            FailoverSelectionMode::CostPriority => {
                // Implemented via matcher scoring
                candidates.first().cloned().cloned()
            }
            FailoverSelectionMode::Balanced { .. } => {
                // Combine health priority and cost
                candidates.first().cloned().cloned()
            }
        }
    }
}
```

### 1.4 Retry Orchestrator

```rust
/// Result of orchestrated execution
pub struct ExecutionResult<T> {
    /// Final result (success or last error)
    pub result: Result<T, ExecutionError>,

    /// Total attempts made
    pub attempts: u32,

    /// Models tried in order
    pub models_tried: Vec<String>,

    /// Total time spent
    pub total_duration: Duration,

    /// Detailed attempt log
    pub attempt_log: Vec<AttemptRecord>,
}

pub struct AttemptRecord {
    pub attempt_number: u32,
    pub model_id: String,
    pub duration: Duration,
    pub outcome: CallOutcome,
    pub error_detail: Option<String>,
}

/// Orchestrates retry and failover logic
pub struct RetryOrchestrator {
    policy: RetryPolicy,
    backoff: BackoffStrategy,
    health_manager: Arc<HealthManager>,
    metrics_collector: Arc<MetricsCollector>,
    budget_manager: Option<Arc<BudgetManager>>,
}

impl RetryOrchestrator {
    /// Execute with retry and failover
    pub async fn execute<F, T>(
        &self,
        failover_chain: &FailoverChain,
        request: &ExecutionRequest,
        executor: F,
    ) -> ExecutionResult<T>
    where
        F: Fn(&str, &ExecutionRequest) -> BoxFuture<'static, Result<T, CallOutcome>>,
    {
        let start = Instant::now();
        let mut attempts = 0;
        let mut models_tried = vec![];
        let mut attempt_log = vec![];
        let mut current_model = failover_chain.primary.clone();
        let mut rate_limit_hint = None;

        loop {
            attempts += 1;
            models_tried.push(current_model.clone());

            // Budget gate check
            if let Some(budget) = &self.budget_manager {
                if let Err(e) = budget.check_budget(request.estimated_cost()).await {
                    return ExecutionResult {
                        result: Err(ExecutionError::BudgetExceeded(e)),
                        attempts,
                        models_tried,
                        total_duration: start.elapsed(),
                        attempt_log,
                    };
                }
            }

            // Circuit breaker check
            let health = self.health_manager.get_status(&current_model);
            if !health.can_call() && !health.can_call_for_recovery() {
                // Skip to failover
                if let Some(next) = failover_chain.next_model(
                    &models_tried,
                    &self.health_manager,
                    CostStrategy::Balanced,
                ) {
                    current_model = next;
                    continue;
                } else {
                    return ExecutionResult {
                        result: Err(ExecutionError::AllModelsUnavailable),
                        attempts,
                        models_tried,
                        total_duration: start.elapsed(),
                        attempt_log,
                    };
                }
            }

            // Execute attempt with timeout
            let attempt_start = Instant::now();
            let result = tokio::time::timeout(
                self.policy.attempt_timeout,
                executor(&current_model, request),
            ).await;

            let (outcome, error_detail) = match &result {
                Ok(Ok(_)) => (CallOutcome::Success, None),
                Ok(Err(outcome)) => (*outcome, Some(outcome.error_type().to_string())),
                Err(_) => (CallOutcome::Timeout, Some("timeout".to_string())),
            };

            attempt_log.push(AttemptRecord {
                attempt_number: attempts,
                model_id: current_model.clone(),
                duration: attempt_start.elapsed(),
                outcome,
                error_detail: error_detail.clone(),
            });

            // Record to metrics
            self.metrics_collector.record_outcome(&current_model, outcome);

            // Success - return
            if let Ok(Ok(value)) = result {
                return ExecutionResult {
                    result: Ok(value),
                    attempts,
                    models_tried,
                    total_duration: start.elapsed(),
                    attempt_log,
                };
            }

            // Extract rate limit hint if present
            if outcome == CallOutcome::RateLimited {
                rate_limit_hint = extract_retry_after(&error_detail);
            }

            // Check if we should retry same model
            let should_retry = self.policy.retryable_outcomes.contains(&outcome)
                && attempts < self.policy.max_attempts
                && self.policy.total_timeout
                    .map(|t| start.elapsed() < t)
                    .unwrap_or(true);

            if should_retry {
                // Backoff then retry same model
                let delay = self.backoff.delay_for_attempt(attempts - 1, rate_limit_hint);
                tokio::time::sleep(delay).await;
                continue;
            }

            // Check if we should failover
            let should_failover = self.policy.failover_on_non_retryable
                || self.policy.retryable_outcomes.contains(&outcome);

            if should_failover {
                if let Some(next) = failover_chain.next_model(
                    &models_tried,
                    &self.health_manager,
                    CostStrategy::Balanced,
                ) {
                    current_model = next;
                    // Reset attempt count for new model? Configurable.
                    continue;
                }
            }

            // No more options
            return ExecutionResult {
                result: Err(ExecutionError::MaxAttemptsExceeded {
                    attempts,
                    last_outcome: outcome,
                }),
                attempts,
                models_tried,
                total_duration: start.elapsed(),
                attempt_log,
            };
        }
    }
}
```

---

## 2. Budget Manager

### 2.1 Architecture

```
                                    ┌─────────────────────┐
                                    │    BudgetManager    │
                                    └──────────┬──────────┘
                                               │
                    ┌──────────────────────────┼──────────────────────────┐
                    │                          │                          │
           ┌────────▼────────┐       ┌─────────▼─────────┐      ┌────────▼────────┐
           │   BudgetPool    │       │   CostTracker     │      │  CostEstimator  │
           │                 │       │                   │      │                 │
           │ • hierarchy     │       │ • accumulator     │      │ • token_pricing │
           │ • limits        │       │ • by_model        │      │ • pre_check     │
           │ • periods       │       │ • by_period       │      │ • margin        │
           └─────────────────┘       └───────────────────┘      └─────────────────┘
                    │                          │                          │
                    │                          │                          │
           ┌────────▼────────┐       ┌─────────▼─────────┐      ┌────────▼────────┐
           │ ResetScheduler  │       │  SpendingAlert    │      │   BudgetGate    │
           │                 │       │                   │      │                 │
           │ • daily/weekly  │       │ • thresholds      │      │ • enforcement   │
           │ • timezone      │       │ • callbacks       │      │ • soft/hard     │
           └─────────────────┘       └───────────────────┘      └─────────────────┘
```

### 2.2 Core Types

```rust
/// Budget limit scope
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BudgetScope {
    /// Global limit across all usage
    Global,
    /// Per-project limit
    Project(ProjectId),
    /// Per-session limit (conversation)
    Session(SessionId),
    /// Per-model limit
    Model(ModelId),
}

/// Time period for budget reset
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BudgetPeriod {
    /// Never resets
    Lifetime,
    /// Resets daily at configured time
    Daily { reset_hour: u8, timezone: Tz },
    /// Resets weekly on configured day
    Weekly { reset_day: Weekday, reset_hour: u8, timezone: Tz },
    /// Resets monthly on configured day
    Monthly { reset_day: u8, reset_hour: u8, timezone: Tz },
}

/// A single budget configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetLimit {
    /// Unique identifier
    pub id: String,

    /// Scope this limit applies to
    pub scope: BudgetScope,

    /// Reset period
    pub period: BudgetPeriod,

    /// Maximum spend in USD
    pub limit_usd: f64,

    /// Warning thresholds (fractions, e.g., [0.5, 0.8, 0.95])
    pub warning_thresholds: Vec<f64>,

    /// Action when limit exceeded
    pub enforcement: BudgetEnforcement,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BudgetEnforcement {
    /// Log warning but allow
    WarnOnly,
    /// Block new requests, allow in-flight
    SoftBlock,
    /// Block immediately
    HardBlock,
}

/// Current state of a budget
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetState {
    /// Reference to limit config
    pub limit_id: String,

    /// Current period start
    pub period_start: SystemTime,

    /// Next reset time
    pub next_reset: SystemTime,

    /// Accumulated spend in current period
    pub spent_usd: f64,

    /// Remaining budget
    pub remaining_usd: f64,

    /// Percentage used
    pub used_percent: f64,

    /// Warnings already fired
    pub warnings_fired: Vec<f64>,
}
```

### 2.3 Cost Estimation

```rust
/// Pre-call cost estimation
pub struct CostEstimator {
    /// Model pricing data (from profiles)
    pricing: HashMap<String, ModelPricing>,

    /// Safety margin for estimation (e.g., 1.2 = 20% buffer)
    safety_margin: f64,
}

pub struct ModelPricing {
    pub input_price_per_1m: f64,
    pub output_price_per_1m: f64,
    pub cached_input_price_per_1m: Option<f64>,
}

impl CostEstimator {
    /// Estimate cost before making a call
    pub fn estimate(
        &self,
        model_id: &str,
        input_tokens: u32,
        estimated_output_tokens: u32,
    ) -> CostEstimate {
        let pricing = self.pricing.get(model_id)
            .unwrap_or(&ModelPricing::default());

        let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_price_per_1m;
        let output_cost = (estimated_output_tokens as f64 / 1_000_000.0) * pricing.output_price_per_1m;
        let base_estimate = input_cost + output_cost;

        CostEstimate {
            model_id: model_id.to_string(),
            input_tokens,
            estimated_output_tokens,
            base_cost_usd: base_estimate,
            with_margin_usd: base_estimate * self.safety_margin,
            pricing_source: PricingSource::Profile,
        }
    }

    /// Update pricing from actual costs
    pub fn learn_from_actual(&mut self, model_id: &str, record: &CallRecord) {
        if let Some(actual_cost) = record.cost_usd {
            if let Some(pricing) = self.pricing.get_mut(model_id) {
                // Exponential moving average update
                let total_tokens = record.input_tokens + record.output_tokens;
                if total_tokens > 0 {
                    let actual_per_token = actual_cost / total_tokens as f64;
                    // Update would require more sophisticated logic
                }
            }
        }
    }
}

pub struct CostEstimate {
    pub model_id: String,
    pub input_tokens: u32,
    pub estimated_output_tokens: u32,
    pub base_cost_usd: f64,
    pub with_margin_usd: f64,
    pub pricing_source: PricingSource,
}

pub enum PricingSource {
    Profile,      // From static config
    Learned,      // From actual usage
    ProviderApi,  // From provider pricing API
}
```

### 2.4 Budget Manager

```rust
/// Central budget management
pub struct BudgetManager {
    /// Configured limits
    limits: Vec<BudgetLimit>,

    /// Current state per limit
    states: RwLock<HashMap<String, BudgetState>>,

    /// Cost estimator
    estimator: CostEstimator,

    /// Alert callback
    alert_handler: Option<Box<dyn Fn(BudgetAlert) + Send + Sync>>,

    /// Persistence layer
    storage: Option<Arc<dyn BudgetStorage>>,
}

pub enum BudgetCheckResult {
    /// OK to proceed
    Allowed { remaining_usd: f64 },

    /// Warning threshold crossed but allowed
    Warning {
        threshold: f64,
        remaining_usd: f64,
        message: String,
    },

    /// Blocked by soft limit
    SoftBlocked {
        limit_id: String,
        spent_usd: f64,
        limit_usd: f64,
    },

    /// Blocked by hard limit
    HardBlocked {
        limit_id: String,
        spent_usd: f64,
        limit_usd: f64,
    },
}

impl BudgetManager {
    /// Check if request is allowed under budget
    pub async fn check_budget(
        &self,
        scope: BudgetScope,
        estimated_cost: &CostEstimate,
    ) -> BudgetCheckResult {
        let states = self.states.read().await;

        // Find applicable limits (from most specific to global)
        let applicable: Vec<_> = self.limits.iter()
            .filter(|l| l.scope == scope || l.scope == BudgetScope::Global)
            .collect();

        for limit in applicable {
            if let Some(state) = states.get(&limit.id) {
                let would_spend = state.spent_usd + estimated_cost.with_margin_usd;

                if would_spend > limit.limit_usd {
                    match limit.enforcement {
                        BudgetEnforcement::HardBlock => {
                            return BudgetCheckResult::HardBlocked {
                                limit_id: limit.id.clone(),
                                spent_usd: state.spent_usd,
                                limit_usd: limit.limit_usd,
                            };
                        }
                        BudgetEnforcement::SoftBlock => {
                            return BudgetCheckResult::SoftBlocked {
                                limit_id: limit.id.clone(),
                                spent_usd: state.spent_usd,
                                limit_usd: limit.limit_usd,
                            };
                        }
                        BudgetEnforcement::WarnOnly => {
                            // Continue to check warnings
                        }
                    }
                }

                // Check warning thresholds
                let new_percent = would_spend / limit.limit_usd;
                for &threshold in &limit.warning_thresholds {
                    if new_percent >= threshold && !state.warnings_fired.contains(&threshold) {
                        return BudgetCheckResult::Warning {
                            threshold,
                            remaining_usd: limit.limit_usd - would_spend,
                            message: format!(
                                "Budget {}% used ({:.2}/{:.2} USD)",
                                (threshold * 100.0) as u32,
                                would_spend,
                                limit.limit_usd
                            ),
                        };
                    }
                }
            }
        }

        let total_remaining: f64 = applicable.iter()
            .filter_map(|l| states.get(&l.id))
            .map(|s| s.remaining_usd)
            .fold(f64::MAX, f64::min);

        BudgetCheckResult::Allowed { remaining_usd: total_remaining }
    }

    /// Record actual cost after call completes
    pub async fn record_cost(
        &self,
        scope: BudgetScope,
        record: &CallRecord,
    ) {
        if let Some(cost) = record.cost_usd {
            let mut states = self.states.write().await;

            for limit in &self.limits {
                if limit.scope == scope || limit.scope == BudgetScope::Global {
                    if let Some(state) = states.get_mut(&limit.id) {
                        state.spent_usd += cost;
                        state.remaining_usd = (limit.limit_usd - state.spent_usd).max(0.0);
                        state.used_percent = state.spent_usd / limit.limit_usd;
                    }
                }
            }

            // Persist if storage available
            if let Some(storage) = &self.storage {
                let _ = storage.save_states(&states).await;
            }
        }
    }

    /// Get current budget status for display
    pub async fn get_status(&self, scope: BudgetScope) -> Vec<BudgetState> {
        let states = self.states.read().await;

        self.limits.iter()
            .filter(|l| l.scope == scope || l.scope == BudgetScope::Global)
            .filter_map(|l| states.get(&l.id).cloned())
            .collect()
    }
}
```

### 2.5 Reset Scheduler

```rust
/// Handles periodic budget resets
pub struct ResetScheduler {
    manager: Arc<BudgetManager>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl ResetScheduler {
    /// Start reset timers for all limits
    pub async fn start(&self) {
        let limits = self.manager.get_limits();
        let mut handles = self.handles.lock().await;

        for limit in limits {
            let manager = Arc::clone(&self.manager);
            let limit_id = limit.id.clone();

            let handle = tokio::spawn(async move {
                loop {
                    let next_reset = calculate_next_reset(&limit.period);
                    let delay = next_reset.duration_since(SystemTime::now())
                        .unwrap_or(Duration::from_secs(3600));

                    tokio::time::sleep(delay).await;

                    // Reset the budget
                    manager.reset_limit(&limit_id).await;
                }
            });

            handles.push(handle);
        }
    }
}
```

---

## 3. Integration Points

### 3.1 With ModelMatcher

```rust
impl ModelMatcher {
    /// Route with budget awareness
    pub fn route_with_budget(
        &self,
        request: &RoutingRequest,
        budget_state: &BudgetState,
    ) -> Result<&ModelProfile, RoutingError> {
        // If budget is low (<20% remaining), prefer cheaper models
        let cost_bias = if budget_state.used_percent > 0.8 {
            CostStrategy::Cheapest
        } else {
            request.cost_strategy.unwrap_or(CostStrategy::Balanced)
        };

        let modified_request = RoutingRequest {
            cost_strategy: Some(cost_bias),
            ..request.clone()
        };

        self.route(&modified_request)
    }
}
```

### 3.2 With IntelligentRouter

```rust
impl IntelligentRouter {
    /// Execute with full retry, failover, and budget support
    pub async fn execute_intelligent<F, T>(
        &self,
        request: &ExecutionRequest,
        executor: F,
    ) -> ExecutionResult<T>
    where
        F: Fn(&str, &ExecutionRequest) -> BoxFuture<'static, Result<T, CallOutcome>>,
    {
        // 1. Budget pre-check
        let estimate = self.budget_manager.estimator.estimate(
            &request.preferred_model,
            request.input_tokens,
            request.estimated_output,
        );

        let budget_check = self.budget_manager
            .check_budget(request.scope, &estimate)
            .await;

        match budget_check {
            BudgetCheckResult::HardBlocked { .. } => {
                return ExecutionResult::budget_exceeded(budget_check);
            }
            BudgetCheckResult::Warning { message, .. } => {
                // Emit warning event
                self.emit_event(RouterEvent::BudgetWarning(message));
            }
            _ => {}
        }

        // 2. Build failover chain
        let chain = FailoverChain::from_matcher(
            &self.matcher,
            &request.preferred_model,
            FailoverSelectionMode::HealthPriority,
        );

        // 3. Execute with retry orchestrator
        let result = self.retry_orchestrator
            .execute(&chain, request, executor)
            .await;

        // 4. Record cost
        if let Ok(ref success) = result.result {
            if let Some(record) = success.call_record() {
                self.budget_manager.record_cost(request.scope, record).await;
            }
        }

        result
    }
}
```

---

## 4. Configuration

```toml
[model_router.retry]
enabled = true
max_attempts = 3
attempt_timeout_ms = 30000
total_timeout_ms = 90000
retryable_errors = ["timeout", "rate_limited", "network_error"]
failover_on_non_retryable = true

[model_router.retry.backoff]
strategy = "exponential_jitter"
initial_ms = 100
max_ms = 5000
jitter_factor = 0.2

[model_router.budget]
enabled = true
default_enforcement = "soft_block"

[[model_router.budget.limits]]
id = "daily_global"
scope = "global"
period = "daily"
reset_hour = 0
timezone = "UTC"
limit_usd = 10.0
warning_thresholds = [0.5, 0.8, 0.95]
enforcement = "soft_block"

[[model_router.budget.limits]]
id = "session_limit"
scope = "session"
period = "lifetime"
limit_usd = 1.0
warning_thresholds = [0.8]
enforcement = "warn_only"
```

---

## 5. Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Budget exceeded: {0}")]
    BudgetExceeded(BudgetCheckResult),

    #[error("All models unavailable after trying: {models_tried:?}")]
    AllModelsUnavailable { models_tried: Vec<String> },

    #[error("Max attempts ({attempts}) exceeded, last error: {last_outcome:?}")]
    MaxAttemptsExceeded { attempts: u32, last_outcome: CallOutcome },

    #[error("Total timeout exceeded after {elapsed:?}")]
    TotalTimeoutExceeded { elapsed: Duration },

    #[error("Circuit breaker open for model: {model_id}")]
    CircuitOpen { model_id: String },
}
```

---

## 6. Observability

### Events for UI

```rust
pub enum RouterEvent {
    /// Retry attempt starting
    RetryAttempt { attempt: u32, model_id: String, reason: String },

    /// Failover to different model
    Failover { from: String, to: String, reason: String },

    /// Budget warning threshold crossed
    BudgetWarning(String),

    /// Budget limit reached
    BudgetExceeded { limit_id: String, spent: f64, limit: f64 },

    /// Budget reset occurred
    BudgetReset { limit_id: String },

    /// Cost recorded
    CostRecorded { model_id: String, cost: f64, remaining: f64 },
}
```

### Metrics

- `model_router_retry_attempts_total{model, outcome}`
- `model_router_failover_total{from_model, to_model}`
- `model_router_budget_spent_usd{scope, limit_id}`
- `model_router_budget_remaining_usd{scope, limit_id}`
- `model_router_budget_blocked_total{scope, limit_id, enforcement}`

---

## 7. Testing Strategy

1. **Unit Tests**:
   - BackoffStrategy calculations
   - BudgetCheckResult logic
   - FailoverChain selection modes

2. **Integration Tests**:
   - RetryOrchestrator with mock executor
   - BudgetManager with simulated costs
   - Full flow: route → retry → failover → record cost

3. **Property-Based Tests**:
   - Budget never exceeds limit (with hard enforcement)
   - Retry always respects max_attempts
   - Failover chain never loops

4. **Chaos Tests**:
   - Random model failures
   - Rate limit storms
   - Budget edge cases (exactly at limit)
