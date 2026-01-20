# Tasks: Add Intelligent Retry/Failover and Budget Management

## Phase 1: Core Data Structures

- [ ] **1.1** Create `core/src/dispatcher/model_router/retry.rs`
  - [ ] Define `RetryPolicy` struct with configurable max_attempts, timeouts, retryable errors
  - [ ] Define `BackoffStrategy` enum (Constant, Exponential, ExponentialJitter, RateLimitAware)
  - [ ] Implement `BackoffStrategy::delay_for_attempt()` with jitter calculation
  - [ ] Add unit tests for backoff calculations

- [ ] **1.2** Create `core/src/dispatcher/model_router/failover.rs`
  - [ ] Define `FailoverSelectionMode` enum (Ordered, HealthPriority, CostPriority, Balanced)
  - [ ] Define `FailoverChain` struct with primary and alternatives
  - [ ] Implement `FailoverChain::from_matcher()` to auto-build chains from capability overlap
  - [ ] Implement `FailoverChain::next_model()` with health-aware selection
  - [ ] Add unit tests for failover selection

- [ ] **1.3** Create `core/src/dispatcher/model_router/budget.rs`
  - [ ] Define `BudgetScope` enum (Global, Project, Session, Model)
  - [ ] Define `BudgetPeriod` enum (Lifetime, Daily, Weekly, Monthly)
  - [ ] Define `BudgetLimit` and `BudgetState` structs
  - [ ] Define `BudgetEnforcement` enum (WarnOnly, SoftBlock, HardBlock)
  - [ ] Define `BudgetCheckResult` enum with all cases
  - [ ] Add unit tests for budget state calculations

## Phase 2: Cost Estimation

- [ ] **2.1** Implement `CostEstimator` in budget.rs
  - [ ] Define `ModelPricing` struct with input/output prices
  - [ ] Implement `CostEstimator::estimate()` with safety margin
  - [ ] Load pricing from `ModelProfile.cost_per_million_*` fields
  - [ ] Add unit tests with known model prices

- [ ] **2.2** Add pricing fields to `ModelProfile` if missing
  - [ ] Review existing `profiles.rs` for pricing data
  - [ ] Add `input_price_per_1m_usd` and `output_price_per_1m_usd` if needed
  - [ ] Update default profiles with current provider pricing

## Phase 3: Budget Manager Implementation

- [ ] **3.1** Implement `BudgetManager` core
  - [ ] Constructor with limit configurations
  - [ ] `check_budget()` method with scope hierarchy
  - [ ] `record_cost()` method to update state after calls
  - [ ] `get_status()` method for UI queries
  - [ ] Thread-safe state with `RwLock<HashMap>`

- [ ] **3.2** Implement `ResetScheduler`
  - [ ] Calculate next reset time from period config
  - [ ] Spawn background tasks for each limit
  - [ ] Implement `reset_limit()` method
  - [ ] Handle timezone-aware resets

- [ ] **3.3** Implement budget persistence (optional)
  - [ ] Define `BudgetStorage` trait
  - [ ] Implement in-memory storage for testing
  - [ ] Consider SQLite storage for persistence across restarts

## Phase 4: Retry Orchestrator Implementation

- [ ] **4.1** Define execution types
  - [ ] `ExecutionRequest` struct with model preference and tokens
  - [ ] `ExecutionResult<T>` struct with attempts, models_tried, duration
  - [ ] `AttemptRecord` for detailed logging
  - [ ] `ExecutionError` enum with all failure modes

- [ ] **4.2** Implement `RetryOrchestrator`
  - [ ] Constructor with policy, backoff, health_manager, metrics, budget
  - [ ] Main `execute()` async method with retry loop
  - [ ] Budget gate check before each attempt
  - [ ] Circuit breaker check via HealthManager
  - [ ] Backoff delay between retries
  - [ ] Failover chain traversal on exhausted retries
  - [ ] Record outcomes to MetricsCollector

- [ ] **4.3** Add rate limit header parsing
  - [ ] Parse `Retry-After` header from API errors
  - [ ] Use in `RateLimitAware` backoff strategy

## Phase 5: Integration

- [ ] **5.1** Update `ModelMatcher`
  - [ ] Add `route_with_budget()` method
  - [ ] Bias toward cheaper models when budget is low
  - [ ] Maintain backward compatibility with existing `route()`

- [ ] **5.2** Update `IntelligentRouter`
  - [ ] Add `execute_intelligent()` method combining all features
  - [ ] Wire up RetryOrchestrator
  - [ ] Wire up BudgetManager pre/post check
  - [ ] Emit events for UI observability

- [ ] **5.3** Update module exports
  - [ ] Add retry, failover, budget modules to `mod.rs`
  - [ ] Export public types via `pub use`

## Phase 6: Configuration

- [ ] **6.1** Add configuration types
  - [ ] `RetryConfig` in `core/src/config/types/dispatcher.rs`
  - [ ] `BudgetConfig` with limits array
  - [ ] Serde deserialization from TOML

- [ ] **6.2** Update configuration loading
  - [ ] Parse `[model_router.retry]` section
  - [ ] Parse `[model_router.budget]` section
  - [ ] Validate configuration at load time
  - [ ] Add default values for optional fields

## Phase 7: UniFFI Exports

- [ ] **7.1** Export budget status types
  - [ ] `BudgetStatusInfo` struct for Swift
  - [ ] `get_budget_status(scope)` function
  - [ ] Event callbacks for budget warnings

- [ ] **7.2** Export retry/failover info
  - [ ] `ExecutionAttemptInfo` for attempt logging
  - [ ] Event callbacks for retry/failover events

## Phase 8: Testing

- [ ] **8.1** Unit tests
  - [ ] RetryPolicy defaults and validation
  - [ ] BackoffStrategy all variants
  - [ ] FailoverChain selection modes
  - [ ] BudgetManager check/record cycle
  - [ ] CostEstimator accuracy

- [ ] **8.2** Integration tests
  - [ ] RetryOrchestrator with mock executor
  - [ ] Full retry → failover flow
  - [ ] Budget enforcement with soft/hard block
  - [ ] Budget reset scheduler

- [ ] **8.3** Edge case tests
  - [ ] Zero budget remaining
  - [ ] All models unhealthy
  - [ ] Timeout during backoff
  - [ ] Concurrent budget updates

## Phase 9: Documentation

- [ ] **9.1** Update module documentation
  - [ ] Doc comments for all public types
  - [ ] Usage examples in module-level docs
  - [ ] Error handling guidance

- [ ] **9.2** Update ARCHITECTURE.md
  - [ ] Add retry/failover section
  - [ ] Add budget management section
  - [ ] Update component diagram

## Verification

- [ ] All 71+ existing model_router tests pass
- [ ] New tests achieve >80% coverage of new code
- [ ] `cargo clippy` passes with no warnings
- [ ] Configuration loads correctly from example TOML
- [ ] Budget enforcement blocks requests at limit
- [ ] Retry succeeds after transient failures
- [ ] Failover activates when primary model is unhealthy
