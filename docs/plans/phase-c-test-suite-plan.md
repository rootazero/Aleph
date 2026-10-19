# Phase C: Comprehensive Test Suite - Implementation Plan

**Date**: 2026-10-19  
**Status**: Planning  
**Prerequisites**: Phase A + B compilation verified and passing

---

## Overview

Build a comprehensive test suite covering unit tests, integration tests, property tests, and performance benchmarks for the Markdown session storage system.

---

## Test Categories

### 1. Unit Tests (Target: 80%+ coverage)

#### 1.1 Markdown Store Implementation
**File**: `src/memory/store/markdown/store_impl.rs`

**Test Cases**:
```rust
// Session Creation
#[tokio::test]
async fn test_create_session_success() { }

#[tokio::test]
async fn test_create_session_duplicate_idempotent() { }

#[tokio::test]
async fn test_create_session_invalid_path() { }

// Transcript Operations
#[tokio::test]
async fn test_append_transcript_formats_correctly() { }

#[tokio::test]
async fn test_append_transcript_preserves_order() { }

#[tokio::test]
async fn test_read_transcript_returns_full_content() { }

#[tokio::test]
async fn test_transcript_handles_special_chars() { }

// Tool Outputs
#[tokio::test]
async fn test_append_tool_output_formats_correctly() { }

#[tokio::test]
async fn test_tool_output_preserves_exit_code() { }

#[tokio::test]
async fn test_tool_output_handles_empty_stderr() { }

// Attachments
#[tokio::test]
async fn test_store_attachment_deduplicates() { }

#[tokio::test]
async fn test_store_attachment_computes_hash() { }

#[tokio::test]
async fn test_get_attachment_returns_original() { }

#[tokio::test]
async fn test_attachment_handles_large_files() { }

// Session Finalization
#[tokio::test]
async fn test_finalize_session_computes_hash() { }

#[tokio::test]
async fn test_finalize_session_sets_end_time() { }

#[tokio::test]
async fn test_finalize_session_updates_stats() { }

// Listing and Pagination
#[tokio::test]
async fn test_list_sessions_pagination() { }

#[tokio::test]
async fn test_list_sessions_filters_by_agent() { }

#[tokio::test]
async fn test_list_sessions_filters_by_date_range() { }

#[tokio::test]
async fn test_list_sessions_filters_by_status() { }

// Search
#[tokio::test]
async fn test_search_transcripts_fts5() { }

#[tokio::test]
async fn test_search_returns_ranked_results() { }

#[tokio::test]
async fn test_search_handles_special_chars() { }

// Archiving
#[tokio::test]
async fn test_archive_session_moves_files() { }

#[tokio::test]
async fn test_archive_session_preserves_structure() { }

#[tokio::test]
async fn test_archive_session_updates_metadata() { }
```

**Estimated**: 25 tests, ~500 lines

#### 1.2 Markdown Projector
**File**: `src/session/markdown_projector.rs`

**Test Cases**:
```rust
// Event Projection
#[tokio::test]
async fn test_projector_handles_run_started() { }

#[tokio::test]
async fn test_projector_handles_user_message() { }

#[tokio::test]
async fn test_projector_handles_assistant_message() { }

#[tokio::test]
async fn test_projector_handles_system_message() { }

#[tokio::test]
async fn test_projector_handles_tool_call() { }

#[tokio::test]
async fn test_projector_handles_tool_result() { }

#[tokio::test]
async fn test_projector_handles_tool_error() { }

#[tokio::test]
async fn test_projector_handles_run_finished() { }

#[tokio::test]
async fn test_projector_ignores_internal_events() { }

// Background Worker
#[tokio::test]
async fn test_projector_worker_processes_queue() { }

#[tokio::test]
async fn test_projector_worker_handles_errors() { }

#[tokio::test]
async fn test_projector_worker_preserves_order() { }

// Concurrency
#[tokio::test]
async fn test_projector_handles_concurrent_events() { }

#[tokio::test]
async fn test_projector_handles_burst_traffic() { }
```

**Estimated**: 14 tests, ~400 lines

#### 1.3 Composite Observer
**File**: `src/session/composite_observer.rs`

**Test Cases**:
```rust
#[test]
fn test_composite_dispatches_to_all_observers() { }

#[test]
fn test_composite_handles_observer_panic() { }

#[test]
fn test_composite_preserves_order() { }
```

**Estimated**: 3 tests, ~100 lines

#### 1.4 Utilities
**File**: `src/memory/store/markdown/utils.rs`

