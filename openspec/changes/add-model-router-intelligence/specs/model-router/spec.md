## ADDED Requirements

### Requirement: Runtime Metrics Collection
The system SHALL collect and aggregate runtime metrics from AI model API calls.

#### Scenario: Record successful call
- **WHEN** an API call completes successfully
- **THEN** CallRecord is created with model_id, latency, tokens, cost
- **AND** record is added to ring buffer asynchronously
- **AND** aggregated metrics are updated incrementally

#### Scenario: Record failed call
- **WHEN** an API call fails
- **THEN** CallRecord is created with error type and status code
- **AND** consecutive_failures counter is incremented
- **AND** error is added to error_distribution

#### Scenario: Multi-window aggregation
- **WHEN** metrics are aggregated
- **THEN** short_term (5min), medium_term (1hr), long_term (24hr) windows are updated
- **AND** each window maintains independent statistics
- **AND** old data is automatically aged out

#### Scenario: Latency statistics
- **WHEN** latency metrics are computed
- **THEN** p50, p90, p95, p99 percentiles are calculated
- **AND** mean and stddev are available
- **AND** statistics use streaming algorithm (no full history storage)

### Requirement: Metrics Persistence
The system SHALL persist metrics data to SQLite for cross-session learning.

#### Scenario: Periodic flush
- **WHEN** flush interval (default 300s) elapses
- **THEN** aggregated metrics are written to SQLite
- **AND** recent call records are appended for audit
- **AND** flush is non-blocking

#### Scenario: Load on startup
- **WHEN** MetricsCollector is initialized
- **THEN** historical metrics are loaded from SQLite
- **AND** routing immediately benefits from past data
- **AND** missing database creates new one

#### Scenario: Ring buffer overflow
- **WHEN** ring buffer reaches capacity (default 10K)
- **THEN** oldest records are evicted
- **AND** aggregated metrics are preserved
- **AND** no memory growth

### Requirement: Dynamic Scoring
The system SHALL score models based on runtime metrics for routing decisions.

#### Scenario: Compute weighted score
- **WHEN** DynamicScorer evaluates a model
- **THEN** latency_score × latency_weight is computed
- **AND** cost_score × cost_weight is computed
- **AND** reliability_score × reliability_weight is computed
- **AND** quality_score × quality_weight is computed
- **AND** final score is weighted sum in range [0.0, 1.0]

#### Scenario: Penalty for failures
- **WHEN** model has consecutive failures >= threshold
- **THEN** penalty factor is applied to score
- **AND** short-term success rate drop triggers additional penalty
- **AND** rate-limited model gets zero score

#### Scenario: Cold start fallback
- **WHEN** model has fewer than 10 recorded calls
- **THEN** static scoring based on CostTier/LatencyTier is used
- **AND** static score is in range [0.3, 0.7]
- **AND** gradual transition to dynamic scoring

### Requirement: Health Status Tracking
The system SHALL track health status for each AI model.

#### Scenario: Healthy status
- **WHEN** model responds successfully with acceptable latency
- **THEN** status is HealthStatus::Healthy
- **AND** model is fully available for routing
- **AND** no warnings displayed

#### Scenario: Degraded status
- **WHEN** model responds but with high latency (>10s p95)
- **OR** model is near rate limit (remaining <20%)
- **THEN** status is HealthStatus::Degraded
- **AND** model is available but with warning
- **AND** degradation_reason describes the issue

#### Scenario: Unhealthy status
- **WHEN** model has consecutive failures >= failure_threshold (default 3)
- **OR** model returns authentication error
- **OR** model is rate limited
- **THEN** status is HealthStatus::Unhealthy
- **AND** model is excluded from routing
- **AND** unhealthy_reason describes the issue

#### Scenario: Unknown status
- **WHEN** model has no recorded calls
- **THEN** status is HealthStatus::Unknown
- **AND** model is allowed for first call
- **AND** status updates after first result

### Requirement: Circuit Breaker
The system SHALL implement circuit breaker pattern for failure isolation.

#### Scenario: Open circuit
- **WHEN** model has >= circuit_failure_threshold (default 5) failures in window (default 60s)
- **THEN** status becomes HealthStatus::CircuitOpen
- **AND** all calls to model are blocked
- **AND** cooldown timer starts

#### Scenario: Half-open transition
- **WHEN** cooldown period (default 30s) elapses
- **THEN** circuit transitions to HalfOpen state
- **AND** single test request is allowed
- **AND** next result determines final state

#### Scenario: Close circuit on recovery
- **WHEN** half_open_successes (default 2) consecutive successes occur
- **THEN** circuit transitions to Closed (Healthy)
- **AND** model is fully available again
- **AND** failure_count is reset

#### Scenario: Reopen on half-open failure
- **WHEN** call fails in HalfOpen state
- **THEN** circuit reopens to Open state
- **AND** cooldown increases exponentially (up to 16 min)
- **AND** failure_count is preserved

### Requirement: Health State Transitions
The system SHALL follow defined state machine for health transitions.

#### Scenario: Unknown to Healthy
- **WHEN** first call to unknown model succeeds
- **THEN** status becomes Healthy
- **AND** consecutive_successes is set to 1

#### Scenario: Unknown to Unhealthy
- **WHEN** first call to unknown model fails
- **THEN** status becomes Unhealthy
- **AND** consecutive_failures is set to 1

#### Scenario: Healthy to Degraded
- **WHEN** successful call has latency > degradation_threshold
- **OR** rate_limit_remaining < warning_threshold
- **THEN** status becomes Degraded
- **AND** degradation_reason is set

