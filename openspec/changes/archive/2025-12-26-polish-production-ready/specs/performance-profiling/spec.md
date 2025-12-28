# performance-profiling Specification

## Purpose

Implement opt-in performance instrumentation to identify bottlenecks in the hotkey→AI→paste pipeline. Profiling data guides optimization efforts and validates that latency targets are met for production deployment.

## ADDED Requirements

### Requirement: Performance Metrics Collection

The system SHALL instrument critical code paths with timing measurements when profiling is enabled.

#### Scenario: Measure hotkey to clipboard latency

- **WHEN** `enable_performance_logging` config is true
- **AND** user presses global hotkey
- **THEN** timestamp is captured at hotkey detection
- **AND** clipboard read timestamp is captured
- **AND** latency (Δt) is calculated and logged
- **AND** target latency is 50ms

#### Scenario: Measure clipboard to memory retrieval latency

- **WHEN** profiling is enabled
- **AND** clipboard content is read
- **THEN** memory retrieval start timestamp is captured
- **AND** memory retrieval completion timestamp is captured
- **AND** latency is logged as "Memory retrieval: Xms (target: 100ms)"

#### Scenario: Measure AI request latency

- **WHEN** profiling is enabled
- **AND** AI provider is invoked
- **THEN** request start timestamp is captured
- **AND** response received timestamp is captured
- **AND** latency includes network round-trip time
- **AND** provider name and model are logged with latency

#### Scenario: Measure end-to-end pipeline latency

- **WHEN** profiling is enabled
- **AND** request completes (success or error)
- **THEN** total pipeline latency is calculated (hotkey → paste)
- **AND** latency is logged: "Total pipeline: Xms"
- **AND** stage breakdown is included in debug logs

### Requirement: Performance Target Validation

The system SHALL compare measured latencies against predefined targets and warn on violations.

#### Scenario: Detect slow clipboard read

- **WHEN** clipboard read takes >100ms (2x target of 50ms)
- **THEN** warn-level log: "Slow clipboard read: 150ms (target: 50ms)"
- **AND** warning is logged even if profiling is disabled
- **AND** repeated warnings trigger user notification

#### Scenario: Detect slow memory retrieval

- **WHEN** memory retrieval takes >200ms (2x target of 100ms)
- **THEN** warn-level log: "Slow memory retrieval: 250ms (target: 100ms)"
- **AND** database size is logged for context
- **AND** suggestion to optimize or clear old memories

#### Scenario: Detect slow AI response

- **WHEN** AI request takes >1000ms (2x target of 500ms)
- **THEN** warn-level log: "Slow AI response: 1500ms (target: 500ms)"
- **AND** provider and model are logged
- **AND** suggestion to check network or switch provider

#### Scenario: All stages within targets

- **WHEN** all pipeline stages meet latency targets
- **THEN** info-level log: "Pipeline completed within targets (total: 450ms)"
- **AND** no warnings are generated

### Requirement: Profiling Configuration

The system SHALL allow users to enable/disable performance profiling via configuration.

#### Scenario: Enable profiling

- **WHEN** user sets `enable_performance_logging = true` in config
- **THEN** all performance metrics are logged at debug level
- **AND** stage-by-stage breakdown is included
- **AND** minimal overhead (<5% latency increase)

#### Scenario: Disable profiling (default)

- **WHEN** `enable_performance_logging` is false or unset
- **THEN** only critical warnings (>2x target) are logged
- **AND** stage breakdown is skipped
- **AND** overhead is negligible (<1%)

#### Scenario: Runtime toggle

- **WHEN** user toggles profiling in Settings UI
- **THEN** change takes effect immediately (no restart)
- **AND** current request completes with old setting
- **AND** next request uses new setting

### Requirement: Metrics Module Architecture

The system SHALL implement a reusable `metrics` module for instrumentation.

#### Scenario: Create stage timer

- **WHEN** code calls `metrics::start_timer("clipboard_read")`
- **THEN** timer struct is returned with stage name and start timestamp
- **AND** timer can be stopped with `timer.stop()`
- **AND** elapsed time is automatically logged on drop

