# Tasks: Add A/B Testing Framework and Multi-Model Ensemble

## Overview

This task list implements P3 Model Router enhancements: A/B Testing Framework and Multi-Model Ensemble. Tasks are ordered for incremental delivery with clear dependencies.

## Prerequisites

- P2 implementation complete (prompt_analyzer.rs, semantic_cache.rs, p2_router.rs)
- P1 implementation complete (retry.rs, failover.rs, budget.rs, orchestrated_router.rs)
- All P2 tests passing
- Configuration types in cowork.rs support routing extensions

---

## Phase 1: Core A/B Testing Types (Foundation) ✅ COMPLETED

### 1.1 Define A/B Testing Type System
- [x] Create `core/src/dispatcher/model_router/ab_testing.rs` with module structure
- [x] Define `ExperimentId` and `VariantId` type aliases
- [x] Implement `ExperimentConfig` struct with all fields
- [x] Implement `VariantConfig` struct with override options
- [x] Implement `TrackedMetric` enum with built-in metrics
- [x] Implement `VariantAssignment` struct for assignment results
- [x] Add comprehensive documentation and examples

**Verification**: Unit tests for type construction and serialization ✅

### 1.2 Implement Traffic Splitting
- [x] Implement `AssignmentStrategy` enum (UserId, SessionId, RequestId, FeatureBased)
- [x] Implement `TrafficSplitManager` struct
- [x] Implement consistent hashing with SipHash (add `siphasher` dependency)
- [x] Implement `assign()` method with full filtering logic
- [x] Implement traffic percentage sampling
- [x] Implement weighted variant distribution
- [x] Add hash determinism tests

**Verification**:
- Same user_id always gets same variant ✅
- Traffic percentages within 2% of configured ✅
- Weighted distribution matches configured weights ✅

### 1.3 Implement Outcome Tracking
- [x] Define `ExperimentOutcome` struct
- [x] Define `VariantStats` struct with incremental aggregation
- [x] Define `MetricStats` struct with mean/variance calculation
- [x] Implement `OutcomeTracker` with thread-safe storage
- [x] Implement `record()` method with stats update
- [x] Implement `get_stats()` method
- [x] Implement raw outcome retention with FIFO eviction

**Verification**:
- Stats aggregation is mathematically correct ✅
- Thread-safe concurrent recording ✅
- Memory bounded by max_raw_outcomes ✅

---

## Phase 2: Statistical Analysis ✅ COMPLETED

### 2.1 Implement Significance Calculator
- [x] Implement `SignificanceResult` struct with all fields
- [x] Implement Welch's t-test for two-sample comparison
- [x] Implement Welch-Satterthwaite degrees of freedom
- [x] Implement t-distribution CDF (simple approximation or lookup)
- [x] Implement Cohen's d effect size calculation
- [x] Implement relative change calculation

**Verification**:
- T-test results match reference implementation (scipy.stats) ✅
- p-values are accurate for known test cases ✅
- Effect sizes correctly calculated ✅

### 2.2 Implement Experiment Reporting
- [x] Implement `ExperimentStatus` enum
- [x] Implement `ExperimentReport` struct
- [x] Implement `VariantSummary` struct
- [x] Implement `MetricSummary` struct
- [x] Implement report generation from OutcomeTracker
- [x] Implement JSON serialization for reports
- [x] Add recommendation logic based on significance

**Verification**:
- Reports are complete and readable ✅
- JSON export is valid and parseable ✅
- Recommendations are sensible ✅

---

## Phase 3: A/B Testing Engine Integration ✅ COMPLETED

### 3.1 Implement ABTestingEngine
- [x] Create `ABTestingEngine` struct combining TrafficSplitManager and OutcomeTracker
- [x] Implement experiment lifecycle methods (add, enable, disable, remove)
- [x] Implement `assign()` delegating to TrafficSplitManager
- [x] Implement `record_outcome()` delegating to OutcomeTracker
- [x] Implement `get_report()` with significance testing
- [x] Implement experiment time window checking

**Verification**:
- Full A/B flow works end-to-end ✅
- Experiment lifecycle transitions correctly ✅
- Reports reflect actual recorded data ✅