**Test Cases**:
```rust
#[test]
fn test_format_timestamp_readable() { }

#[test]
fn test_compute_sha256_deterministic() { }

#[test]
fn test_atomic_write_creates_temp_file() { }

#[test]
fn test_atomic_write_renames_atomically() { }

#[test]
fn test_append_markdown_uses_lock() { }
```

**Estimated**: 5 tests, ~150 lines

---

### 2. Integration Tests (Realistic Scenarios)

#### 2.1 End-to-End Session Lifecycle
**File**: `tests/markdown_storage_integration.rs`

```rust
#[tokio::test]
async fn test_complete_session_lifecycle() {
    // 1. Initialize Gateway with Markdown projector
    // 2. Create session via SessionService
    // 3. Emit 10 events (user messages, tool calls, etc.)
    // 4. Finalize session
    // 5. Verify Markdown files exist and contain correct data
    // 6. Verify metadata.db has correct records
    // 7. Verify FTS5 index is searchable
}
```

#### 2.2 Concurrent Sessions
**File**: `tests/markdown_storage_concurrency.rs`

```rust
#[tokio::test]
async fn test_multiple_sessions_concurrent() {
    // Spawn 10 sessions concurrently
    // Each session emits 100 events
    // Verify no data corruption or lost events
}
```

#### 2.3 Projector Failure Recovery
**File**: `tests/markdown_projector_resilience.rs`

```rust
#[tokio::test]
async fn test_projector_initialization_failure() {
    // Simulate MarkdownSessionStore::new() failure
    // Verify Gateway starts successfully
    // Verify MessageProjector still works
}

#[tokio::test]
async fn test_projector_disk_full() {
    // Simulate disk full error during projection
    // Verify error is logged
    // Verify SessionEventStore unaffected
}
```

#### 2.4 Migration Tool Integration
**File**: `tests/aleph_migrate_integration.rs`

```rust
#[tokio::test]
async fn test_migrate_export_import_roundtrip() {
    // 1. Create sessions in SQLite
    // 2. Export to JSON
    // 3. Transform to Markdown
    // 4. Index metadata
    // 5. Verify integrity
}
```

**Estimated**: 4 integration tests, ~800 lines

---

### 3. Property Tests (Randomized Testing)

#### 3.1 Content Preservation
**File**: `tests/markdown_storage_properties.rs`

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_transcript_preserves_content(content: String) {
        // Append content to transcript
        // Read back and verify exact match
    }

    #[test]
    fn test_attachment_preserves_bytes(data: Vec<u8>) {
        // Store attachment
        // Read back and verify exact byte match
    }

    #[test]
    fn test_sha256_deterministic(data: Vec<u8>) {
        // Compute hash twice
        // Verify identical results
    }
}
```

**Estimated**: 5 property tests, ~200 lines

---

### 4. Performance Benchmarks

#### 4.1 Throughput Benchmark
**File**: `benches/markdown_storage_throughput.rs`

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};

fn benchmark_append_transcript(c: &mut Criterion) {
    let mut group = c.benchmark_group("append_transcript");
    
    for size in [100, 1000, 10000] {
        group.bench_with_input(
            BenchmarkId::new("message_size", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    // Append message of given size
                });
            },
        );
    }
    
    group.finish();
}

fn benchmark_projection_latency(c: &mut Criterion) {
    c.bench_function("projector_enqueue", |b| {
        b.iter(|| {
            // Enqueue event to projector
            // Measure time to return
        });
    });
}

criterion_group!(benches, 
    benchmark_append_transcript,
    benchmark_projection_latency,
);
criterion_main!(benches);
```

**Targets**:
- Append transcript: < 10 ms p99
- Projector enqueue: < 1 µs p99
- Throughput: 1000 events/sec sustained

**Estimated**: 3 benchmarks, ~300 lines

#### 4.2 Memory Benchmark
**File**: `benches/markdown_storage_memory.rs`

```rust
#[bench]
fn bench_memory_usage_1000_sessions() {
    // Create 1000 sessions with 100 events each
    // Measure peak RSS
    // Target: < 500 MB
}
```

---

### 5. Stress Tests

#### 5.1 High-Volume Stress Test
**File**: `tests/stress_high_volume.rs`

```rust
#[tokio::test]
#[ignore] // Run manually with: cargo test --release -- --ignored
async fn test_1000_events_per_second_for_1_minute() {
    // Send 60,000 events total
    // Verify no memory leaks
    // Verify all events projected
}
```

#### 5.2 Long-Running Stress Test
**File**: `tests/stress_long_running.rs`

