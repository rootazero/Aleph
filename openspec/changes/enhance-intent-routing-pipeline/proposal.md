# Change: Enhance Intent Routing Pipeline

## Status
- **Stage**: Proposed
- **Created**: 2026-01-11
- **Depends on**:
  - harden-dispatcher-data-processing (in progress)
  - introduce-native-function-calling (in progress)
  - unify-intent-detection-with-conversation (partial)
- **Supersedes**: None

## Why

### Current State Analysis

Aleph currently implements a multi-layer routing system (L1/L2/L3) with the following components:

**Existing Architecture:**
```
User Input
    ↓
UnifiedRouter
    ↓ (L1) Regex Match → High Confidence (1.0)
    ↓ (L2) SemanticMatcher → Medium Confidence (0.5-0.9)
    ↓ (L3) L3Router (AI Inference) → Variable Confidence
    ↓
DispatcherIntegration
    ↓
Tool Execution / General Chat
```

### Problems Identified

#### 1. **Disconnected L1/L2 Matching**

The `SemanticMatcher` handles both L1 (regex) and L2 (keyword) matching internally, but `UnifiedRouter` treats them as separate layers. This creates:
- Redundant matching attempts
- Inconsistent confidence scoring between layers
- No unified context passing across layers

```rust
// Current: SemanticMatcher runs BOTH L1+L2 internally
// Then UnifiedRouter calls it separately for "L1" and "L2"
async fn try_l1_regex(&self, ctx: &RoutingContext) -> Option<RoutingMatch> {
    let result = self.semantic_matcher.match_input(&matching_ctx).await;
    if result.confidence >= 0.9 { ... }  // Only accept high confidence
}

async fn try_l2_semantic(&self, ctx: &RoutingContext) -> Option<RoutingMatch> {
    let result = self.semantic_matcher.match_input(&matching_ctx).await;
    if result.confidence >= 0.5 { ... }  // Accept medium confidence
}
```

#### 2. **L3 Router Underutilized**

The L3 AI router has powerful capabilities but is only used as a fallback:
- No conversation context optimization
- No intent caching for repeated patterns
- Prompt is rebuilt every request (expensive)
- No feedback loop to improve L1/L2 rules

#### 3. **Tool-Intent Mapping Fragile**

`find_tool_for_intent()` uses heuristic matching that often creates synthetic tools:
```rust
// Current: Falls back to synthetic tool creation
debug!("No tool found for intent, creating synthetic tool");
Some(UnifiedTool::new(
    intent_type,
    intent_type,
    &format!("Inferred tool for intent: {}", intent_type),
    ToolSource::Custom { rule_index: 0 },
))
```

#### 4. **No Unified Confidence Model**

Different layers use different confidence scoring:
- L1: Fixed 1.0 for command match, configurable for regex
- L2: KeywordIndex returns raw score, then truncated to [0,1]
- L3: AI returns 0.0-1.0 with unclear calibration

#### 5. **Confirmation Flow Interrupts Processing**

The async confirmation flow is complex and can lose context:
- Pending confirmation stored in separate state
- Resume logic doesn't preserve full routing context
- Confirmation timeout results in lost user intent

### The Vision: Unified Intent-Aware Routing

Transform the 3-layer system into a cohesive **Intent Routing Pipeline** that:

1. **Single-pass Layer Processing**: Each layer contributes to a unified intent score
2. **Context-Aware L3**: AI routing uses conversation history and prior matches
3. **Dynamic Confidence Calibration**: Confidence thresholds adapt based on tool characteristics
4. **Intent Memory**: Cache successful intent matches to accelerate future routing
5. **Seamless Execution**: Tool execution integrated with routing for single-request completion

## What Changes

### 1. Unified Intent Pipeline Architecture

Replace the cascade model with a pipeline that aggregates signals:

