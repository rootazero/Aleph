# Design: Model Router Intelligence System

## Context

The Aleph Model Router currently routes tasks to AI models based on static configuration:
- `CostTier`: Free, Low, Medium, High
- `LatencyTier`: Fast, Medium, Slow
- `Capability`: CodeGeneration, Reasoning, etc.

This approach has limitations:
1. Static tiers don't reflect actual performance
2. No detection of model unavailability
3. No learning from historical data
4. No circuit breaker for failure isolation

This design introduces two complementary systems:
1. **Runtime Metrics** - Learn from actual call data
2. **Health Check** - Detect and respond to availability issues

## Goals / Non-Goals

### Goals
- Improve routing accuracy based on actual performance
- Detect model unavailability within seconds
- Implement circuit breaker to prevent cascade failures
- Provide health/metrics visibility to users
- Zero-configuration startup (graceful degradation to static routing)

### Non-Goals
- Real-time alerting/notification system (future)
- Multi-region routing (future)
- Budget enforcement (future, separate feature)
- Semantic caching (future, separate feature)

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Intelligent Model Router                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────────┐  │
│   │  API Call       │     │ MetricsCollector│     │   HealthManager     │  │
│   │  (Success/Fail) │────▶│ (Ring Buffer)   │────▶│   (State Machine)   │  │
│   └─────────────────┘     └────────┬────────┘     └──────────┬──────────┘  │
│                                    │                          │             │
│                                    ▼                          ▼             │
│                           ┌─────────────────┐        ┌─────────────────┐   │
│                           │ ModelMetrics    │        │  ModelHealth    │   │
│                           │ (Aggregated)    │        │  (Per Model)    │   │
│                           └────────┬────────┘        └──────────┬──────┘   │
│                                    │                            │          │
│                                    └──────────┬─────────────────┘          │
│                                               │                             │
│                                               ▼                             │
│   ┌─────────────────┐                ┌─────────────────┐                   │
│   │  ModelMatcher   │◀───────────────│  DynamicScorer  │                   │
│   │  (Routing)      │                │  (Scoring)      │                   │
│   └────────┬────────┘                └─────────────────┘                   │
│            │                                                                │
│            ▼                                                                │
│   ┌─────────────────┐                                                      │
│   │ Selected Model  │                                                      │
│   └─────────────────┘                                                      │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Part 1: Runtime Metrics System

### 1.1 Core Data Structures

#### CallRecord (Raw Data)
```rust
pub struct CallRecord {
    pub id: String,
    pub model_id: String,
    pub timestamp: SystemTime,
    pub intent: TaskIntent,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_latency: Duration,
    pub ttft: Option<Duration>,        // Time to first token
    pub outcome: CallOutcome,
    pub cost_usd: Option<f64>,
    pub user_feedback: Option<UserFeedback>,
}

pub enum CallOutcome {
    Success,
    Timeout,
    ApiError { status_code: u16 },
    RateLimited,
    ContentFiltered,
    ContextOverflow,
    NetworkError,
    Unknown,
}

pub enum UserFeedback {
    Positive,           // Thumbs up
    Negative,           // Thumbs down
    Regenerated,        // Implicit negative
    EditedAndUsed,      // Partial acceptance
    UsedAsIs,           // Implicit positive
}
```

#### ModelMetrics (Aggregated)
```rust
pub struct ModelMetrics {
    pub model_id: String,
    pub last_updated: SystemTime,
    pub total_calls: u64,
    pub successful_calls: u64,
    pub latency: LatencyStats,          // P50, P90, P95, P99
    pub ttft: Option<LatencyStats>,
    pub cost: CostStats,
    pub success_rate: f64,              // Sliding window
    pub error_distribution: ErrorDistribution,
    pub consecutive_failures: u32,
    pub rate_limit: Option<RateLimitState>,
    pub satisfaction_score: Option<f64>,
    pub intent_performance: HashMap<TaskIntent, IntentMetrics>,
}

pub struct LatencyStats {
    pub count: u64,
    pub mean: f64,
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
    pub stddev: f64,
}
```