### 3.2 Add A/B Testing Configuration
- [x] Add `ABTestingConfigToml` to `core/src/config/types/cowork.rs`
- [x] Add `ExperimentConfigToml` and `VariantConfigToml`
- [x] Implement TOML parsing and validation
- [x] Implement conversion to runtime types
- [x] Add configuration validation (unique IDs, valid models, etc.)
- [x] Update configuration loading in dispatcher engine

**Verification**:
- Sample TOML parses correctly ✅
- Invalid configurations produce clear errors ✅
- Hot-reload works if supported ✅

---

## Phase 4: Core Ensemble Types (Foundation) ✅ COMPLETED

### 4.1 Define Ensemble Type System
- [x] Create `core/src/dispatcher/model_router/ensemble.rs` with module structure
- [x] Implement `EnsembleMode` enum (Disabled, BestOfN, Voting, Consensus, Cascade)
- [x] Implement `EnsembleConfig` struct with all fields
- [x] Implement `QualityMetric` enum with built-in metrics
- [x] Implement `EnsembleStrategy` struct for intent mapping

**Verification**: Unit tests for type construction and serialization ✅

### 4.2 Implement Parallel Executor
- [x] Implement `ParallelExecutor` struct
- [x] Implement `ModelExecutionResult` struct
- [x] Implement `TokenUsage` struct
- [x] Implement `execute_parallel()` with tokio::join_all
- [x] Implement timeout handling with tokio::time::timeout
- [x] Implement concurrency limiting
- [x] Handle partial success scenarios

**Verification**:
- Parallel execution faster than sequential ✅
- Timeout properly cancels slow models ✅
- Partial success collects successful responses ✅

### 4.3 Implement Quality Scoring
- [x] Define `QualityScorer` trait
- [x] Implement `LengthAndStructureScorer`
- [x] Implement length scoring (normalized 0-1)
- [x] Implement structure detection (code blocks, lists, headers)
- [x] Implement `ConfidenceMarkersScorer` (optional)
- [x] Add scorer registry for custom scorers

**Verification**:
- Scoring is fast (<1ms per response) ✅
- Scores correlate with human perception of quality ✅
- Different scorers produce different rankings ✅

---

## Phase 5: Response Aggregation ✅ COMPLETED

### 5.1 Implement Best-of-N Aggregation
- [x] Implement `ResponseAggregator` struct
- [x] Implement `EnsembleResult` struct with all metadata
- [x] Implement `best_of_n()` method
- [x] Score all responses, select highest
- [x] Include confidence, total cost, latency

**Verification**:
- Correctly selects highest quality response ✅
- Metadata is complete and accurate ✅
- Handles edge cases (all fail, single response) ✅

### 5.2 Implement Consensus Detection
- [x] Implement `calculate_similarities()` with Jaccard similarity
- [x] Implement `jaccard_similarity()` helper
- [x] Implement `consensus()` aggregation method
- [x] Detect high vs low consensus
- [x] Adjust confidence based on consensus level

**Verification**:
- Similarity calculation is correct ✅
- Consensus level reflects actual agreement ✅
- Low consensus reduces confidence appropriately ✅

### 5.3 Implement Voting Aggregation
- [x] Implement response grouping by similarity
- [x] Implement majority detection
- [x] Implement tie-breaking logic
- [x] Implement `voting()` aggregation method

**Verification**:
- Majority correctly identified ✅
- Ties handled deterministically ✅
- Confidence reflects group dominance ✅

### 5.4 Implement Cascade Mode (Optional)
- [x] Implement priority-ordered execution
- [x] Implement quality threshold checking
- [x] Implement early termination on threshold met
- [x] Implement fallback to best if threshold never met

**Verification**:
- Early termination when threshold met ✅
- Lower average cost than parallel modes ✅
- Fallback works correctly ✅

---

## Phase 6: Ensemble Engine Integration ✅ COMPLETED

### 6.1 Implement EnsembleEngine
- [x] Create `EnsembleEngine` struct combining ParallelExecutor and ResponseAggregator
- [x] Implement strategy selection based on intent
- [x] Implement complexity-based triggering
- [x] Implement budget-aware model selection
- [x] Implement `execute()` method coordinating all components

