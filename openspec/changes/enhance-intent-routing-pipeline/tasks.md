# Tasks: Enhanced Intent Routing Pipeline

## Overview

Implementation tasks for the enhanced intent routing pipeline. Tasks are ordered by dependency and grouped by phase.

**Estimated Total Effort**: 3 weeks
**Priority**: High (improves core user experience)

---

## Phase 1: Foundation Types and Structures

### Task 1.1: Define Core Types
**File**: `core/src/routing/intent.rs` (new)

- [x] Define `IntentSignal` struct
- [x] Define `AggregatedIntent` struct
- [x] Define `IntentAction` enum
- [x] Define `ParameterRequirement` struct
- [x] Define `CalibratedSignal` struct
- [x] Define `CalibrationFactor` struct
- [x] Add unit tests for type construction and serialization
- [x] Export types from `routing/mod.rs`

**Validation**: Types compile and tests pass

### Task 1.2: Define Pipeline Configuration
**File**: `core/src/routing/config.rs` (new)

- [x] Define `PipelineConfig` struct
- [x] Define `CacheConfig` struct
- [x] Define `LayerConfig` struct with execution mode
- [x] Define `ToolConfidenceConfig` struct
- [x] Add TOML deserialization
- [x] Add default implementations
- [x] Update `Config` struct in `config/mod.rs` to include pipeline config
- [x] Add unit tests for config parsing

**Validation**: Config loads from TOML correctly

### Task 1.3: Define Pipeline Result Types
**File**: `core/src/routing/result.rs` (new)

- [x] Define `PipelineResult` enum
- [x] Define `ResumeResult` struct
- [x] Define `ClarificationError` enum
- [x] Implement Display and Error traits
- [x] Add conversion to/from existing `DispatcherResult`

**Validation**: Types are compatible with existing error handling

---

## Phase 2: Intent Cache Implementation

### Task 2.1: Implement Cache Core
**File**: `core/src/routing/cache.rs` (new)

- [x] Implement `IntentCache` struct with LRU backing
- [x] Implement `CachedIntent` struct
- [x] Implement hash function for normalized input
- [x] Implement `get()` with time decay
- [x] Implement `put()` for new entries
- [x] Add `record_success()` method
- [x] Add `record_failure()` method
- [x] Add `clear()` and `size()` methods
- [x] Add thread-safe access with `Arc<RwLock<>>`

**Validation**: Unit tests for all cache operations

### Task 2.2: Implement Cache Metrics
**File**: `core/src/routing/cache.rs`

- [x] Define `CacheMetrics` struct
- [x] Track hit/miss counts
- [x] Track eviction counts
- [x] Track average hit confidence
- [x] Add method to export metrics

**Validation**: Metrics update correctly during cache operations

### Task 2.3: Integrate Cache with Config
**File**: `core/src/routing/cache.rs`

- [x] Load cache config from `PipelineConfig`
- [x] Apply TTL from config
- [x] Apply max_size from config
- [x] Apply decay_half_life from config
- [x] Add config reload support (clear cache on config change)

**Validation**: Config changes apply to cache behavior

---

## Phase 3: Confidence Calibration System

### Task 3.1: Implement Calibrator Core
**File**: `core/src/routing/calibrator.rs` (new)

- [x] Implement `ConfidenceCalibrator` struct
- [x] Implement `calibrate()` main method
- [x] Implement layer-specific calibration
- [x] Implement tool-specific calibration
- [x] Implement context-based calibration
- [x] Add calibration factor tracking

**Validation**: Calibration produces valid confidence values [0,1]

### Task 3.2: Implement History-Based Calibration
**File**: `core/src/routing/calibrator.rs`

- [x] Define `CalibrationHistory` struct
- [x] Store success/failure patterns per tool
- [x] Implement `apply_history_boost()`
- [x] Implement history pruning (max entries, TTL)
- [x] Add persistence option (save/load history)

**Validation**: History boost increases confidence for successful patterns

### Task 3.3: Add Tool-Specific Config Loading
**File**: `core/src/routing/calibrator.rs`

- [x] Load per-tool configs from `[[routing.pipeline.tools]]`
- [x] Apply min_threshold per tool
- [x] Apply auto_execute_threshold per tool
- [x] Apply repeat_boost setting per tool
- [x] Add default config for unconfigured tools

**Validation**: Tool-specific configs override global thresholds

---

## Phase 4: Layer Execution Engine

### Task 4.1: Refactor L1 Matcher for Pipeline
**File**: `core/src/routing/l1_regex.rs` (new)

- [x] Create `L1RegexMatcher` wrapper around `SemanticMatcher`
- [x] Implement `match_input()` returning `IntentSignal`
- [x] Add latency tracking
- [x] Ensure L1 only returns for exact command matches
- [x] Add tests for command pattern matching

**Validation**: L1 returns signals with correct confidence

### Task 4.2: Refactor L2 Matcher for Pipeline
**File**: `core/src/routing/l2_semantic.rs` (new)