```rust
#[tokio::test]
#[ignore]
async fn test_1_hour_continuous_operation() {
    // Run for 1 hour
    // 100 events/sec steady state
    // Monitor memory usage
    // Verify no file descriptor leaks
}
```

---

## Test Infrastructure

### Test Utilities Module
**File**: `tests/common/mod.rs`

```rust
pub mod fixtures {
    // Sample SessionEvents
    pub fn sample_run_started() -> SessionEvent { }
    pub fn sample_user_message(content: &str) -> SessionEvent { }
    // ...
}

pub mod helpers {
    // Create temp test directory
    pub fn temp_test_dir() -> TempDir { }
    
    // Create test MarkdownSessionStore
    pub async fn test_store() -> Arc<MarkdownSessionStore> { }
    
    // Create test projector
    pub fn test_projector(store: Arc<MarkdownSessionStore>) 
        -> MarkdownProjector { }
}

pub mod assertions {
    // Assert session directory exists
    pub fn assert_session_exists(base_dir: &Path, session_id: &str) { }
    
    // Assert transcript contains content
    pub async fn assert_transcript_contains(
        store: &MarkdownSessionStore,
        session_id: &str,
        content: &str,
    ) { }
}
```

**Estimated**: ~400 lines

---

## Test Execution Strategy

### CI Pipeline
```yaml
# .github/workflows/test.yml
name: Test Suite

on: [push, pull_request]

jobs:
  unit-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
      - run: cargo test --lib

  integration-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
      - run: cargo test --test '*'

  property-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
      - run: cargo test --release -- --ignored proptest

  benchmarks:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
      - run: cargo bench
```

### Local Development
```bash
# Run all tests
./scripts/test-all.sh

# Run specific category
cargo test --lib memory::store::markdown
cargo test --test markdown_storage_integration

# Run with coverage
cargo tarpaulin --out Html

# Run benchmarks
cargo bench --bench markdown_storage_throughput
```

---

## Success Criteria

### Coverage Targets
- **Unit tests**: 80%+ line coverage
- **Integration tests**: All critical paths covered
- **Property tests**: 5+ properties verified
- **Benchmarks**: All targets met

### Quality Gates
- [ ] All unit tests pass
- [ ] All integration tests pass
- [ ] Property tests pass with 1000 iterations
- [ ] Throughput benchmark: 1000 events/sec
- [ ] Latency benchmark: < 10 ms p99 append, < 1 µs enqueue
- [ ] Memory: < 500 MB for 1000 sessions
- [ ] No memory leaks in 1-hour stress test
- [ ] No file descriptor leaks

---

## Implementation Timeline

### Day 1: Unit Tests
- Morning: Markdown store tests (15 tests)
- Afternoon: Projector tests (10 tests)
- Evening: Utilities tests (5 tests)

### Day 2: Integration Tests
- Morning: End-to-end lifecycle test
- Afternoon: Concurrency and resilience tests
- Evening: Migration tool integration

### Day 3: Property Tests & Benchmarks
- Morning: Property tests with proptest
- Afternoon: Throughput and latency benchmarks
- Evening: Stress tests (1-hour run overnight)

### Day 4: Verification & Documentation
- Review all test results
- Fix any failures
- Generate coverage report
- Update documentation

---

## Subagent Task Breakdown

### Task C1: Unit Tests - Markdown Store
**Owner**: Subagent  
**Duration**: 4 hours  
**Deliverable**: 25 passing unit tests for `store_impl.rs`

### Task C2: Unit Tests - Projector
**Owner**: Subagent  
**Duration**: 3 hours  
**Deliverable**: 14 passing unit tests for `markdown_projector.rs`

### Task C3: Integration Tests
**Owner**: Subagent  
**Duration**: 4 hours  
**Deliverable**: 4 passing integration tests

### Task C4: Property Tests
**Owner**: Subagent  
**Duration**: 2 hours  
**Deliverable**: 5 property tests with proptest

### Task C5: Benchmarks
**Owner**: Subagent  
**Duration**: 3 hours  
**Deliverable**: Criterion benchmarks with baseline

### Task C6: Stress Tests
**Owner**: Subagent  
**Duration**: 2 hours + 1 hour monitoring  
**Deliverable**: 2 stress test results (1-hour run)

---

**Total Estimated Effort**: 18-20 hours across 6 parallel tasks  
**Elapsed Time**: 3-4 days (with parallelization)

---

*This plan will be executed after Phase A + B compilation is verified.*
