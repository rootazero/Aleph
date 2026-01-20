# ab-testing Specification

## Purpose

Provides A/B testing framework for controlled experimentation with routing strategies, enabling data-driven optimization of model selection through traffic splitting, outcome tracking, and statistical analysis.

## ADDED Requirements

### Requirement: Experiment Configuration

The system SHALL support defining and managing A/B experiments through configuration.

#### Scenario: Define experiment with variants
- **GIVEN** the system is initializing
- **WHEN** configuration includes an experiment definition
- **THEN** experiment is created with unique `id`, `name`, and `enabled` status
- **AND** experiment includes `traffic_percentage` (0-100) for inclusion rate
- **AND** experiment includes at least 2 variants with `id`, `name`, and `weight`
- **AND** invalid configuration returns `ConfigError::InvalidExperiment`

#### Scenario: Target experiment to specific intent
- **GIVEN** an experiment is defined
- **WHEN** `target_intent` is specified (e.g., "Reasoning", "CodeGeneration")
- **THEN** experiment only applies to requests with matching `TaskIntent`
- **AND** non-matching requests bypass the experiment entirely
- **AND** multiple experiments can target the same intent

#### Scenario: Configure tracked metrics
- **GIVEN** an experiment is defined
- **WHEN** `tracked_metrics` array is specified
- **THEN** system tracks listed metrics per variant (latency_ms, cost_usd, etc.)
- **AND** unknown metric names are ignored with warning
- **AND** at least one metric must be specified for analysis

#### Scenario: Set experiment time window
- **GIVEN** an experiment is defined
- **WHEN** `start_time` and/or `end_time` are specified
- **THEN** experiment only runs within the time window
- **AND** requests outside time window bypass experiment
- **AND** omitted times default to "now" for start and "indefinite" for end

### Requirement: Traffic Splitting

The system SHALL consistently assign traffic to experiment variants.

#### Scenario: Assign by user_id
- **GIVEN** assignment strategy is `user_id`
- **WHEN** request includes `user_id`
- **THEN** same `user_id` always gets same variant (deterministic)
- **AND** assignment uses consistent hashing (SipHash)
- **AND** variant selection respects configured weights
- **AND** missing `user_id` falls back to `session_id` or `request_id`

#### Scenario: Assign by session_id
- **GIVEN** assignment strategy is `session_id`
- **WHEN** request includes `session_id`
- **THEN** same `session_id` always gets same variant within session
- **AND** new session may get different variant
- **AND** useful for within-session consistency

#### Scenario: Assign by request_id (random)
- **GIVEN** assignment strategy is `request_id`
- **WHEN** request has unique `request_id`
- **THEN** each request randomly assigned to variant
- **AND** no consistency guarantee across requests
- **AND** fastest path for high-throughput experiments

#### Scenario: Traffic percentage filtering
- **GIVEN** experiment has `traffic_percentage = 10`
- **WHEN** 1000 requests are processed
- **THEN** approximately 100 requests enter the experiment
- **AND** remaining 900 requests use default routing
- **AND** actual percentage is within 2% of configured (8-12% for 10%)

#### Scenario: Weighted variant distribution
- **GIVEN** experiment has variants with weights [50, 30, 20]
- **WHEN** requests are assigned to variants
- **THEN** distribution approximates 50%/30%/20% split
- **AND** weights are relative (actual percentages sum to 100%)
- **AND** zero-weight variant receives no traffic

### Requirement: Variant Overrides

The system SHALL apply variant-specific routing overrides.

#### Scenario: Override model selection
- **GIVEN** variant has `model_override = "gpt-4o"`
- **WHEN** request is assigned to this variant
- **THEN** routing selects the specified model
- **AND** normal routing logic is bypassed for model selection
- **AND** model must exist in configured profiles

#### Scenario: Override cost strategy
- **GIVEN** variant has `cost_strategy_override = "cheapest"`
- **WHEN** request is assigned to this variant
- **THEN** routing uses the specified cost strategy
- **AND** normal cost strategy selection is bypassed
- **AND** model selection follows override strategy

#### Scenario: No override (observe only)
- **GIVEN** variant has no overrides specified
- **WHEN** request is assigned to this variant
- **THEN** normal routing logic is used
- **AND** variant assignment is still recorded
- **AND** useful for observing baseline behavior

### Requirement: Outcome Tracking

The system SHALL track outcomes per experiment variant.