- [x] Create `L2SemanticMatcher` wrapper
- [x] Implement `match_input()` returning `IntentSignal`
- [x] Include keyword matches in signal
- [x] Add context-aware matching (conversation history)
- [x] Add latency tracking
- [x] Add tests for keyword matching

**Validation**: L2 returns calibrated keyword-based signals

### Task 4.3: Enhance L3 Router for Pipeline
**File**: `core/src/routing/l3_enhanced.rs` (new)

- [x] Create `EnhancedL3Router` wrapping existing `L3Router`
- [x] Add tool pre-filtering based on input
- [x] Implement `route()` returning `IntentSignal`
- [x] Add streaming support (if provider supports)
- [x] Add entity hints from conversation
- [x] Add latency tracking
- [x] Add tests with mock provider

**Validation**: L3 returns properly formatted signals

### Task 4.4: Implement Layer Execution Engine
**File**: `core/src/routing/engine.rs` (new)

- [x] Implement `LayerExecutionEngine` struct
- [x] Implement `execute()` with sequential mode
- [x] Implement `execute()` with parallel mode
- [x] Implement early exit for high-confidence L1
- [x] Add configurable L2-skip-L3 threshold
- [x] Add timeout handling for L3
- [x] Add comprehensive tests

**Validation**: Engine executes layers correctly per configuration

---

## Phase 5: Intent Aggregation

### Task 5.1: Implement Aggregator Core
**File**: `core/src/routing/aggregator.rs` (new)

- [x] Implement `IntentAggregator` struct
- [x] Implement `aggregate()` for multiple signals
- [x] Implement signal sorting by calibrated confidence
- [x] Implement conflict detection
- [x] Add `from_single()` for single-signal case

**Validation**: Aggregation produces valid `AggregatedIntent`

### Task 5.2: Implement Action Determination
**File**: `core/src/routing/aggregator.rs`

- [x] Implement `determine_action()` with thresholds
- [x] Handle Execute action (high confidence)
- [x] Handle RequestConfirmation action (medium confidence)
- [x] Handle GeneralChat action (no match)
- [x] Account for conflicts in action determination

**Validation**: Actions match expected behavior for confidence ranges

### Task 5.3: Implement Parameter Completeness Check
**File**: `core/src/routing/aggregator.rs`

- [x] Implement `find_missing_params()` from tool schema
- [x] Parse JSON Schema for required parameters
- [x] Compare with provided parameters
- [x] Generate `ParameterRequirement` for missing params
- [x] Override action to RequestClarification if params missing

**Validation**: Missing params correctly identified from tool schema

---

## Phase 6: Clarification Flow Integration

### Task 6.1: Implement Clarification Integrator
**File**: `core/src/routing/clarification.rs` (new)

- [x] Implement `ClarificationIntegrator` struct
- [x] Implement `start_clarification()` with session creation
- [x] Store `PendingClarification` with full context
- [x] Generate `ClarificationRequest` for UI
- [x] Add timeout configuration

**Validation**: Clarification requests are created with correct data

### Task 6.2: Implement Resume Flow
**File**: `core/src/routing/clarification.rs`

- [x] Implement `resume()` with user input
- [x] Restore original context from pending
- [x] Augment parameters with user input
- [x] Update intent with new parameters
- [x] Remove clarified parameter from missing list
- [x] Update action based on new state

**Validation**: Resume correctly augments and continues routing

### Task 6.3: Implement Cleanup and Timeout
**File**: `core/src/routing/clarification.rs`

- [x] Implement `cleanup_expired()` method
- [x] Add periodic cleanup task (timer-based)
- [x] Handle timeout in `resume()` gracefully
- [x] Add metrics for timeout rate

**Validation**: Expired clarifications are cleaned up correctly

---

## Phase 7: Pipeline Coordinator

### Task 7.1: Implement Pipeline Coordinator Core
**File**: `core/src/routing/pipeline.rs` (new)

- [x] Implement `IntentRoutingPipeline` struct
- [x] Wire all components (cache, engine, aggregator, calibrator, clarification)
- [x] Implement `process()` main entry point
- [x] Add config loading

**Validation**: Pipeline initializes with all components

### Task 7.2: Implement Fast Path (Cache Hit)
**File**: `core/src/routing/pipeline.rs`

- [x] Check cache before layer execution
- [x] Handle high-confidence cache hits directly
- [x] Bypass layers for cached intents
- [x] Update cache hit metrics

**Validation**: Cache hits skip layer execution

### Task 7.3: Implement Full Flow
**File**: `core/src/routing/pipeline.rs`

- [x] Run L1 always
- [x] Early exit on high-confidence L1
- [x] Run L2/L3 based on execution mode
- [x] Aggregate signals
- [x] Determine action
- [x] Handle clarification flow
- [x] Execute tools
- [x] Record to cache

**Validation**: Full flow produces correct results

### Task 7.4: Implement Event Handler Integration
**File**: `core/src/routing/pipeline.rs`

- [x] Call `on_tool_confirmation()` for confirmation action
- [x] Call `on_clarification_needed()` for clarification action
- [x] Handle user decisions
- [x] Update cache based on outcomes

**Validation**: UI callbacks work correctly

