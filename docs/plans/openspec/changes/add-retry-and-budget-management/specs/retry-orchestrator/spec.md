# retry-orchestrator Specification

## Purpose

Provides resilient request execution with automatic retry and failover capabilities for the Model Router. When an API call fails due to transient errors, the system automatically retries with configurable backoff. When a model is persistently unavailable, the system fails over to alternative models with compatible capabilities.

## ADDED Requirements

### Requirement: Retry Policy Configuration
The system SHALL allow configuration of retry behavior per request or globally.

#### Scenario: Configure maximum retry attempts
- **WHEN** RetryPolicy is created
- **THEN** max_attempts can be set (default: 3)
- **AND** attempt_timeout can be set per-attempt (default: 30s)
- **AND** total_timeout can optionally cap all attempts (default: 90s)
- **AND** values are validated at construction time

#### Scenario: Configure retryable error types
- **WHEN** RetryPolicy specifies retryable_outcomes
- **THEN** only those CallOutcome types trigger retry
- **AND** default includes Timeout, RateLimited, NetworkError
- **AND** ContentFiltered and ContextOverflow are never retryable
- **AND** custom lists override defaults

#### Scenario: Configure failover behavior
- **WHEN** RetryPolicy.failover_on_non_retryable is true
- **THEN** non-retryable errors trigger failover to alternative models
- **AND** when false, non-retryable errors fail immediately
- **AND** default is true

### Requirement: Backoff Strategy
The system SHALL implement configurable backoff delays between retry attempts.

#### Scenario: Constant backoff
- **WHEN** BackoffStrategy::Constant is selected
- **THEN** delay between attempts is fixed
- **AND** no jitter is applied
- **AND** useful for debugging/testing

#### Scenario: Exponential backoff
- **WHEN** BackoffStrategy::Exponential is selected
- **THEN** delay doubles with each attempt (initial * 2^attempt)
- **AND** delay is capped at configured maximum
- **AND** multiplier can be customized (default: 2.0)

#### Scenario: Exponential backoff with jitter
- **WHEN** BackoffStrategy::ExponentialJitter is selected
- **THEN** base delay follows exponential pattern
- **AND** random jitter (0 to jitter_factor * base) is added
- **AND** jitter prevents thundering herd on recovery
- **AND** this is the recommended default strategy

#### Scenario: Rate limit aware backoff
- **WHEN** BackoffStrategy::RateLimitAware is selected
- **AND** API returns Retry-After header
- **THEN** delay respects the Retry-After value
- **AND** fallback strategy is used when header is absent
- **AND** minimum 1 second delay is enforced

### Requirement: Failover Chain Management
The system SHALL maintain ordered lists of alternative models for failover.

#### Scenario: Build failover chain from capabilities
- **WHEN** FailoverChain::from_matcher is called with a primary model
- **THEN** alternatives are found by matching overlapping capabilities
- **AND** alternatives are ordered by configured selection mode
- **AND** chain includes only models registered in ModelMatcher

#### Scenario: Manual failover chain configuration
- **WHEN** FailoverChain is constructed with explicit alternatives
- **THEN** the provided order is preserved
- **AND** alternatives are validated against available models
- **AND** invalid model IDs cause error at construction

#### Scenario: Select next model in chain (Ordered mode)
- **WHEN** FailoverSelectionMode::Ordered is active
- **AND** next_model is called after failure
- **THEN** the first untried model in alternatives list is selected
- **AND** already-tried models are skipped
- **AND** unhealthy models are skipped

#### Scenario: Select next model in chain (HealthPriority mode)
- **WHEN** FailoverSelectionMode::HealthPriority is active
- **AND** next_model is called after failure
- **THEN** the healthiest untried model is selected
- **AND** health is determined by HealthManager status
- **AND** Healthy > Degraded > Unknown > others

#### Scenario: Select next model in chain (CostPriority mode)
- **WHEN** FailoverSelectionMode::CostPriority is active
- **AND** next_model is called after failure
- **THEN** the cheapest healthy model is selected
- **AND** cost is determined by ModelProfile.cost_tier
- **AND** ties are broken by health status

### Requirement: Retry Orchestrator Execution
The system SHALL orchestrate retry and failover in a single execution flow.

#### Scenario: Execute with successful first attempt
- **WHEN** RetryOrchestrator.execute is called
- **AND** first attempt succeeds
- **THEN** result is returned immediately
- **AND** ExecutionResult.attempts equals 1
- **AND** ExecutionResult.models_tried contains only primary model
- **AND** metrics are recorded for the successful call

#### Scenario: Execute with retry on transient error
- **WHEN** first attempt fails with retryable error
- **AND** max_attempts > 1
- **THEN** backoff delay is applied
- **AND** same model is retried
- **AND** attempt counter increments
- **AND** attempt_log records each attempt with outcome

#### Scenario: Execute with failover after exhausted retries
- **WHEN** all retry attempts fail on primary model
- **AND** failover_chain has alternatives
- **THEN** failover to next healthy model
- **AND** retry counter resets for new model (configurable)
- **AND** ExecutionResult.models_tried includes all attempted models

#### Scenario: Execute with circuit breaker integration
- **WHEN** target model has HealthStatus::CircuitOpen
- **THEN** skip directly to next model in failover chain
- **AND** do not count as an attempt
- **AND** no request is made to the blocked model
- **AND** log indicates circuit breaker skip

#### Scenario: Execute with budget gate integration
- **WHEN** BudgetManager is configured
- **AND** budget check fails before attempt
- **THEN** return ExecutionError::BudgetExceeded immediately
- **AND** do not make API call
- **AND** do not try failover models

#### Scenario: All models exhausted
- **WHEN** all models in failover chain have been tried
- **AND** all attempts failed
- **THEN** return ExecutionError::AllModelsUnavailable
- **AND** ExecutionResult includes all models_tried
- **AND** attempt_log contains full history

#### Scenario: Total timeout exceeded
- **WHEN** total_timeout is configured
- **AND** cumulative time exceeds total_timeout
- **THEN** abort current attempt
- **AND** return ExecutionError::TotalTimeoutExceeded
- **AND** do not start additional attempts

### Requirement: Execution Logging and Observability
The system SHALL provide detailed execution logs for debugging and monitoring.

#### Scenario: Record attempt details
- **WHEN** an attempt completes (success or failure)
- **THEN** AttemptRecord is created with attempt_number, model_id, duration, outcome
- **AND** error_detail captures error message if applicable
- **AND** records are appended to ExecutionResult.attempt_log

#### Scenario: Emit retry events
- **WHEN** retry is about to begin
- **THEN** RouterEvent::RetryAttempt is emitted
- **AND** event includes attempt number, model_id, and reason
- **AND** event is available to UI via callback

#### Scenario: Emit failover events
- **WHEN** failover to different model occurs
- **THEN** RouterEvent::Failover is emitted
- **AND** event includes from_model, to_model, and reason
- **AND** event is available to UI via callback

### Requirement: Metrics Recording
The system SHALL record retry and failover metrics for analysis.

#### Scenario: Record retry attempts
- **WHEN** retry attempt occurs
- **THEN** MetricsCollector records attempt outcome
- **AND** outcome is tagged with model_id
- **AND** counter metric increments for retries

#### Scenario: Record failover transitions
- **WHEN** failover occurs
- **THEN** failover counter metric increments
- **AND** metric is tagged with from_model and to_model
- **AND** reason for failover is logged
