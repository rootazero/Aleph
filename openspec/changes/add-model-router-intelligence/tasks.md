# Tasks: Model Router Intelligence

## 1. Runtime Metrics - Data Structures
- [ ] 1.1 Create `metrics.rs` with `CallRecord`, `CallOutcome`, `UserFeedback`
- [ ] 1.2 Add `ModelMetrics` with `LatencyStats`, `CostStats`, `ErrorDistribution`
- [ ] 1.3 Add `MultiWindowMetrics` with `WindowConfig`
- [ ] 1.4 Add `RateLimitState` for rate limit tracking
- [ ] 1.5 Write unit tests for data structures

## 2. Runtime Metrics - Collector
- [ ] 2.1 Create `collector.rs` with `MetricsCollector` trait
- [ ] 2.2 Implement `RingBuffer<T>` for fixed-size storage
- [ ] 2.3 Implement `ExponentialDecayCounter` for success rate
- [ ] 2.4 Implement `HybridMetricsCollector` with async channel
- [ ] 2.5 Add background tasks (aggregation, persistence)
- [ ] 2.6 Write unit tests for collector

## 3. Runtime Metrics - Scoring
- [ ] 3.1 Create `scoring.rs` with `ScoringConfig`
- [ ] 3.2 Implement `DynamicScorer` with scoring methods
- [ ] 3.3 Add latency, cost, reliability, quality score computation
- [ ] 3.4 Add penalty factor computation (consecutive failures, etc.)
- [ ] 3.5 Add static scoring fallback for cold start
- [ ] 3.6 Write unit tests for scorer

## 4. Runtime Metrics - Storage
- [ ] 4.1 Create `storage.rs` with `MetricsStorage` trait
- [ ] 4.2 Implement `SqliteMetricsStorage` with schema
- [ ] 4.3 Add save/load/append methods
- [ ] 4.4 Add index for efficient queries
- [ ] 4.5 Write unit tests for storage

## 5. Health Check - Data Structures
- [ ] 5.1 Create `health.rs` with `HealthStatus` enum
- [ ] 5.2 Add `ModelHealth` with all state fields
- [ ] 5.3 Add `DegradationReason` and `UnhealthyReason` enums
- [ ] 5.4 Add `CircuitBreakerState` and `CircuitState`
- [ ] 5.5 Add `RateLimitInfo` from headers parsing
- [ ] 5.6 Add `HealthError` and `ErrorType`
- [ ] 5.7 Write unit tests for data structures

## 6. Health Check - State Machine
- [ ] 6.1 Create `transition.rs` with `HealthTransitionEngine`
- [ ] 6.2 Implement success handling with state transitions
- [ ] 6.3 Implement failure handling with state transitions
- [ ] 6.4 Implement circuit breaker logic (open, half-open, close)
- [ ] 6.5 Add degradation detection (high latency, near rate limit)
- [ ] 6.6 Add recovery detection (consecutive successes)
- [ ] 6.7 Write unit tests for all state transitions

## 7. Health Check - Manager
- [ ] 7.1 Create `health_manager.rs` with `HealthManager`
- [ ] 7.2 Implement `record_call()` method
- [ ] 7.3 Implement `can_call()` with `CanCallResult`
- [ ] 7.4 Add health event broadcasting
- [ ] 7.5 Add background task for half-open transitions
- [ ] 7.6 Add manual status override methods
- [ ] 7.7 Write unit tests for manager

## 8. Health Check - Active Probing
- [ ] 8.1 Create `prober.rs` with `HealthProber`
- [ ] 8.2 Add `ProbeEndpoint` configuration
- [ ] 8.3 Implement health endpoint probing
- [ ] 8.4 Implement minimal request probing
- [ ] 8.5 Integrate with `HealthManager` background tasks
- [ ] 8.6 Write unit tests for prober