---

## Phase 8: AlephCore Integration

### Task 8.1: Add Pipeline to AlephCore
**File**: `core/src/core.rs`

- [x] Add `IntentRoutingPipeline` field to `AlephCore`
- [x] Initialize pipeline in `new()` based on config
- [x] Add feature flag for pipeline enable/disable

**Validation**: Pipeline initializes with AlephCore

### Task 8.2: Route Through Pipeline
**File**: `core/src/core.rs`

- [x] Modify `process_input()` to use pipeline when enabled
- [x] Build `RoutingContext` from input
- [x] Handle `PipelineResult` variants
- [x] Fall back to dispatcher if pipeline disabled
- [x] Add logging for routing decisions

**Validation**: Requests route through pipeline correctly

### Task 8.3: Handle Pipeline Results
**File**: `core/src/core.rs`

- [x] Handle `Executed` result (return response)
- [x] Handle `PendingClarification` result (trigger UI)
- [x] Handle `Cancelled` result (log and notify)
- [x] Handle `GeneralChat` result (route to AI)

**Validation**: All result types handled correctly

### Task 8.4: Add Clarification Resume Endpoint
**File**: `core/src/core.rs`

- [x] Add `resume_clarification()` method
- [x] Call pipeline's resume flow
- [x] Handle resumed routing result
- [x] Expose via UniFFI

**Validation**: Clarification resume works end-to-end

---

## Phase 9: Testing and Validation

### Task 9.1: Unit Tests
**Files**: Various `*_test.rs` files

- [x] Test IntentCache operations
- [x] Test ConfidenceCalibrator calibration
- [x] Test IntentAggregator aggregation
- [x] Test ClarificationIntegrator flow
- [x] Test LayerExecutionEngine modes
- [x] Test IntentRoutingPipeline end-to-end

**Validation**: All unit tests pass

### Task 9.2: Integration Tests
**File**: `core/src/tests/pipeline_integration.rs` (new)

- [x] Test pipeline with mock providers
- [x] Test cache hit fast path
- [x] Test L1 early exit
- [x] Test full L1→L2→L3 cascade
- [x] Test clarification flow
- [x] Test confirmation flow
- [x] Test timeout handling

**Validation**: Integration tests pass with realistic scenarios

### Task 9.3: Performance Benchmarks
**File**: `core/benches/pipeline_bench.rs` (new)

- [x] Benchmark cache lookup time
- [x] Benchmark L1 matching time
- [x] Benchmark full pipeline latency
- [x] Benchmark memory usage (via concurrent tests)
- [x] Compare with existing dispatcher (via baseline measurements)

**Validation**: Performance meets targets (p50 <100ms cache, <500ms miss)

---

## Phase 10: Documentation and Migration

### Task 10.1: Update API Documentation
**Files**: Various

- [x] Document `IntentRoutingPipeline` public API
- [x] Document configuration options
- [x] Add usage examples
- [x] Update ARCHITECTURE.md with pipeline diagram

**Validation**: Documentation is complete and accurate

### Task 10.2: Update CLAUDE.md
**File**: `CLAUDE.md`

- [x] Add pipeline architecture section
- [x] Update routing description
- [x] Add configuration schema
- [x] Update anti-patterns

**Validation**: CLAUDE.md reflects new architecture

### Task 10.3: Migration Guide
**File**: `docs/PIPELINE_MIGRATION.md` (new)

- [x] Document how to enable pipeline
- [x] Document config migration
- [x] Document breaking changes (if any)
- [x] Add troubleshooting section

**Validation**: Migration guide covers all scenarios

---

## Dependencies Graph

```
Phase 1 (Types) ──────────────────────────────────────┐
                                                       │
Phase 2 (Cache) ─────────────────────────────────────┼──┐
                                                       │  │
Phase 3 (Calibrator) ────────────────────────────────┼──┤
                                                       │  │
Phase 4 (Layers) ────────────────────────────────────┼──┤
                                                       ↓  ↓
Phase 5 (Aggregator) ─────────────────────────────→ Phase 7 (Pipeline)
                                                       ↓
Phase 6 (Clarification) ─────────────────────────→ Phase 7 (Pipeline)
                                                       ↓
                                                  Phase 8 (Integration)
                                                       ↓
                                                  Phase 9 (Testing)
                                                       ↓
                                                  Phase 10 (Docs)
```

---

## Acceptance Criteria

### Performance
- [ ] Cache hit latency < 50ms
- [ ] L1-only latency < 100ms
- [ ] Full pipeline latency < 500ms (cache miss)
- [ ] L3 calls reduced by >= 50% via cache

### Functionality
- [x] All existing commands continue to work
- [x] Clarification flow preserves context
- [x] Confidence calibration improves accuracy
- [x] Cache learns from user feedback

### Quality
- [x] Unit test coverage >= 80%
- [x] Integration tests cover main flows
- [x] No regressions in existing tests
- [x] Documentation complete

### Rollout
- [x] Feature flag allows gradual rollout
- [x] Metrics enable A/B comparison
- [x] Rollback path documented