```
User Input + Context
        ↓
┌───────────────────────────────────────────────────────────────┐
│                   Intent Routing Pipeline                      │
│                                                                │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐                    │
│  │  L1     │    │  L2     │    │  L3     │                    │
│  │ Fast    │ →  │ Keyword │ →  │ AI      │  All run in        │
│  │ Regex   │    │ Context │    │ Router  │  parallel when     │
│  └────┬────┘    └────┬────┘    └────┬────┘  needed            │
│       │              │              │                          │
│       ↓              ↓              ↓                          │
│  ┌──────────────────────────────────────────────┐             │
│  │            Intent Aggregator                  │             │
│  │  • Combines confidence signals                │             │
│  │  • Resolves conflicts                         │             │
│  │  • Applies tool-specific thresholds          │             │
│  └─────────────────────┬────────────────────────┘             │
│                        ↓                                       │
│  ┌──────────────────────────────────────────────┐             │
│  │            Intent Cache                       │             │
│  │  • Store successful matches                   │             │
│  │  • Fast-path for repeated patterns           │             │
│  └──────────────────────────────────────────────┘             │
└────────────────────────────────────────────────────────────────┘
        ↓
┌───────────────────────────────────────────────────────────────┐
│              Tool Executor (Unified Execution)                 │
│  • Direct execution for high-confidence matches               │
│  • Inline confirmation for medium-confidence                  │
│  • Clarification request for missing parameters               │
└───────────────────────────────────────────────────────────────┘
```

### 2. Intent Signal Aggregation

New `IntentSignal` type for unified scoring:

```rust
/// Signal from a routing layer
pub struct IntentSignal {
    /// Source layer that produced this signal
    pub layer: RoutingLayerType,

    /// Matched tool (if any)
    pub tool: Option<UnifiedTool>,

    /// Confidence score (0.0-1.0, calibrated)
    pub confidence: f32,

    /// Extracted parameters
    pub parameters: serde_json::Value,

    /// Reasoning for the match
    pub reason: String,

    /// Processing latency for this layer
    pub latency_ms: u64,
}

/// Aggregated intent from all layers
pub struct AggregatedIntent {
    /// Primary intent signal (highest confidence after calibration)
    pub primary: IntentSignal,

    /// Alternative signals (for disambiguation UI if needed)
    pub alternatives: Vec<IntentSignal>,

    /// Final confidence after aggregation
    pub final_confidence: f32,

    /// Whether parameters are complete
    pub parameters_complete: bool,

    /// Missing parameters (for clarification)
    pub missing_parameters: Vec<ParameterRequirement>,

    /// Recommended action
    pub action: IntentAction,
}

pub enum IntentAction {
    /// Execute tool directly (confidence >= auto_execute)
    Execute,
    /// Request user confirmation (confidence in range)
    RequestConfirmation,
    /// Request clarification for missing parameters
    RequestClarification { prompt: String, suggestions: Vec<String> },
    /// Fall back to general chat (no tool match)
    GeneralChat,
}
```

### 3. Enhanced L3 Router with Streaming

Optimize L3 for lower latency and better context:

```rust
pub struct EnhancedL3Router {
    /// AI provider for routing
    provider: Arc<dyn AiProvider>,

    /// Cached tool list (refreshed on registry change)
    tools_cache: Arc<RwLock<ToolsCache>>,

    /// Conversation context window
    context_window: usize,

    /// Enable parallel parameter extraction
    extract_params_parallel: bool,

    /// Intent pattern cache
    intent_cache: Arc<IntentCache>,
}

impl EnhancedL3Router {
    /// Route with streaming response for lower time-to-first-token
    pub async fn route_streaming(
        &self,
        input: &str,
        context: &RoutingContext,
    ) -> Result<impl Stream<Item = L3ProgressEvent>> {
        // 1. Check intent cache first
        if let Some(cached) = self.intent_cache.get(input).await {
            return Ok(stream::once(L3ProgressEvent::CachedMatch(cached)));
        }

        // 2. Build optimized prompt with only relevant tools
        let relevant_tools = self.filter_tools_for_input(input).await;

        // 3. Stream AI response
        self.provider.stream_routing(input, &relevant_tools, context).await
    }

    /// Pre-filter tools based on input characteristics
    async fn filter_tools_for_input(&self, input: &str) -> Vec<UnifiedTool> {
        // Use embedding similarity or keyword matching to reduce tool list
        // Smaller tool list = faster L3 inference
    }
}
```

