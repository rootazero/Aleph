# Tasks: Add A/B Testing Framework and Multi-Model Ensemble

## Overview

This task list implements P3 Model Router enhancements: A/B Testing Framework and Multi-Model Ensemble. Tasks are ordered for incremental delivery with clear dependencies.

## Prerequisites

- P2 implementation complete (prompt_analyzer.rs, semantic_cache.rs, p2_router.rs)
- P1 implementation complete (retry.rs, failover.rs, budget.rs, orchestrated_router.rs)
- All P2 tests passing
- Configuration types in cowork.rs support routing extensions

---

## Phase 1: Core A/B Testing Types (Foundation)

### 1.1 Define A/B Testing Type System
- [ ] Create `core/src/dispatcher/model_router/ab_testing.rs` with module structure
- [ ] Define `ExperimentId` and `VariantId` type aliases
- [ ] Implement `ExperimentConfig` struct with all fields
- [ ] Implement `VariantConfig` struct with override options
- [ ] Implement `TrackedMetric` enum with built-in metrics
- [ ] Implement `VariantAssignment` struct for assignment results
- [ ] Add comprehensive documentation and examples

**Verification**: Unit tests for type construction and serialization

### 1.2 Implement Traffic Splitting
- [ ] Implement `AssignmentStrategy` enum (UserId, SessionId, RequestId, FeatureBased)
- [ ] Implement `TrafficSplitManager` struct
- [ ] Implement consistent hashing with SipHash (add `siphasher` dependency)
- [ ] Implement `assign()` method with full filtering logic
- [ ] Implement traffic percentage sampling
- [ ] Implement weighted variant distribution
- [ ] Add hash determinism tests

**Verification**:
- Same user_id always gets same variant
- Traffic percentages within 2% of configured
- Weighted distribution matches configured weights

### 1.3 Implement Outcome Tracking
- [ ] Define `ExperimentOutcome` struct
- [ ] Define `VariantStats` struct with incremental aggregation
- [ ] Define `MetricStats` struct with mean/variance calculation
- [ ] Implement `OutcomeTracker` with thread-safe storage
- [ ] Implement `record()` method with stats update
- [ ] Implement `get_stats()` method
- [ ] Implement raw outcome retention with FIFO eviction

**Verification**:
- Stats aggregation is mathematically correct
- Thread-safe concurrent recording
- Memory bounded by max_raw_outcomes

---

## Phase 2: Statistical Analysis

### 2.1 Implement Significance Calculator
- [ ] Implement `SignificanceResult` struct with all fields
- [ ] Implement Welch's t-test for two-sample comparison
- [ ] Implement Welch-Satterthwaite degrees of freedom
- [ ] Implement t-distribution CDF (simple approximation or lookup)
- [ ] Implement Cohen's d effect size calculation
- [ ] Implement relative change calculation

**Verification**:
- T-test results match reference implementation (scipy.stats)
- p-values are accurate for known test cases
- Effect sizes correctly calculated

### 2.2 Implement Experiment Reporting
- [ ] Implement `ExperimentStatus` enum
- [ ] Implement `ExperimentReport` struct
- [ ] Implement `VariantSummary` struct
- [ ] Implement `MetricSummary` struct
- [ ] Implement report generation from OutcomeTracker
- [ ] Implement JSON serialization for reports
- [ ] Add recommendation logic based on significance

**Verification**:
- Reports are complete and readable
- JSON export is valid and parseable
- Recommendations are sensible

---

## Phase 3: A/B Testing Engine Integration

### 3.1 Implement ABTestingEngine
- [ ] Create `ABTestingEngine` struct combining TrafficSplitManager and OutcomeTracker
- [ ] Implement experiment lifecycle methods (add, enable, disable, remove)
- [ ] Implement `assign()` delegating to TrafficSplitManager
- [ ] Implement `record_outcome()` delegating to OutcomeTracker
- [ ] Implement `get_report()` with significance testing
- [ ] Implement experiment time window checking

**Verification**:
- Full A/B flow works end-to-end
- Experiment lifecycle transitions correctly
- Reports reflect actual recorded data

### 3.2 Add A/B Testing Configuration
- [ ] Add `ABTestingConfigToml` to `core/src/config/types/cowork.rs`
- [ ] Add `ExperimentConfigToml` and `VariantConfigToml`
- [ ] Implement TOML parsing and validation
- [ ] Implement conversion to runtime types
- [ ] Add configuration validation (unique IDs, valid models, etc.)
- [ ] Update configuration loading in dispatcher engine

**Verification**:
- Sample TOML parses correctly
- Invalid configurations produce clear errors
- Hot-reload works if supported

---

