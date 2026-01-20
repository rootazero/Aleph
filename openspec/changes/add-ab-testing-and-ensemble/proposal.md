# Change: Add A/B Testing Framework and Multi-Model Ensemble to Model Router

## Why

The Model Router has evolved through P0 (runtime metrics, health monitoring), P1 (retry/failover, budget management), and P2 (prompt analysis, semantic cache). The system now needs P3 capabilities for experimentation and reliability optimization.

### Problem 1: No Way to Experiment with Routing Strategies

Current routing is deterministic:
- Same input always routes to the same model
- Cannot compare different routing strategies in production
- No data-driven way to optimize routing decisions
- Cannot A/B test new models before full rollout
- Cannot measure the impact of cost strategy changes

**What's Missing**: An experimentation framework that can:
- Randomly assign users/requests to different strategies (treatment groups)
- Track outcomes per strategy (latency, cost, quality, user satisfaction)
- Provide statistical significance testing for results
- Enable gradual rollout of new routing logic

**Impact**: Routing optimizations are based on intuition rather than data; suboptimal strategies persist because there's no way to measure alternatives.

### Problem 2: Single Model Selection Limits Reliability and Quality

Current routing selects exactly one model per request:
- If that model produces a poor response, user gets poor results
- No cross-validation between models for critical tasks
- Cannot leverage model diversity for better outputs
- High-stakes tasks have same reliability as casual queries

**What's Missing**: A multi-model ensemble system that can:
- Route critical tasks to multiple models simultaneously
- Aggregate responses using voting, consensus, or quality scoring
- Provide higher confidence outputs for important decisions
- Detect model hallucinations through cross-validation

**Impact**: Critical tasks (code generation, reasoning) rely on single model's output quality; user cannot trade cost for higher reliability.

## What Changes

### 1. A/B Testing Framework (NEW)

A controlled experimentation layer for routing strategy comparison:

```
┌─────────────────────────────────────────────────────────────────┐
│                     ABTestingEngine                              │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ Experiment   │  │ TrafficSplit │  │   OutcomeTracker       │ │
│  │ Config       │  │ Manager      │  │   (metrics per group)  │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ Assignment   │  │ Significance │  │   ExperimentReport     │ │
│  │ Strategy     │  │ Calculator   │  │   (results analysis)   │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

**Key Features**:
- **ExperimentConfig**: Define experiments with name, variants, traffic allocation
- **TrafficSplitManager**: Consistent hashing for stable user→variant assignment
- **AssignmentStrategy**: User-based, request-based, or feature-based splitting
- **OutcomeTracker**: Collect metrics per variant (latency, cost, quality signals)
- **SignificanceCalculator**: T-test, chi-square for statistical validity
- **ExperimentReport**: Human-readable and JSON summary of results

### 2. Multi-Model Ensemble (NEW)

A reliability and quality enhancement layer using model diversity:

```
┌─────────────────────────────────────────────────────────────────┐
│                     EnsembleEngine                               │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ Ensemble     │  │ Parallel     │  │   ResponseAggregator   │ │
│  │ Strategy     │  │ Executor     │  │   (combine outputs)    │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │ QualityScore │  │ Consensus    │  │   EnsembleResult       │ │
│  │ Estimator    │  │ Detector     │  │   (final + metadata)   │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

**Key Features**:
- **EnsembleStrategy**: Configure which tasks use ensemble and how many models
- **ParallelExecutor**: Run multiple models concurrently with timeout handling
- **ResponseAggregator**: Combine responses using voting, best-of-n, or weighted
- **QualityScoreEstimator**: Heuristic scoring of response quality (length, structure, confidence markers)
- **ConsensusDetector**: Identify agreement/disagreement between model outputs
- **EnsembleResult**: Final response with confidence score and model attributions

### 3. Integration with Existing Components