#### Scenario: Record request outcome
- **GIVEN** request was assigned to a variant
- **WHEN** request execution completes
- **THEN** outcome is recorded with experiment_id, variant_id, timestamp
- **AND** specified metrics are captured (latency, cost, tokens, etc.)
- **AND** model_used and request_id are included

#### Scenario: Aggregate statistics
- **GIVEN** outcomes are recorded for a variant
- **WHEN** statistics are requested
- **THEN** system returns count, mean, std_dev for each metric
- **AND** min/max values are tracked
- **AND** statistics are computed incrementally (O(1) space)

#### Scenario: Retain raw outcomes
- **GIVEN** `max_raw_outcomes` is configured
- **WHEN** raw outcomes exceed limit
- **THEN** oldest outcomes are evicted (FIFO)
- **AND** aggregated statistics are unaffected
- **AND** raw data available for detailed analysis

### Requirement: Statistical Analysis

The system SHALL provide statistical significance testing.

#### Scenario: Two-sample t-test
- **GIVEN** control and treatment variants have sufficient samples (n >= 30)
- **WHEN** significance test is requested for a metric
- **THEN** system computes Welch's t-test
- **AND** returns t-statistic, p-value, and is_significant (p < 0.05)
- **AND** handles unequal variances correctly

#### Scenario: Effect size calculation
- **GIVEN** significance test is performed
- **WHEN** results are returned
- **THEN** relative_change is calculated ((treatment - control) / control)
- **AND** Cohen's d is calculated for standardized effect size
- **AND** confidence intervals are optionally included

#### Scenario: Insufficient data
- **GIVEN** variant has fewer than 30 samples
- **WHEN** significance test is requested
- **THEN** status is `InsufficientData`
- **AND** preliminary statistics are still available
- **AND** message indicates samples needed

### Requirement: Experiment Reporting

The system SHALL generate human-readable experiment reports.

#### Scenario: Generate experiment report
- **GIVEN** experiment has recorded outcomes
- **WHEN** report is requested
- **THEN** report includes experiment name, status, duration
- **AND** report includes per-variant summaries (mean, std_dev, sample_count)
- **AND** report includes significance tests for tracked metrics

#### Scenario: Export report as JSON
- **GIVEN** experiment report is generated
- **WHEN** JSON format is requested
- **THEN** report is serializable to JSON
- **AND** all numeric values are precise (no formatting loss)
- **AND** timestamps are ISO 8601 format

#### Scenario: Include recommendation
- **GIVEN** experiment has statistically significant results
- **WHEN** report is generated
- **THEN** recommendation is included based on metrics
- **AND** recommendation indicates which variant performed better
- **AND** recommendation notes confidence level

### Requirement: Experiment Lifecycle

The system SHALL support experiment enable/disable and cleanup.

#### Scenario: Enable experiment
- **GIVEN** experiment is defined but disabled
- **WHEN** `enable_experiment(id)` is called
- **THEN** experiment starts accepting traffic
- **AND** start_time is updated if not set
- **AND** existing stats are preserved

#### Scenario: Disable experiment
- **GIVEN** experiment is running
- **WHEN** `disable_experiment(id)` is called
- **THEN** experiment stops accepting traffic
- **AND** existing stats are preserved
- **AND** report can still be generated

#### Scenario: Cleanup completed experiments
- **GIVEN** experiment has passed end_time
- **WHEN** cleanup runs (periodic or manual)
- **THEN** experiment status becomes `Completed`
- **AND** stats are retained for configured retention period
- **AND** memory is freed after retention expires

### Requirement: FFI Export

The system SHALL expose A/B testing status via UniFFI.

#### Scenario: List active experiments
- **GIVEN** A/B testing is enabled
- **WHEN** `get_active_experiments()` is called via FFI
- **THEN** returns list of ExperimentSummary records
- **AND** includes id, name, enabled, traffic_percentage, sample_count
- **AND** sorted by start_time descending

#### Scenario: Get experiment statistics
- **GIVEN** experiment exists
- **WHEN** `get_experiment_stats(id)` is called via FFI
- **THEN** returns ExperimentReport if found
- **AND** returns None if experiment not found
- **AND** includes all significance test results

#### Scenario: Get user's variant assignments
- **GIVEN** user has been assigned to experiments
- **WHEN** `get_user_experiment_assignment(user_id)` is called
- **THEN** returns list of current VariantAssignment records
- **AND** useful for debugging and UI display
- **AND** only includes active experiments