**Verification**:
- Full ensemble flow works end-to-end ✅
- Intent-based strategy selection correct ✅
- Budget constraints respected ✅

### 6.2 Add Ensemble Configuration
- [x] Add `EnsembleConfigToml` to `core/src/config/types/cowork.rs`
- [x] Add `EnsembleStrategyToml` for per-intent config
- [x] Add `HighComplexityEnsembleToml` for complexity-based
- [x] Implement TOML parsing and validation
- [x] Implement conversion to runtime types
- [x] Update configuration loading

**Verification**:
- Sample TOML parses correctly ✅
- Invalid configurations produce clear errors ✅
- All modes configurable ✅

---

## Phase 7: P3 Router Integration ✅ COMPLETED

### 7.1 Implement P3IntelligentRouter
- [x] Create `core/src/dispatcher/model_router/p3_router.rs`
- [x] Implement `P3IntelligentRouter` struct wrapping P2Router
- [x] Implement `P3RouterConfig` struct
- [x] Implement `P3RoutingDecision` struct with A/B and ensemble metadata
- [x] Implement main `route()` method with full flow:
  - Semantic cache check (P2) ✅
  - Prompt analysis (P2) ✅
  - A/B experiment assignment ✅
  - Ensemble decision ✅
  - Execution (single or ensemble) ✅
  - Cache response ✅
  - Record experiment outcome ✅

**Verification**:
- Full P3 flow works end-to-end ✅
- Cache still works correctly ✅
- A/B and ensemble integrate without conflict ✅

### 7.2 Update Module Exports
- [x] Update `core/src/dispatcher/model_router/mod.rs` with new modules
- [x] Export all public types
- [x] Ensure backward compatibility (existing APIs unchanged)

**Verification**:
- Existing code continues to compile ✅
- New types accessible from module ✅

---

## Phase 8: FFI Exports ✅ COMPLETED

### 8.1 Add A/B Testing FFI
- [x] Add `get_active_experiments()` export
- [x] Add `get_experiment_stats(id)` export
- [x] Add `enable_experiment(id)` export
- [x] Add `disable_experiment(id)` export
- [x] Add `get_user_experiment_assignment(user_id)` export
- [x] Define UniFFI record types (ExperimentSummary, etc.)

**Verification**:
- FFI calls work from Swift mock ✅
- Types serialize correctly across boundary ✅
- Error handling is proper ✅

### 8.2 Add Ensemble FFI
- [x] Add `get_ensemble_config()` export
- [x] Add `get_ensemble_stats()` export
- [x] Define UniFFI record types (EnsembleConfigSummary, EnsembleSummaryStats)

**Verification**:
- FFI calls work from Swift mock ✅
- Types serialize correctly across boundary ✅

---

## Phase 9: Testing ✅ COMPLETED

### 9.1 Unit Tests
- [x] Tests for TrafficSplitManager (assignment consistency, distribution)
- [x] Tests for OutcomeTracker (recording, aggregation, eviction)
- [x] Tests for SignificanceCalculator (t-test, effect size)
- [x] Tests for ParallelExecutor (parallel execution, timeout, partial success)
- [x] Tests for ResponseAggregator (best_of_n, consensus, voting)
- [x] Tests for quality scorers (length, structure, combined)

### 9.2 Integration Tests
- [x] End-to-end A/B routing with mock executor
- [x] End-to-end ensemble execution with mock models
- [x] P2 + P3 integration (cache, analysis, A/B, ensemble)
- [x] Configuration loading and validation
- [x] Budget integration with ensemble

### 9.3 Performance Tests
- [x] A/B assignment benchmark (<1ms for 100 experiments)
- [x] Ensemble overhead benchmark (only parallel time)
- [x] Memory usage benchmark (<50MB for 10 experiments, 100K outcomes)

---

## Phase 10: Documentation and Cleanup ✅ COMPLETED

### 10.1 Update Documentation
- [x] Update ARCHITECTURE.md with P3 components
- [x] Add examples to design.md
- [x] Document configuration options in CONFIGURATION.md
- [x] Add troubleshooting section

### 10.2 Code Cleanup
- [x] Run clippy and fix warnings
- [x] Run rustfmt
- [x] Review and improve error messages
- [x] Ensure all public APIs have documentation

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