#### Multi-Window Metrics
```rust
pub struct MultiWindowMetrics {
    pub short_term: ModelMetrics,   // 5 minutes - detect spikes
    pub medium_term: ModelMetrics,  // 1 hour - routing decisions
    pub long_term: ModelMetrics,    // 24 hours - trends
    pub all_time: ModelMetrics,     // Historical total
}

pub struct WindowConfig {
    pub short_term: Duration,   // Default: 5 min
    pub medium_term: Duration,  // Default: 1 hour
    pub long_term: Duration,    // Default: 24 hours
}
```

### 1.2 Metrics Collector

```rust
pub struct HybridMetricsCollector {
    records: Arc<RwLock<RingBuffer<CallRecord>>>,
    aggregated: Arc<RwLock<HashMap<String, MultiWindowMetrics>>>,
    write_tx: mpsc::Sender<CollectorCommand>,
    window_config: WindowConfig,
    storage: Arc<dyn MetricsStorage>,
}

enum CollectorCommand {
    Record(CallRecord),
    Feedback { call_id: String, feedback: UserFeedback },
    Flush,
    Aggregate,
}
```

**Key Design Decisions:**
1. **Ring Buffer**: Fixed-size in-memory storage (10K records default)
2. **Async Processing**: Non-blocking `record()` via channel
3. **Incremental Aggregation**: Use Welford's algorithm for streaming stats
4. **Background Tasks**: Periodic aggregation (60s) and persistence (300s)

### 1.3 Dynamic Scorer

```rust
pub struct DynamicScorer {
    config: ScoringConfig,
}

pub struct ScoringConfig {
    pub latency_weight: f64,        // Default: 0.25
    pub cost_weight: f64,           // Default: 0.25
    pub reliability_weight: f64,    // Default: 0.35
    pub quality_weight: f64,        // Default: 0.15
    pub latency_target_ms: f64,     // Default: 2000
    pub latency_max_ms: f64,        // Default: 30000
    pub min_success_rate: f64,      // Default: 0.9
    pub degradation_threshold: u32, // Default: 3
}
```

**Scoring Formula:**
```
score = health_weight × (
    latency_weight × latency_score +
    cost_weight × cost_score +
    reliability_weight × reliability_score +
    quality_weight × quality_score
) × penalty_factor
```

### 1.4 Storage (SQLite)

```sql
CREATE TABLE model_metrics (
    model_id TEXT PRIMARY KEY,
    metrics_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE call_records (
    id TEXT PRIMARY KEY,
    model_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    record_json TEXT NOT NULL
);

CREATE INDEX idx_records_model ON call_records(model_id);
CREATE INDEX idx_records_time ON call_records(timestamp);
```

---

## Part 2: Health Check System

### 2.1 Health Status Model

```rust
pub enum HealthStatus {
    Healthy,        // Normal service
    Degraded,       // Available but impaired
    Unhealthy,      // Temporarily unavailable
    CircuitOpen,    // Circuit breaker triggered
    Unknown,        // Insufficient data
}

pub struct ModelHealth {
    pub model_id: String,
    pub status: HealthStatus,
    pub degradation_reason: Option<DegradationReason>,
    pub unhealthy_reason: Option<UnhealthyReason>,
    pub status_since: SystemTime,
    pub last_success: Option<SystemTime>,
    pub last_failure: Option<SystemTime>,
    pub consecutive_successes: u32,
    pub consecutive_failures: u32,
    pub rate_limit: Option<RateLimitInfo>,
    pub circuit_breaker: CircuitBreakerState,
    pub recent_errors: Vec<HealthError>,
}

pub enum DegradationReason {
    HighLatency { current_p95_ms: f64, threshold_ms: f64 },
    PartialErrors { error_rate: f64, threshold: f64 },
    NearRateLimit { remaining_percent: f64 },
    ManualOverride { reason: String },
}

pub enum UnhealthyReason {
    ConsecutiveFailures { count: u32, threshold: u32 },
    HighErrorRate { rate: f64, threshold: f64 },
    RateLimited { reset_at: Option<SystemTime> },
    ApiUnreachable { last_error: String },
    AuthenticationFailed,
    QuotaExhausted,
    ManuallyDisabled { reason: String },
}
```

