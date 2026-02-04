# Change: Add Runtime Metrics and Health Check to Model Router

## Why

The current Model Router uses static configuration (CostTier, LatencyTier) for routing decisions. This doesn't reflect actual model performance, availability, or cost in production. We need a data-driven approach that:

1. Learns from actual API call results to improve routing decisions
2. Detects and responds to model unavailability (rate limits, outages, degraded performance)
3. Implements circuit breaker patterns to prevent cascade failures
4. Provides observability into model health and performance

## What Changes

### P0: Runtime Metrics System
- **NEW** `CallRecord` - Raw call data (latency, tokens, cost, outcome)
- **NEW** `ModelMetrics` - Aggregated statistics with time windows
- **NEW** `MetricsCollector` - Async collection with ring buffer
- **NEW** `DynamicScorer` - Scoring based on actual performance
- **MODIFIED** `ModelMatcher` - Add `route_with_metrics()` method
- **NEW** SQLite persistence for metrics data

### P0: Health Check System
- **NEW** `HealthStatus` enum (Healthy, Degraded, Unhealthy, CircuitOpen, Unknown)
- **NEW** `ModelHealth` - Complete health state with history
- **NEW** `HealthTransitionEngine` - State machine for transitions
- **NEW** `HealthManager` - Central health state management
- **NEW** `CircuitBreaker` - Failure isolation with exponential backoff
- **NEW** `HealthProber` - Optional active health probing
- **MODIFIED** `ModelMatcher` - Add `route_with_health()` method
- **NEW** UniFFI exports for Swift UI integration

### Integration
- **NEW** `route_intelligent()` - Combined metrics + health routing
- **NEW** Health events system for UI updates
- **NEW** Configuration options for all thresholds

## Impact

- Affected specs: `model-router` (new capability spec)
- Affected code:
  - `core/src/cowork/model_router/` - New modules: metrics.rs, health.rs, collector.rs, scoring.rs
  - `core/src/cowork_ffi.rs` - New UniFFI exports
  - `core/src/config/types/cowork.rs` - New configuration types
  - `platforms/macos/Aleph/Sources/` - New health/metrics UI views
- Dependencies: None (uses existing tokio, serde, sqlx)
- **Non-breaking**: All new APIs, existing `route()` unchanged