## 9. ModelMatcher Integration
- [ ] 9.1 Add `route_with_health()` method
- [ ] 9.2 Add `route_with_metrics()` method
- [ ] 9.3 Add `route_intelligent()` combined method
- [ ] 9.4 Add exploratory routing (epsilon-greedy)
- [ ] 9.5 Update existing routing tests
- [ ] 9.6 Write integration tests

## 10. Configuration
- [ ] 10.1 Add `MetricsConfig` to `config/types/cowork.rs`
- [ ] 10.2 Add `HealthConfig` to `config/types/cowork.rs`
- [ ] 10.3 Add `ScoringConfig` defaults
- [ ] 10.4 Add `WindowConfig` defaults
- [ ] 10.5 Add `CircuitBreakerConfig`
- [ ] 10.6 Add `ProbeConfig`
- [ ] 10.7 Update TOML parsing and validation
- [ ] 10.8 Write config tests

## 11. UniFFI Exports
- [ ] 11.1 Add `ModelMetricsFfi` and `ModelMetricsSummaryFfi`
- [ ] 11.2 Add `ModelHealthFfi` and `ModelHealthSummaryFfi`
- [ ] 11.3 Add `HealthStatusFfi` and `CircuitStateFfi`
- [ ] 11.4 Add `UserFeedbackFfi`
- [ ] 11.5 Export `cowork_get_model_metrics()`
- [ ] 11.6 Export `cowork_get_all_metrics_summary()`
- [ ] 11.7 Export `cowork_get_model_health()`
- [ ] 11.8 Export `cowork_get_all_health_summary()`
- [ ] 11.9 Export `cowork_set_model_status()`
- [ ] 11.10 Export `cowork_record_feedback()`
- [ ] 11.11 Generate Swift bindings

## 12. Swift UI - Health View
- [ ] 12.1 Create `ModelHealthView.swift` list view
- [ ] 12.2 Create `ModelHealthRow.swift` row component
- [ ] 12.3 Create `ModelHealthDetailView.swift` detail sheet
- [ ] 12.4 Add status indicators (emoji/color)
- [ ] 12.5 Add manual override controls
- [ ] 12.6 Integrate into Settings

## 13. Swift UI - Metrics View
- [ ] 13.1 Create `ModelMetricsView.swift` list view
- [ ] 13.2 Create `ModelMetricsDetailView.swift` detail sheet
- [ ] 13.3 Add latency/cost statistics display
- [ ] 13.4 Add success rate visualization
- [ ] 13.5 Integrate into Settings

## 14. Module Integration
- [ ] 14.1 Update `model_router/mod.rs` exports
- [ ] 14.2 Initialize metrics collector in CoworkEngine
- [ ] 14.3 Initialize health manager in CoworkEngine
- [ ] 14.4 Wire up call recording after task execution
- [ ] 14.5 Update task executor to use intelligent routing

## 15. Documentation
- [ ] 15.1 Update `docs/COWORK.md` with metrics section
- [ ] 15.2 Update `docs/COWORK.md` with health section
- [ ] 15.3 Add configuration examples
- [ ] 15.4 Add troubleshooting guide

## 16. Testing
- [ ] 16.1 Write end-to-end test for metrics flow
- [ ] 16.2 Write end-to-end test for health flow
- [ ] 16.3 Write integration test for intelligent routing
- [ ] 16.4 Manual testing on macOS
- [ ] 16.5 Verify SQLite persistence across restarts

## Estimated Timeline

| Phase | Tasks | Days |
|-------|-------|------|
| Phase 1: Metrics Data | 1.x | 2 |
| Phase 2: Metrics Collector | 2.x, 3.x, 4.x | 4 |
| Phase 3: Health Data | 5.x | 1 |
| Phase 4: Health State Machine | 6.x | 2 |
| Phase 5: Health Manager | 7.x, 8.x | 2 |
| Phase 6: Integration | 9.x, 10.x, 14.x | 3 |
| Phase 7: UniFFI + UI | 11.x, 12.x, 13.x | 4 |
| Phase 8: Docs & Testing | 15.x, 16.x | 2 |
| **Total** | | **~20 days** |
