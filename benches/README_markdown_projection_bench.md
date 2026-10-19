# Markdown Projection Benchmarks

## Overview

This benchmark measures the performance of the Markdown projection system with three key metrics:

1. **Event Throughput** - Target: 1000 events/sec
   - Processes 1,000 / 5,000 / 10,000 events across multiple sessions
   - Reports events per second

2. **Projection Latency** - Target: <10ms per event
   - Measures individual event processing time
   - Tests: UserMessage, AssistantMessage, ToolCallRequested, ToolResult
   - Reports p50, p95, p99 latencies

3. **Memory Usage** - Target: <500MB for 1000 sessions
   - Creates 100 / 500 / 1000 sessions with ~100 events each
   - Measures peak memory consumption
   - Note: For precise memory profiling, use external tools like `heaptrack` or `valgrind`

## Running the Benchmarks

```bash
# Run all markdown projection benchmarks
cargo bench --bench markdown_projection_bench

# Run specific benchmark group
cargo bench --bench markdown_projection_bench event_throughput
cargo bench --bench markdown_projection_bench projection_latency
cargo bench --bench markdown_projection_bench memory_usage

# Generate HTML report (requires criterion html_reports feature)
cargo bench --bench markdown_projection_bench -- --save-baseline my_baseline
```

## Prerequisites

Before running these benchmarks, ensure the codebase compiles cleanly:

### Known Compilation Issues (as of 2026-10-19)

1. **Module ambiguity error** in `src/gateway/handlers/`:
   - Both `memory.rs` and `memory/mod.rs` exist
   - Rust cannot determine which module file to use
   - Fix: Remove or rename one of them

2. **Missing trait import** in `src/gateway/server/attachment_route.rs`:
   - `get_attachment` method requires `RawSessionStore` trait in scope
   - Fix: Add `use crate::memory::store::session::RawSessionStore;`

These are pre-existing issues unrelated to the benchmark code.

## Benchmark Structure

Each benchmark follows this pattern:

```rust
// Setup phase
let runtime = tokio::runtime::Runtime::new().unwrap();
let temp_dir = TempDir::new().unwrap();
let store = Arc::new(MarkdownSessionStore::new(temp_dir.path()).await?);
let projector = MarkdownProjector::new(store.clone());

// Measurement phase
runtime.block_on(async {
    // Process events
    for event in events {
        projector.on_appended(&session_id, &event);
    }
    
    // Wait for background projection
    tokio::time::sleep(Duration::from_millis(50)).await;
});
```

## Expected Results

Baseline targets (from task specification):

- Event throughput: **≥ 1000 events/sec**
- Projection latency: **< 10ms per event**
- Memory usage: **< 500MB for 1000 sessions**

Actual results will depend on:
- Hardware (CPU, disk I/O, memory speed)
- File system type (SSD vs HDD, encryption overhead)
- System load during benchmark run
- SQLite performance characteristics

## Implementation Notes

- Uses `criterion` crate for statistical benchmarking
- Creates isolated temp directories for each benchmark run
- Follows same event structure as integration test (`tests/markdown_session_lifecycle.rs`)
- Background projection uses unbounded channels (async processing)
- Includes small sleep delays to allow background writes to complete

## Related Files

- Integration test: `tests/markdown_session_lifecycle.rs`
- Implementation: `src/session/markdown_projector.rs`
- Store: `src/memory/store/markdown/store_impl.rs`
- Design doc: `docs/specs/2026-10-18-markdown-storage-design.md`