### 2.2 State Transition Machine

```
                    ┌─────────────────────────────────────────────────────────┐
                    │                    状态转换图                            │
                    └─────────────────────────────────────────────────────────┘

                                    启动/首次调用
                                          │
                                          ▼
                                    ┌──────────┐
                            ┌───────│ Unknown  │───────┐
                            │       └──────────┘       │
                            │ 首次成功                  │ 首次失败
                            ▼                          ▼
                      ┌──────────┐              ┌──────────────┐
          ┌──────────▶│ Healthy  │◀─────────────│  Unhealthy   │◀──────┐
          │           └──────────┘  恢复成功     └──────────────┘       │
          │                │                           │               │
          │                │ 性能下降                   │ 连续失败      │
          │                │ 或接近限流                 │ 达到阈值      │
          │                ▼                          ▼               │
          │           ┌──────────┐              ┌──────────────┐      │
          │           │ Degraded │              │ CircuitOpen  │──────┘
          │           └──────────┘              └──────────────┘  冷却后
          │                │                          │          半开尝试
          │                │ 性能恢复                  │
          └────────────────┘                          │
                    ▲                                 │
                    └─────────────────────────────────┘
                            半开成功 N 次
```

**Transition Rules:**
| Current | Event | Condition | Next |
|---------|-------|-----------|------|
| Unknown | Success | - | Healthy |
| Unknown | Failure | - | Unhealthy |
| Healthy | Success + High Latency | p95 > threshold | Degraded |
| Healthy | Failures | consecutive >= 3 | Unhealthy |
| Degraded | Success × N | latency normal | Healthy |
| Degraded | Failures | consecutive >= 3 | Unhealthy |
| Unhealthy | Success × N | N >= 2 | Healthy |
| Unhealthy | Failures | >= 5 in 60s | CircuitOpen |
| CircuitOpen | Cooldown elapsed | - | HalfOpen |
| HalfOpen | Success × N | N >= 2 | Healthy |
| HalfOpen | Failure | - | CircuitOpen (reopen) |

### 2.3 Circuit Breaker

```rust
pub struct CircuitBreakerState {
    pub state: CircuitState,          // Closed, Open, HalfOpen
    pub failure_count: u32,
    pub opened_at: Option<SystemTime>,
    pub next_attempt_at: Option<SystemTime>,
    pub half_open_successes: u32,
}
```

**Backoff Strategy**: Exponential with cap
```
cooldown = base_cooldown × 2^(min(failure_count - 1, 5))
```

Default: 30s base, max 16 minutes (30 × 2^5 = 960s)

### 2.4 Health Manager

```rust
pub struct HealthManager {
    health_states: Arc<RwLock<HashMap<String, ModelHealth>>>,
    transition_engine: HealthTransitionEngine,
    config: HealthConfig,
    event_tx: broadcast::Sender<HealthEvent>,
    prober: Option<Arc<HealthProber>>,
}

pub enum HealthEvent {
    StatusChanged { model_id: String, old: HealthStatus, new: HealthStatus },
    CircuitOpened { model_id: String, failure_count: u32, cooldown_secs: u64 },
    CircuitClosed { model_id: String },
    RateLimitWarning { model_id: String, remaining_percent: f64 },
}
```

### 2.5 Active Probing (Optional)

