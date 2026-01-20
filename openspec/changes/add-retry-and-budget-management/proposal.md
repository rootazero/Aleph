# Change: Add Intelligent Retry/Failover and Budget Management to Model Router

## Why

The Model Router (P0) successfully implemented runtime metrics collection and health monitoring. However, two critical P1 capabilities are missing:

### Problem 1: No Automatic Retry or Failover
When an API call fails (timeout, rate limit, network error), the system:
- Returns error immediately without retry attempt
- Doesn't try alternative models that could handle the request
- Loses user's work when a single provider has temporary issues

**Impact**: Poor user experience during provider outages; wasted API calls that could succeed on retry.

### Problem 2: No Budget Controls
The system:
- Has no spending limits per user/project/time-period
- Cannot track cumulative costs in real-time
- Allows unlimited API spend with no warnings
- Has no cost-aware routing decisions based on remaining budget

**Impact**: Users can accidentally incur large bills; no visibility into costs until provider invoice arrives.

## What Changes

### 1. Retry Orchestrator (NEW)

A resilient request execution layer that wraps model calls with intelligent retry logic:

```
┌─────────────────────────────────────────────────────────────────┐
│                     RetryOrchestrator                           │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ RetryPolicy  │  │FailoverChain │  │   BackoffStrategy      │ │
│  │ (rules)      │  │ (alternatives)│  │   (timing)             │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ BudgetGate   │  │ CircuitCheck │  │   ExecutionLog         │ │
│  │ (pre-check)  │  │ (health)     │  │   (observability)      │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

**Key Features**:
- **RetryPolicy**: Configurable max attempts, retryable error types, timeout per attempt
- **BackoffStrategy**: Exponential backoff with jitter, rate-limit aware delays
- **FailoverChain**: Automatic fallback to alternative models on failure
- **Integration**: Works with existing HealthManager and MetricsCollector

### 2. Budget Manager (NEW)

A cost control layer that enforces spending limits and provides cost visibility:

```
┌─────────────────────────────────────────────────────────────────┐
│                      BudgetManager                              │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ BudgetPool   │  │ CostTracker  │  │   SpendingAlert        │ │
│  │ (limits)     │  │ (accumulator)│  │   (notifications)      │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ BudgetGate   │  │ CostEstimator│  │   ResetScheduler       │ │
│  │ (enforcement)│  │ (pre-call)   │  │   (periods)            │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

**Key Features**:
- **BudgetPool**: Hierarchical limits (global → project → session)
- **CostTracker**: Real-time cost accumulation from CallRecords
- **CostEstimator**: Pre-call estimation based on token count and model pricing
- **SpendingAlert**: Configurable thresholds (50%, 80%, 100%) with notifications
- **BudgetGate**: Pre-execution check that blocks calls exceeding budget

### 3. Integration with Existing Components

**MODIFIED** `ModelMatcher`:
- New method: `route_with_budget(request, budget_context) -> Result<ModelProfile, RoutingError>`
- Budget-aware model selection (prefer cheaper models when budget is low)

**MODIFIED** `IntelligentRouter`:
- Integration point for RetryOrchestrator
- Budget context passed through routing decisions

**NEW** Configuration:
```toml
[model_router.retry]
max_attempts = 3
initial_backoff_ms = 100
max_backoff_ms = 5000
jitter_factor = 0.2
retryable_errors = ["timeout", "rate_limited", "network_error"]

[model_router.budget]
enabled = true
default_daily_limit_usd = 10.0
warning_thresholds = [0.5, 0.8, 0.95]
reset_timezone = "UTC"
```

## Impact

### Affected Specs
- **NEW**: `retry-orchestrator` - Retry and failover capability
- **NEW**: `budget-manager` - Budget enforcement capability
- **MODIFIED**: `ai-routing` - Integration points (minimal changes)

### Affected Code
- `core/src/dispatcher/model_router/retry.rs` - NEW: RetryOrchestrator, RetryPolicy, BackoffStrategy
- `core/src/dispatcher/model_router/failover.rs` - NEW: FailoverChain, FailoverStrategy
- `core/src/dispatcher/model_router/budget.rs` - NEW: BudgetManager, BudgetPool, CostTracker
- `core/src/dispatcher/model_router/mod.rs` - Export new modules
- `core/src/dispatcher/model_router/matcher.rs` - Add budget-aware routing method
- `core/src/dispatcher/model_router/intelligent_routing.rs` - Integration hooks
- `core/src/config/types/dispatcher.rs` - New configuration types
- `core/src/ffi_uniffi.rs` - UniFFI exports for budget status

### Dependencies
- Uses existing: `tokio`, `serde`, metrics from P0
- No new external dependencies

### Non-Breaking Changes
- All new APIs are additive
- Existing `route()` method unchanged
- Retry/budget features are opt-in via configuration
- Default behavior matches current (no retry, no budget limits)

## Success Criteria

1. **Retry**: Failed requests with retryable errors succeed within 3 attempts 90% of the time
2. **Failover**: When primary model is unhealthy, requests automatically route to healthy alternatives
3. **Budget**: Users can set daily limits that are enforced with <1% overage tolerance
4. **Visibility**: Real-time budget status available via UniFFI for UI display
5. **Performance**: <5ms overhead for budget check per request