```
┌─────────────────────────────────────────────────────────────────┐
│                     P3IntelligentRouter                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                    [P2] Pre-Route Layer                  │   │
│  │  - SemanticCache.lookup()                                │   │
│  │  - PromptAnalyzer.analyze()                              │   │
│  └──────────────────────────────────────────────────────────┘   │
│                               ↓                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │              [NEW] A/B Testing Decision                  │   │
│  │  - Check if request matches active experiment            │   │
│  │  - Assign to variant based on user_id/session_id         │   │
│  │  - Override routing strategy per variant                 │   │
│  └──────────────────────────────────────────────────────────┘   │
│                               ↓                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │              [NEW] Ensemble Decision                     │   │
│  │  - Check if task qualifies for ensemble (critical/high$) │   │
│  │  - Select models for ensemble based on strategy          │   │
│  │  - Execute in parallel if ensemble is enabled            │   │
│  └──────────────────────────────────────────────────────────┘   │
│                               ↓                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                 [P1] Execute Layer                       │   │
│  │  - RetryOrchestrator.execute()                           │   │
│  │  - Budget tracking                                       │   │
│  │  - Outcome recording for A/B analysis                    │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**NEW Configuration**:
```toml
[cowork.model_routing.ab_testing]
enabled = true
default_assignment = "user_id"  # user_id | session_id | request_id

[[cowork.model_routing.ab_testing.experiments]]
name = "gemini-vs-claude-reasoning"
enabled = true
traffic_percentage = 10  # 10% of traffic enters experiment
variants = [
    { name = "control", model = "claude-sonnet", weight = 50 },
    { name = "treatment", model = "gemini-pro", weight = 50 }
]
target_intent = "Reasoning"  # Only for reasoning tasks
metrics = ["latency_ms", "cost_usd", "user_rating"]  # Tracked outcomes

[cowork.model_routing.ensemble]
enabled = true
default_strategy = "disabled"  # disabled | best_of_n | voting | consensus

[cowork.model_routing.ensemble.strategies.reasoning]
mode = "best_of_n"
n = 2
models = ["claude-opus", "gpt-4o"]
timeout_ms = 30000
quality_metric = "length_and_structure"

[cowork.model_routing.ensemble.strategies.code_generation]
mode = "consensus"
models = ["claude-sonnet", "gpt-4o", "gemini-pro"]
timeout_ms = 20000
min_agreement = 0.7  # 70% similarity required
```

## Impact

### Affected Specs
- **NEW**: `ab-testing` - A/B experimentation capability
- **NEW**: `model-ensemble` - Multi-model execution capability
- **MODIFIED**: `ai-routing` - Integration points (minimal)

### Affected Code
- `core/src/dispatcher/model_router/ab_testing.rs` - NEW: ABTestingEngine, ExperimentConfig
- `core/src/dispatcher/model_router/ensemble.rs` - NEW: EnsembleEngine, ResponseAggregator
- `core/src/dispatcher/model_router/p3_router.rs` - NEW: P3IntelligentRouter
- `core/src/dispatcher/model_router/mod.rs` - Export new modules
- `core/src/config/types/cowork.rs` - New configuration types
- `core/src/ffi/exports.rs` - UniFFI exports for experiment status and results

### Dependencies
- Uses existing: `tokio`, `serde`, P2 infrastructure (prompt analysis, metrics)
- New: `siphasher` or similar for consistent hashing (traffic splitting)
- New: `statrs` for statistical significance calculations (optional, can be simple)

### Non-Breaking Changes
- All new APIs are additive
- A/B testing and ensemble are opt-in via configuration
- Default behavior unchanged (single model, no experiments)
- Existing `route()` method remains functional

## Success Criteria

1. **A/B Testing**:
   - Stable variant assignment: same user_id always gets same variant
   - Traffic split accuracy within 2% of configured percentages
   - Outcome tracking for all specified metrics
   - Significance calculation returns valid p-values after sufficient data

2. **Ensemble**:
   - Parallel execution completes within timeout
   - Response aggregation produces deterministic results
   - Quality scoring correlates with actual response quality
   - Consensus detection accurately identifies agreement

3. **Performance**:
   - A/B assignment overhead <1ms per request
   - Ensemble adds only parallel execution time (not sequential)
   - Memory footprint <50MB for experiment tracking (10 active experiments)

4. **Integration**:
   - P2 semantic cache still works (no double-caching)
   - P1 retry/failover applies to ensemble members
   - Budget tracking accounts for ensemble cost multiplier