### 4. Confidence Calibration System

Introduce calibrated confidence with tool-specific thresholds:

```rust
pub struct ConfidenceCalibrator {
    /// Global thresholds (from config)
    global: ConfidenceThresholds,

    /// Per-tool threshold overrides
    tool_overrides: HashMap<String, ToolConfidenceConfig>,

    /// Calibration history (for learning)
    history: Arc<RwLock<CalibrationHistory>>,
}

pub struct ToolConfidenceConfig {
    /// Minimum confidence to consider this tool
    pub min_threshold: f32,

    /// Confidence required to auto-execute
    pub auto_execute_threshold: f32,

    /// Whether to boost confidence for repeat patterns
    pub enable_repeat_boost: bool,

    /// Decay factor for cached confidence
    pub cache_decay_factor: f32,
}

impl ConfidenceCalibrator {
    /// Calibrate raw confidence based on tool and context
    pub fn calibrate(
        &self,
        raw_confidence: f32,
        tool: &UnifiedTool,
        layer: RoutingLayerType,
        context: &RoutingContext,
    ) -> CalibratedConfidence {
        let base = raw_confidence;

        // Apply layer-specific calibration
        let layer_calibrated = match layer {
            RoutingLayerType::L1Regex => base,  // L1 already calibrated
            RoutingLayerType::L2Semantic => self.calibrate_l2(base, context),
            RoutingLayerType::L3Inference => self.calibrate_l3(base, tool),
        };

        // Apply tool-specific adjustments
        let tool_adjusted = self.apply_tool_config(layer_calibrated, tool);

        // Apply repeat pattern boost if applicable
        let final_confidence = self.apply_repeat_boost(tool_adjusted, tool, context);

        CalibratedConfidence {
            raw: raw_confidence,
            calibrated: final_confidence,
            layer,
            tool_name: tool.name.clone(),
        }
    }
}
```

### 5. Intent Cache for Fast Path

Cache successful intent matches to accelerate repeated patterns:

```rust
pub struct IntentCache {
    /// Cache entries indexed by normalized input hash
    entries: Arc<RwLock<LruCache<u64, CachedIntent>>>,

    /// TTL for cache entries
    ttl: Duration,

    /// Maximum cache size
    max_size: usize,
}

pub struct CachedIntent {
    /// Original input pattern
    pub pattern: String,

    /// Matched tool
    pub tool_name: String,

    /// Cached confidence
    pub confidence: f32,

    /// Time cached
    pub cached_at: Instant,

    /// Hit count (for boosting)
    pub hit_count: u32,

    /// Success rate (for learning)
    pub success_rate: f32,
}

impl IntentCache {
    /// Record a successful tool execution for learning
    pub fn record_success(&self, input: &str, tool: &str) {
        // Update success rate, potentially add to cache
    }

    /// Record a failed/cancelled execution
    pub fn record_failure(&self, input: &str, tool: &str) {
        // Decrease confidence, potentially remove from cache
    }
}
```

### 6. Integrated Clarification Flow

Integrate clarification directly into the routing pipeline:

```rust
pub struct ClarificationIntegrator {
    /// Pending clarifications (keyed by session ID)
    pending: Arc<RwLock<HashMap<String, PendingClarification>>>,
}

pub struct PendingClarification {
    /// Original routing context (preserved)
    pub original_context: RoutingContext,

    /// Intent that needs clarification
    pub intent: AggregatedIntent,

    /// Missing parameter being clarified
    pub missing_param: ParameterRequirement,

    /// Timestamp
    pub created_at: Instant,
}

impl ClarificationIntegrator {
    /// Start clarification flow
    pub fn request_clarification(
        &self,
        intent: AggregatedIntent,
        context: RoutingContext,
    ) -> ClarificationRequest {
        // Preserve full context
        let session_id = self.create_session(intent.clone(), context);

        // Build clarification request with smart suggestions
        ClarificationRequest::builder()
            .session_id(session_id)
            .prompt(&intent.missing_parameters[0].clarification_prompt)
            .suggestions(self.generate_suggestions(&intent))
            .build()
    }

    /// Resume routing after clarification
    pub async fn resume_with_input(
        &self,
        session_id: &str,
        user_input: &str,
    ) -> Result<RoutingResult> {
        // Restore context
        let pending = self.pending.write().await.remove(session_id)?;

        // Augment original input with clarification
        let augmented_context = pending.original_context
            .with_param(pending.missing_param.name.clone(), user_input);

        // Re-route with augmented context (skip L3 if tool already determined)
        self.fast_reroute(augmented_context, &pending.intent).await
    }
}
```