#### Scenario: Degraded to Healthy
- **WHEN** degraded_recovery_successes (default 3) consecutive successes occur
- **AND** latency returns to normal
- **AND** rate limit recovers
- **THEN** status becomes Healthy

#### Scenario: Any to Unhealthy
- **WHEN** consecutive_failures >= failure_threshold
- **THEN** status becomes Unhealthy
- **AND** unhealthy_reason describes cause

#### Scenario: Unhealthy to Healthy
- **WHEN** recovery_successes (default 2) consecutive successes occur
- **THEN** status becomes Healthy
- **AND** consecutive_failures is reset

### Requirement: Health Events
The system SHALL emit events for health status changes.

#### Scenario: Status change event
- **WHEN** health status changes
- **THEN** HealthEvent::StatusChanged is emitted
- **AND** event includes old_status, new_status, reason
- **AND** subscribers receive event asynchronously

#### Scenario: Circuit breaker events
- **WHEN** circuit opens
- **THEN** HealthEvent::CircuitOpened is emitted with cooldown duration
- **WHEN** circuit closes
- **THEN** HealthEvent::CircuitClosed is emitted

#### Scenario: Rate limit warning
- **WHEN** rate_limit_remaining < warning_threshold
- **THEN** HealthEvent::RateLimitWarning is emitted
- **AND** event includes remaining_percent and reset_at

### Requirement: Health-Aware Routing
The system SHALL use health status in routing decisions.

#### Scenario: Filter unavailable models
- **WHEN** route_with_health() is called
- **THEN** models with CircuitOpen status are excluded
- **AND** models with Unhealthy status are excluded
- **AND** only Healthy/Degraded/Unknown models are considered

#### Scenario: Prioritize healthy models
- **WHEN** multiple models are available
- **THEN** Healthy models are preferred over Degraded
- **AND** Degraded models are preferred over Unknown
- **AND** within same status, use capability/cost matching

#### Scenario: Allow recovery attempts
- **WHEN** model is in HalfOpen state
- **THEN** can_call() returns AllowedForRecovery
- **AND** routing may select model for recovery test
- **AND** recovery weight is lower than healthy

### Requirement: Intelligent Routing
The system SHALL combine metrics and health for optimal routing.

#### Scenario: Combined scoring
- **WHEN** route_intelligent() is called
- **THEN** health status determines base eligibility
- **AND** dynamic score determines ranking among eligible models
- **AND** final_score = health_weight × dynamic_score

#### Scenario: Exploratory routing
- **WHEN** exploration_rate (default 0.05) is configured
- **THEN** 5% of requests explore non-optimal models
- **AND** exploration collects data on underused models
- **AND** epsilon-greedy strategy is used

#### Scenario: Graceful degradation
- **WHEN** all preferred models are unavailable
- **THEN** routing falls back to degraded models
- **AND** if all models unavailable, return error with details
- **AND** error lists blocked models and reasons

### Requirement: Active Health Probing
The system SHALL optionally probe unhealthy models for recovery detection.

#### Scenario: Probe unhealthy models
- **WHEN** active_probing is enabled
- **AND** model status is Unhealthy or CircuitOpen
- **THEN** periodic probe (default 30s) is sent
- **AND** probe uses minimal request ("Hi")
- **AND** probe result updates health status

#### Scenario: Use dedicated health endpoint
- **WHEN** model has health_url configured
- **THEN** probe uses HTTP GET to health endpoint
- **AND** success is 2xx response
- **AND** no API cost incurred

#### Scenario: Probe timeout
- **WHEN** probe times out (default 10s)
- **THEN** probe is recorded as failure
- **AND** status remains unchanged
- **AND** next probe scheduled normally

### Requirement: Configuration
The system SHALL be configurable via TOML configuration file.

#### Scenario: Metrics configuration
- **WHEN** `[cowork.model_router.metrics]` section exists
- **THEN** enabled, buffer_size, aggregation_interval, flush_interval are read
- **AND** db_path specifies SQLite location
- **AND** exploration_rate controls epsilon-greedy

#### Scenario: Scoring configuration
- **WHEN** `[cowork.model_router.metrics.scoring]` section exists
- **THEN** latency_weight, cost_weight, reliability_weight, quality_weight are read
- **AND** weights should sum to 1.0 (normalized if not)
- **AND** threshold values are read

#### Scenario: Health configuration
- **WHEN** `[cowork.model_router.health]` section exists
- **THEN** enabled, failure_threshold, recovery_successes are read
- **AND** latency thresholds, rate limit thresholds are read
- **AND** circuit breaker settings are read

#### Scenario: Default configuration
- **WHEN** configuration sections are missing
- **THEN** sensible defaults are used
- **AND** metrics and health are enabled by default
- **AND** system works without explicit configuration

### Requirement: UniFFI Integration
The system SHALL expose metrics and health to Swift UI via UniFFI.

#### Scenario: Get model metrics
- **WHEN** cowork_get_model_metrics(model_id) is called from Swift
- **THEN** ModelMetricsFfi is returned with all statistics
- **AND** None is returned if model has no data

#### Scenario: Get health summary
- **WHEN** cowork_get_all_health_summary() is called from Swift
- **THEN** Vec<ModelHealthSummaryFfi> is returned
- **AND** each summary includes status emoji and text
- **AND** list is sorted by status priority

#### Scenario: Manual status override
- **WHEN** cowork_set_model_status(model_id, status, reason) is called
- **THEN** model status is forcibly changed
- **AND** event is emitted
- **AND** override persists until next automatic transition

#### Scenario: Record user feedback
- **WHEN** cowork_record_feedback(call_id, feedback) is called
- **THEN** feedback is associated with call record
- **AND** satisfaction_score is updated
- **AND** quality scoring incorporates feedback