## Phase 4: Core Ensemble Types (Foundation)

### 4.1 Define Ensemble Type System
- [ ] Create `core/src/dispatcher/model_router/ensemble.rs` with module structure
- [ ] Implement `EnsembleMode` enum (Disabled, BestOfN, Voting, Consensus, Cascade)
- [ ] Implement `EnsembleConfig` struct with all fields
- [ ] Implement `QualityMetric` enum with built-in metrics
- [ ] Implement `EnsembleStrategy` struct for intent mapping

**Verification**: Unit tests for type construction and serialization

### 4.2 Implement Parallel Executor
- [ ] Implement `ParallelExecutor` struct
- [ ] Implement `ModelExecutionResult` struct
- [ ] Implement `TokenUsage` struct
- [ ] Implement `execute_parallel()` with tokio::join_all
- [ ] Implement timeout handling with tokio::time::timeout
- [ ] Implement concurrency limiting
- [ ] Handle partial success scenarios

**Verification**:
- Parallel execution faster than sequential
- Timeout properly cancels slow models
- Partial success collects successful responses

### 4.3 Implement Quality Scoring
- [ ] Define `QualityScorer` trait
- [ ] Implement `LengthAndStructureScorer`
- [ ] Implement length scoring (normalized 0-1)
- [ ] Implement structure detection (code blocks, lists, headers)
- [ ] Implement `ConfidenceMarkersScorer` (optional)
- [ ] Add scorer registry for custom scorers

**Verification**:
- Scoring is fast (<1ms per response)
- Scores correlate with human perception of quality
- Different scorers produce different rankings

---

## Phase 5: Response Aggregation

### 5.1 Implement Best-of-N Aggregation
- [ ] Implement `ResponseAggregator` struct
- [ ] Implement `EnsembleResult` struct with all metadata
- [ ] Implement `best_of_n()` method
- [ ] Score all responses, select highest
- [ ] Include confidence, total cost, latency

**Verification**:
- Correctly selects highest quality response
- Metadata is complete and accurate
- Handles edge cases (all fail, single response)

### 5.2 Implement Consensus Detection
- [ ] Implement `calculate_similarities()` with Jaccard similarity
- [ ] Implement `jaccard_similarity()` helper
- [ ] Implement `consensus()` aggregation method
- [ ] Detect high vs low consensus
- [ ] Adjust confidence based on consensus level

**Verification**:
- Similarity calculation is correct
- Consensus level reflects actual agreement
- Low consensus reduces confidence appropriately

### 5.3 Implement Voting Aggregation
- [ ] Implement response grouping by similarity
- [ ] Implement majority detection
- [ ] Implement tie-breaking logic
- [ ] Implement `voting()` aggregation method

**Verification**:
- Majority correctly identified
- Ties handled deterministically
- Confidence reflects group dominance

### 5.4 Implement Cascade Mode (Optional)
- [ ] Implement priority-ordered execution
- [ ] Implement quality threshold checking
- [ ] Implement early termination on threshold met
- [ ] Implement fallback to best if threshold never met

**Verification**:
- Early termination when threshold met
- Lower average cost than parallel modes
- Fallback works correctly

---

## Phase 6: Ensemble Engine Integration

### 6.1 Implement EnsembleEngine
- [ ] Create `EnsembleEngine` struct combining ParallelExecutor and ResponseAggregator
- [ ] Implement strategy selection based on intent
- [ ] Implement complexity-based triggering
- [ ] Implement budget-aware model selection
- [ ] Implement `execute()` method coordinating all components

**Verification**:
- Full ensemble flow works end-to-end
- Intent-based strategy selection correct
- Budget constraints respected

### 6.2 Add Ensemble Configuration
- [ ] Add `EnsembleConfigToml` to `core/src/config/types/cowork.rs`
- [ ] Add `EnsembleStrategyToml` for per-intent config
- [ ] Add `HighComplexityEnsembleToml` for complexity-based
- [ ] Implement TOML parsing and validation
- [ ] Implement conversion to runtime types
- [ ] Update configuration loading

**Verification**:
- Sample TOML parses correctly
- Invalid configurations produce clear errors
- All modes configurable

---

## Phase 7: P3 Router Integration

### 7.1 Implement P3IntelligentRouter
- [ ] Create `core/src/dispatcher/model_router/p3_router.rs`
- [ ] Implement `P3IntelligentRouter` struct wrapping P2Router
- [ ] Implement `P3RouterConfig` struct
- [ ] Implement `P3RoutingDecision` struct with A/B and ensemble metadata
- [ ] Implement main `route()` method with full flow:
  - Semantic cache check (P2)
  - Prompt analysis (P2)
  - A/B experiment assignment
  - Ensemble decision
  - Execution (single or ensemble)
  - Cache response
  - Record experiment outcome