### 7. Pipeline Coordinator

Single entry point for all routing:

```rust
pub struct IntentRoutingPipeline {
    /// L1 fast regex matcher
    l1_matcher: Arc<FastRegexMatcher>,

    /// L2 semantic matcher
    l2_matcher: Arc<SemanticMatcher>,

    /// L3 AI router
    l3_router: Arc<EnhancedL3Router>,

    /// Intent aggregator
    aggregator: IntentAggregator,

    /// Confidence calibrator
    calibrator: ConfidenceCalibrator,

    /// Intent cache
    cache: Arc<IntentCache>,

    /// Clarification integrator
    clarification: ClarificationIntegrator,

    /// Tool executor
    executor: ToolExecutor,

    /// Configuration
    config: PipelineConfig,
}

impl IntentRoutingPipeline {
    /// Process user input end-to-end
    pub async fn process(
        &self,
        input: &str,
        context: RoutingContext,
        event_handler: &dyn AlephEventHandler,
    ) -> Result<PipelineResult> {
        // 1. Check cache for fast path
        if let Some(cached) = self.cache.get(input).await {
            if cached.confidence >= self.config.cache_auto_execute {
                return self.execute_cached(cached, context).await;
            }
        }

        // 2. Run L1 (always)
        let l1_signal = self.l1_matcher.match_input(input).await;

        // 3. Early exit if L1 matches with high confidence
        if l1_signal.as_ref().map(|s| s.confidence >= 0.95).unwrap_or(false) {
            let intent = self.aggregator.from_single(l1_signal.unwrap());
            return self.handle_intent(intent, context, event_handler).await;
        }

        // 4. Run L2 (parallel with L3 if enabled)
        let (l2_signal, l3_signal) = if self.config.parallel_l2_l3 {
            tokio::join!(
                self.l2_matcher.match_input(&context),
                self.l3_router.route_if_needed(&context, l1_signal.as_ref())
            )
        } else {
            let l2 = self.l2_matcher.match_input(&context).await;
            let l3 = if l2.confidence < self.config.l3_trigger_threshold {
                Some(self.l3_router.route(&context).await?)
            } else {
                None
            };
            (l2, l3)
        };

        // 5. Aggregate signals
        let signals = vec![l1_signal, l2_signal, l3_signal]
            .into_iter()
            .flatten()
            .collect();
        let intent = self.aggregator.aggregate(signals, &context);

        // 6. Handle aggregated intent
        self.handle_intent(intent, context, event_handler).await
    }

    /// Handle aggregated intent based on action
    async fn handle_intent(
        &self,
        intent: AggregatedIntent,
        context: RoutingContext,
        event_handler: &dyn AlephEventHandler,
    ) -> Result<PipelineResult> {
        match intent.action {
            IntentAction::Execute => {
                // Direct execution
                let result = self.executor.execute(&intent.primary, &context).await?;
                self.cache.record_success(&context.input, &intent.primary.tool.name);
                Ok(PipelineResult::Executed(result))
            }
            IntentAction::RequestConfirmation => {
                // Show inline confirmation
                let confirmed = event_handler.on_tool_confirmation(&intent.primary.tool);
                if confirmed {
                    let result = self.executor.execute(&intent.primary, &context).await?;
                    self.cache.record_success(&context.input, &intent.primary.tool.name);
                    Ok(PipelineResult::Executed(result))
                } else {
                    self.cache.record_failure(&context.input, &intent.primary.tool.name);
                    Ok(PipelineResult::Cancelled)
                }
            }
            IntentAction::RequestClarification { prompt, suggestions } => {
                let request = self.clarification.request_clarification(intent, context);
                event_handler.on_clarification_needed(request.clone());
                Ok(PipelineResult::PendingClarification(request))
            }
            IntentAction::GeneralChat => {
                Ok(PipelineResult::GeneralChat)
            }
        }
    }
}
```