#### Scenario: Nested timers

- **WHEN** "ai_pipeline" timer is started
- **AND** "memory_retrieval" timer is started within pipeline
- **THEN** both timers track independently
- **AND** memory timer completes before pipeline timer
- **AND** log output shows hierarchical relationship

#### Scenario: Timer with metadata

- **WHEN** timer is created with context: `timer.with_meta("provider", "OpenAI")`
- **THEN** metadata is included in log output
- **AND** log shows: "AI request (provider: OpenAI): 450ms"

### Requirement: Performance Optimization Recommendations

The system SHALL provide optimization recommendations based on profiling data.

#### Scenario: Recommend memory cleanup

- **WHEN** memory retrieval consistently exceeds 200ms
- **AND** memory database size >100MB
- **THEN** suggestion logged: "Consider clearing old memories or increasing retention threshold"
- **AND** Settings → Memory shows recommendation badge

#### Scenario: Recommend provider switch

- **WHEN** AI provider latency consistently exceeds 1000ms
- **AND** alternative provider is configured
- **THEN** suggestion logged: "Provider 'Ollama' is slow. Consider switching to 'OpenAI' for faster responses"

#### Scenario: Recommend config optimization

- **WHEN** clipboard read is slow (>100ms)
- **AND** large image is detected
- **THEN** suggestion: "Enable auto_compress_images to reduce clipboard read time"

### Requirement: Profiling Data Export

The system SHALL allow exporting profiling data for offline analysis (developer tool).

#### Scenario: Export profiling data

- **WHEN** user enables `export_profiling_data` in config (hidden option)
- **THEN** profiling data is written to `~/.config/aether/profiling.jsonl`
- **AND** each line is JSON object: `{"stage": "ai_request", "latency_ms": 450, "provider": "OpenAI", "timestamp": "..."}`
- **AND** file is rotated daily (same as logs)

#### Scenario: Analyze profiling data

- **WHEN** developer runs `cargo run --bin analyze_profiling`
- **THEN** script parses profiling.jsonl
- **AND** outputs percentile statistics (p50, p90, p99)
- **AND** identifies slowest stages and outliers

### Requirement: Zero-Overhead When Disabled

The system SHALL ensure profiling instrumentation has negligible impact when disabled.

#### Scenario: Check overhead when profiling disabled

- **WHEN** `enable_performance_logging = false`
- **AND** pipeline executes 100 requests
- **THEN** average overhead is <1% (measured via benchmarks)
- **AND** no memory allocations for profiling occur
- **AND** no observable performance degradation

#### Scenario: Compile-time optimization

- **WHEN** profiling code is behind conditional compilation (`#[cfg(feature = "profiling")]`)
- **THEN** disabled profiling code is completely removed from binary
- **AND** release builds have zero profiling overhead (if feature disabled)

### Requirement: Profiling Integration with Structured Logging

The system SHALL integrate performance metrics with the structured logging system.

#### Scenario: Performance logs use tracing framework

- **WHEN** performance metric is logged
- **THEN** tracing debug-level span is used
- **AND** metric appears in log file with timestamp and context
- **AND** log viewer can filter by "performance" keyword

#### Scenario: Profiling respects log level

- **WHEN** `RUST_LOG` is set to "info"
- **AND** profiling is enabled
- **THEN** only warn-level performance alerts are logged
- **AND** debug-level metrics are suppressed

## ADDED Requirements (Optional/Future)

### Requirement: Real-Time Performance Dashboard (Deferred to Phase 8)

The system COULD provide a real-time dashboard in Settings UI showing current performance metrics.

#### Scenario: View performance dashboard

- **WHEN** user opens Settings → Advanced → Performance
- **THEN** live chart shows last 50 requests' latencies
- **AND** current targets and thresholds are displayed
- **AND** chart updates every 5 seconds

## MODIFIED Requirements

None - This is a new capability with no modifications to existing specs.

## References

- **Related Spec**: `structured-logging` - Performance logs use logging framework
- **Related Spec**: `core-library` - AetherCore integrates metrics module
- **Depends On**: `tracing` for instrumentation, `std::time::Instant` for timing