```rust
pub struct HealthProber {
    client: reqwest::Client,
    config: ProbeConfig,
    endpoints: HashMap<String, ProbeEndpoint>,
}

pub struct ProbeEndpoint {
    pub health_url: Option<String>,    // Dedicated health endpoint
    pub test_prompt: String,           // Minimal "Hi" prompt
    pub timeout: Duration,
}
```

**Probing Strategy:**
- Only probe unhealthy/circuit-open models
- Use dedicated health endpoints when available
- Fallback to minimal API request ("Hi")
- Default interval: 30s

---

## Part 3: Integration

### 3.1 ModelMatcher Extensions

```rust
impl ModelMatcher {
    // Existing
    pub fn route(&self, task: &Task) -> Result<ModelProfile, RoutingError>;

    // New: Health-aware routing
    pub async fn route_with_health(
        &self,
        task: &Task,
        health_manager: &HealthManager,
    ) -> Result<ModelProfile, RoutingError>;

    // New: Metrics-aware routing
    pub async fn route_with_metrics(
        &self,
        task: &Task,
        collector: &dyn MetricsCollector,
        scorer: &DynamicScorer,
    ) -> Result<ModelProfile, RoutingError>;

    // New: Combined intelligent routing
    pub async fn route_intelligent(
        &self,
        task: &Task,
        health_manager: &HealthManager,
        collector: &dyn MetricsCollector,
        scorer: &DynamicScorer,
    ) -> Result<ModelProfile, RoutingError>;
}
```

### 3.2 Routing Flow

```
route_intelligent():
    1. Get all candidates for task
    2. For each candidate:
       a. Check health status (can_call)
       b. Get runtime metrics
       c. Compute dynamic score
       d. Combine: final_score = health_weight × dynamic_score
    3. Filter blocked models (CircuitOpen, Unhealthy)
    4. Sort by final_score descending
    5. Return top model
```

---

## Configuration

```toml
[cowork.model_router.metrics]
enabled = true
buffer_size = 10000
aggregation_interval_secs = 60
flush_interval_secs = 300
db_path = "~/.aleph/metrics.db"
exploration_rate = 0.05

[cowork.model_router.metrics.windows]
short_term_secs = 300
medium_term_secs = 3600
long_term_secs = 86400

[cowork.model_router.metrics.scoring]
latency_weight = 0.25
cost_weight = 0.25
reliability_weight = 0.35
quality_weight = 0.15
latency_target_ms = 2000
latency_max_ms = 30000
min_success_rate = 0.9

[cowork.model_router.health]
enabled = true
active_probing = true
failure_threshold = 3
recovery_successes = 2
latency_degradation_threshold_ms = 10000
latency_healthy_threshold_ms = 5000
rate_limit_warning_threshold = 0.2

[cowork.model_router.health.circuit_breaker]
failure_threshold = 5
window_secs = 60
cooldown_secs = 30
half_open_successes = 2
```

---

## Risks / Trade-offs

| Risk | Impact | Mitigation |
|------|--------|------------|
| SQLite I/O overhead | Minor latency | Async writes, batched persistence |
| Memory for ring buffer | ~10MB | Configurable size, auto-eviction |
| False positive circuit break | Model unavailable | Conservative thresholds, manual override |
| Cold start (no data) | Suboptimal routing | Graceful fallback to static routing |
| Probe requests cost | API usage | Minimal prompts, only for unhealthy |

---

## Migration Plan

1. **Phase 1**: Add data structures, no behavioral change
2. **Phase 2**: Collect metrics passively, existing routing unchanged
3. **Phase 3**: Enable `route_with_metrics()` opt-in
4. **Phase 4**: Enable health checking
5. **Phase 5**: Make `route_intelligent()` default

**Rollback**: Each phase is independently disableable via config.

---

## Open Questions

1. Should we implement cross-session metrics sharing?
2. Should metrics influence provider selection in addition to model selection?
3. Should we add alerting webhooks for circuit breaker events?
4. Should we persist health state across restarts?