## Impact

### Affected Specs
- **Modified**: `ai-routing` - Update for unified pipeline
- **New**: `intent-routing-pipeline` - Pipeline architecture
- **New**: `intent-cache` - Caching requirements
- **New**: `confidence-calibration` - Calibration system

### Affected Code
- **Modify**: `dispatcher/integration.rs` - Use new pipeline
- **Modify**: `dispatcher/l3_router.rs` - Enhanced streaming and caching
- **Add**: `routing/pipeline.rs` - New pipeline coordinator
- **Add**: `routing/aggregator.rs` - Intent signal aggregation
- **Add**: `routing/cache.rs` - Intent cache implementation
- **Add**: `routing/calibrator.rs` - Confidence calibration
- **Modify**: `semantic/matcher.rs` - Expose layer-specific matching
- **Modify**: `core.rs` - Wire up new pipeline

### Breaking Changes
- **Internal API**: `DispatcherIntegration.route_with_confirmation()` signature changes
- **Config**: New `[routing.pipeline]` config section
- **No UniFFI changes**: Swift APIs remain compatible

### Performance Impact
- **Improved**: Cache hits reduce L3 calls by ~70% (estimated)
- **Improved**: Parallel L2+L3 reduces p95 latency by ~40%
- **Improved**: Tool pre-filtering reduces L3 prompt size
- **Trade-off**: Cache memory usage (~5MB for 10K entries)

## Migration Strategy

### Phase 1: Intent Aggregator (Week 1)
1. Implement `IntentSignal` and `AggregatedIntent` types
2. Add `IntentAggregator` with basic aggregation logic
3. Wire into existing `DispatcherIntegration` as opt-in

### Phase 2: Confidence Calibration (Week 1)
1. Implement `ConfidenceCalibrator` with tool config
2. Add calibration to existing L1/L2/L3 outputs
3. Add config section for per-tool thresholds

### Phase 3: Intent Cache (Week 2)
1. Implement `IntentCache` with LRU eviction
2. Add success/failure recording
3. Enable fast-path for cached intents

### Phase 4: Enhanced L3 Router (Week 2)
1. Add tool pre-filtering
2. Implement streaming route (if provider supports)
3. Integrate with intent cache

### Phase 5: Pipeline Coordinator (Week 3)
1. Implement `IntentRoutingPipeline`
2. Add parallel L2+L3 execution
3. Integrate clarification flow

### Phase 6: Migration and Cleanup (Week 3)
1. Migrate `AlephCore` to use pipeline
2. Remove legacy `route_with_confirmation` path
3. Update tests

## Success Criteria

1. **Latency**: p50 intent detection < 100ms (cache hit) / < 500ms (cache miss)
2. **Accuracy**: Intent match accuracy >= 95% for top-10 commands
3. **Cache Hit Rate**: >= 60% for repeat patterns
4. **L3 Reduction**: L3 calls reduced by >= 50% via cache + L1/L2 improvements
5. **User Satisfaction**: Confirmation dialogs reduced by >= 70% for high-confidence matches
6. **Tests Pass**: All existing routing tests pass with new pipeline

## References

- Current implementation: `dispatcher/integration.rs` (DispatcherIntegration)
- Current L3: `dispatcher/l3_router.rs` (L3Router)
- Current semantic: `semantic/matcher.rs` (SemanticMatcher)
- Related changes: `introduce-native-function-calling`, `unify-intent-detection-with-conversation`
- Pattern inspiration: LangChain Router, AutoGPT intent detection