**Verification**:
- Full P3 flow works end-to-end
- Cache still works correctly
- A/B and ensemble integrate without conflict

### 7.2 Update Module Exports
- [ ] Update `core/src/dispatcher/model_router/mod.rs` with new modules
- [ ] Export all public types
- [ ] Ensure backward compatibility (existing APIs unchanged)

**Verification**:
- Existing code continues to compile
- New types accessible from module

---

## Phase 8: FFI Exports

### 8.1 Add A/B Testing FFI
- [ ] Add `get_active_experiments()` export
- [ ] Add `get_experiment_stats(id)` export
- [ ] Add `enable_experiment(id)` export
- [ ] Add `disable_experiment(id)` export
- [ ] Add `get_user_experiment_assignment(user_id)` export
- [ ] Define UniFFI record types (ExperimentSummary, etc.)

**Verification**:
- FFI calls work from Swift mock
- Types serialize correctly across boundary
- Error handling is proper

### 8.2 Add Ensemble FFI
- [ ] Add `get_ensemble_config()` export
- [ ] Add `get_ensemble_stats()` export
- [ ] Define UniFFI record types (EnsembleConfigSummary, EnsembleSummaryStats)

**Verification**:
- FFI calls work from Swift mock
- Types serialize correctly across boundary

---

## Phase 9: Testing

### 9.1 Unit Tests
- [ ] Tests for TrafficSplitManager (assignment consistency, distribution)
- [ ] Tests for OutcomeTracker (recording, aggregation, eviction)
- [ ] Tests for SignificanceCalculator (t-test, effect size)
- [ ] Tests for ParallelExecutor (parallel execution, timeout, partial success)
- [ ] Tests for ResponseAggregator (best_of_n, consensus, voting)
- [ ] Tests for quality scorers (length, structure, combined)

### 9.2 Integration Tests
- [ ] End-to-end A/B routing with mock executor
- [ ] End-to-end ensemble execution with mock models
- [ ] P2 + P3 integration (cache, analysis, A/B, ensemble)
- [ ] Configuration loading and validation
- [ ] Budget integration with ensemble

### 9.3 Performance Tests
- [ ] A/B assignment benchmark (<1ms for 100 experiments)
- [ ] Ensemble overhead benchmark (only parallel time)
- [ ] Memory usage benchmark (<50MB for 10 experiments, 100K outcomes)

---

## Phase 10: Documentation and Cleanup

### 10.1 Update Documentation
- [ ] Update ARCHITECTURE.md with P3 components
- [ ] Add examples to design.md
- [ ] Document configuration options in CONFIGURATION.md
- [ ] Add troubleshooting section

### 10.2 Code Cleanup
- [ ] Run clippy and fix warnings
- [ ] Run rustfmt
- [ ] Review and improve error messages
- [ ] Ensure all public APIs have documentation

---

## Dependencies

```
Phase 1 (A/B Types) ─────┬─────> Phase 2 (Statistics) ────> Phase 3 (A/B Engine)
                         │                                        │
                         │                                        │
                         v                                        v
Phase 4 (Ensemble Types) ─> Phase 5 (Aggregation) ────> Phase 6 (Ensemble Engine)
                                                                  │
                                                                  v
                                                        Phase 7 (P3 Router) ────> Phase 8 (FFI)
                                                                  │
                                                                  v
                                                        Phase 9 (Testing) ────> Phase 10 (Docs)
```

## Parallelization Opportunities

- **Phase 1 & 4**: A/B types and Ensemble types can be developed in parallel
- **Phase 2 & 5**: Statistics and Aggregation can be developed in parallel
- **Phase 3 & 6**: A/B Engine and Ensemble Engine can be developed in parallel
- **Phase 8**: FFI exports can be done incrementally as engines complete
- **Phase 9**: Tests can be written alongside implementation

---

## Estimated Effort

| Phase | Tasks | Complexity | Notes |
|-------|-------|------------|-------|
| 1 | 3 | Medium | Core A/B types, traffic splitting |
| 2 | 2 | Medium | Statistical analysis |
| 3 | 2 | Low | Engine integration |
| 4 | 2 | Medium | Core Ensemble types |
| 5 | 4 | Medium-High | Aggregation strategies |
| 6 | 2 | Low | Engine integration |
| 7 | 2 | Medium | P3 Router |
| 8 | 2 | Low | FFI exports |
| 9 | 3 | Medium | Comprehensive testing |
| 10 | 2 | Low | Documentation |

**Total**: ~24 tasks across 10 phases
