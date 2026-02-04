# Design: Enhanced Intent Routing Pipeline

## Architecture Overview

This document describes the technical architecture for the enhanced intent routing pipeline.

## System Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Intent Routing Pipeline                          │
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                      Input Preprocessor                          │   │
│  │  • Normalize input (trim, lowercase for matching)               │   │
│  │  • Extract attachments/multimodal content                       │   │
│  │  • Build RoutingContext with conversation history               │   │
│  └────────────────────────────┬────────────────────────────────────┘   │
│                               ↓                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                      Cache Lookup (Fast Path)                    │   │
│  │  • Hash normalized input                                         │   │
│  │  • Check IntentCache for recent matches                         │   │
│  │  • If hit with high confidence → skip to Executor               │   │
│  └────────────────────────────┬────────────────────────────────────┘   │
│                               ↓                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                    Layer Execution Engine                        │   │
│  │                                                                   │   │
│  │   ┌─────────┐   ┌─────────┐   ┌─────────┐                       │   │
│  │   │   L1    │   │   L2    │   │   L3    │                       │   │
│  │   │ Regex   │   │Semantic │   │   AI    │                       │   │
│  │   │ <10ms   │   │200-500ms│   │ >1s     │                       │   │
│  │   └────┬────┘   └────┬────┘   └────┬────┘                       │   │
│  │        │             │             │                             │   │
│  │   IntentSignal  IntentSignal  IntentSignal                      │   │
│  │        │             │             │                             │   │
│  │        └─────────────┴─────────────┘                            │   │
│  │                      │                                           │   │
│  │                      ↓                                           │   │
│  │        ┌─────────────────────────────┐                          │   │
│  │        │    Intent Aggregator        │                          │   │
│  │        │    • Collect all signals    │                          │   │
│  │        │    • Apply calibration      │                          │   │
│  │        │    • Resolve conflicts      │                          │   │
│  │        │    • Determine action       │                          │   │
│  │        └──────────────┬──────────────┘                          │   │
│  │                       ↓                                          │   │
│  │        ┌─────────────────────────────┐                          │   │
│  │        │   Confidence Calibrator     │                          │   │
│  │        │   • Layer-specific adjust   │                          │   │
│  │        │   • Tool-specific config    │                          │   │
│  │        │   • History-based boost     │                          │   │
│  │        └─────────────────────────────┘                          │   │
│  └────────────────────────────┬────────────────────────────────────┘   │
│                               ↓                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                    Action Router                                 │   │
│  │                                                                   │   │
│  │   ┌───────────┐  ┌───────────┐  ┌───────────┐  ┌───────────┐   │   │
│  │   │  Execute  │  │ Confirm   │  │ Clarify   │  │ General   │   │   │
│  │   │  (≥0.9)   │  │(0.7-0.9)  │  │ (missing  │  │   Chat    │   │   │
│  │   │           │  │           │  │  params)  │  │  (<0.3)   │   │   │
│  │   └─────┬─────┘  └─────┬─────┘  └─────┬─────┘  └─────┬─────┘   │   │
│  │         │              │              │              │          │   │
│  └─────────┼──────────────┼──────────────┼──────────────┼──────────┘   │
│            ↓              ↓              ↓              ↓               │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                    Execution Layer                               │   │
│  │                                                                   │   │
│  │   ToolExecutor ─────────────────────────────────────────→ Result│   │
│  │        ↑                                                         │   │
│  │   ConfirmationUI ──→ User Decision ──→ Execute/Cancel           │   │
│  │        ↑                                                         │   │
│  │   ClarificationUI ──→ User Input ──→ Re-route with params       │   │
│  │        ↑                                                         │   │
│  │   GeneralChatHandler ──→ AI Response ──→ Result                 │   │
│  │                                                                   │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                               ↓                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                    Feedback Loop                                 │   │
│  │  • Record success/failure to IntentCache                        │   │
│  │  • Update confidence history for calibration                    │   │
│  │  • Log for analytics                                            │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

## Component Details

### 1. Input Preprocessor

**Responsibility**: Transform raw user input into normalized `RoutingContext`

```rust
pub struct InputPreprocessor {
    /// Conversation history manager
    conversation: Arc<ConversationManager>,
    /// Multimodal content handler
    multimodal: MultimodalHandler,
}

impl InputPreprocessor {
    pub fn preprocess(&self, raw_input: &str, session_id: &str) -> RoutingContext {
        // 1. Normalize text
        let normalized = raw_input.trim();

        // 2. Extract command prefix if present
        let (command, content) = self.extract_command(normalized);

        // 3. Build conversation context
        let conversation = self.conversation.get_context(session_id);

        // 4. Extract entity hints from recent conversation
        let entity_hints = conversation.extract_entity_hints();

        RoutingContext {
            input: normalized.to_string(),
            command_prefix: command,
            content_without_command: content,
            conversation: Some(conversation),
            entity_hints,
            multimodal_attachments: vec![],
            timestamp: Instant::now(),
        }
    }

    fn extract_command(&self, input: &str) -> (Option<String>, String) {
        if input.starts_with('/') {
            let parts: Vec<&str> = input.splitn(2, ' ').collect();
            let command = parts[0].trim_start_matches('/').to_string();
            let content = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
            (Some(command), content)
        } else {
            (None, input.to_string())
        }
    }
}
```

### 2. Intent Cache

**Responsibility**: Store and retrieve successful intent matches

```rust
pub struct IntentCache {
    /// LRU cache with capacity limit
    cache: Arc<RwLock<LruCache<u64, CachedIntent>>>,
    /// Configuration
    config: CacheConfig,
    /// Metrics
    metrics: CacheMetrics,
}

#[derive(Clone)]
pub struct CachedIntent {
    /// Normalized input pattern (for logging/debugging)
    pub pattern: String,
    /// Matched tool name
    pub tool_name: String,
    /// Extracted parameters (if any)
    pub parameters: serde_json::Value,
    /// Cached confidence (decays over time)
    pub confidence: f32,
    /// Original confidence from routing
    pub original_confidence: f32,
    /// When cached
    pub cached_at: Instant,
    /// Number of times this cache entry was hit
    pub hit_count: u32,
    /// Success count (tool execution succeeded)
    pub success_count: u32,
    /// Failure count (user cancelled or tool failed)
    pub failure_count: u32,
}

impl IntentCache {
    /// Get cached intent, applying time decay
    pub async fn get(&self, input: &str) -> Option<CachedIntent> {
        let hash = self.hash_input(input);
        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get_mut(&hash) {
            // Apply time decay to confidence
            let age = entry.cached_at.elapsed();
            let decay = (-age.as_secs_f32() / self.config.decay_half_life_secs).exp();
            let decayed_confidence = entry.original_confidence * decay;

            // Apply success rate adjustment
            let total = entry.success_count + entry.failure_count;
            let success_rate = if total > 0 {
                entry.success_count as f32 / total as f32
            } else {
                1.0
            };
            let adjusted_confidence = decayed_confidence * success_rate;

            // Increment hit count
            entry.hit_count += 1;
            entry.confidence = adjusted_confidence;

            self.metrics.record_hit();
            Some(entry.clone())
        } else {
            self.metrics.record_miss();
            None
        }
    }

    /// Add new cache entry
    pub async fn put(&self, input: &str, tool: &str, parameters: serde_json::Value, confidence: f32) {
        let hash = self.hash_input(input);
        let entry = CachedIntent {
            pattern: input.to_string(),
            tool_name: tool.to_string(),
            parameters,
            confidence,
            original_confidence: confidence,
            cached_at: Instant::now(),
            hit_count: 0,
            success_count: 0,
            failure_count: 0,
        };

        let mut cache = self.cache.write().await;
        cache.put(hash, entry);
    }

    /// Record successful execution
    pub async fn record_success(&self, input: &str) {
        let hash = self.hash_input(input);
        if let Some(entry) = self.cache.write().await.get_mut(&hash) {
            entry.success_count += 1;
        }
    }

    /// Record failed/cancelled execution
    pub async fn record_failure(&self, input: &str) {
        let hash = self.hash_input(input);
        if let Some(entry) = self.cache.write().await.get_mut(&hash) {
            entry.failure_count += 1;
            // Consider removing if failure rate too high
            if entry.failure_count > 3 && entry.success_count == 0 {
                self.cache.write().await.pop(&hash);
            }
        }
    }

    fn hash_input(&self, input: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let normalized = input.trim().to_lowercase();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        normalized.hash(&mut hasher);
        hasher.finish()
    }
}
```

### 3. Layer Execution Engine

**Responsibility**: Execute L1/L2/L3 layers and collect signals

```rust
pub struct LayerExecutionEngine {
    l1: Arc<L1RegexMatcher>,
    l2: Arc<L2SemanticMatcher>,
    l3: Arc<L3AiRouter>,
    config: LayerConfig,
}

impl LayerExecutionEngine {
    /// Execute layers based on configuration
    pub async fn execute(&self, ctx: &RoutingContext) -> Vec<IntentSignal> {
        let mut signals = Vec::new();

        // L1 always runs (fast)
        let l1_start = Instant::now();
        if let Some(signal) = self.l1.match_input(&ctx.input).await {
            signals.push(signal.with_latency(l1_start.elapsed().as_millis() as u64));

            // Early exit if L1 is high confidence
            if signal.confidence >= self.config.l1_auto_accept {
                return signals;
            }
        }

        // Determine L2/L3 execution strategy
        match self.config.execution_mode {
            ExecutionMode::Sequential => {
                // L2 first, then L3 if needed
                let l2_start = Instant::now();
                if let Some(signal) = self.l2.match_input(ctx).await {
                    signals.push(signal.with_latency(l2_start.elapsed().as_millis() as u64));

                    if signal.confidence >= self.config.l2_skip_l3_threshold {
                        return signals;
                    }
                }

                // Run L3
                let l3_start = Instant::now();
                if let Some(signal) = self.l3.route(ctx).await {
                    signals.push(signal.with_latency(l3_start.elapsed().as_millis() as u64));
                }
            }
            ExecutionMode::Parallel => {
                // Run L2 and L3 concurrently
                let (l2_result, l3_result) = tokio::join!(
                    async {
                        let start = Instant::now();
                        self.l2.match_input(ctx).await.map(|s|
                            s.with_latency(start.elapsed().as_millis() as u64)
                        )
                    },
                    async {
                        let start = Instant::now();
                        self.l3.route(ctx).await.map(|s|
                            s.with_latency(start.elapsed().as_millis() as u64)
                        )
                    }
                );

                if let Some(signal) = l2_result {
                    signals.push(signal);
                }
                if let Some(signal) = l3_result {
                    signals.push(signal);
                }
            }
            ExecutionMode::L1Only => {
                // Already handled above
            }
        }

        signals
    }
}

#[derive(Clone, Copy)]
pub enum ExecutionMode {
    /// Run L2 first, then L3 if L2 confidence too low
    Sequential,
    /// Run L2 and L3 in parallel
    Parallel,
    /// Only run L1 (fastest, for explicit commands only)
    L1Only,
}
```

### 4. Intent Aggregator

**Responsibility**: Combine signals into final intent decision

```rust
pub struct IntentAggregator {
    calibrator: ConfidenceCalibrator,
    thresholds: ConfidenceThresholds,
}

impl IntentAggregator {
    /// Aggregate multiple signals into a single intent
    pub fn aggregate(
        &self,
        signals: Vec<IntentSignal>,
        ctx: &RoutingContext,
    ) -> AggregatedIntent {
        if signals.is_empty() {
            return AggregatedIntent::general_chat();
        }

        // Calibrate all signals
        let calibrated: Vec<CalibratedSignal> = signals
            .into_iter()
            .map(|s| self.calibrator.calibrate(s, ctx))
            .collect();

        // Sort by calibrated confidence
        let mut sorted = calibrated;
        sorted.sort_by(|a, b| b.calibrated_confidence.partial_cmp(&a.calibrated_confidence).unwrap());

        let primary = sorted.remove(0);

        // Check for conflicts (multiple high-confidence different tools)
        let has_conflict = sorted.iter().any(|s|
            s.calibrated_confidence > 0.7 &&
            s.signal.tool.as_ref().map(|t| &t.name) != primary.signal.tool.as_ref().map(|t| &t.name)
        );

        // Determine action
        let action = self.determine_action(&primary, has_conflict);

        // Check for missing parameters
        let missing_params = if let Some(ref tool) = primary.signal.tool {
            self.find_missing_params(tool, &primary.signal.parameters)
        } else {
            vec![]
        };

        let action = if !missing_params.is_empty() {
            IntentAction::RequestClarification {
                prompt: missing_params[0].clarification_prompt.clone(),
                suggestions: missing_params[0].suggestions.clone(),
            }
        } else {
            action
        };

        AggregatedIntent {
            primary: primary.signal,
            alternatives: sorted.into_iter().map(|s| s.signal).collect(),
            final_confidence: primary.calibrated_confidence,
            parameters_complete: missing_params.is_empty(),
            missing_parameters: missing_params,
            action,
        }
    }

    fn determine_action(&self, primary: &CalibratedSignal, has_conflict: bool) -> IntentAction {
        let confidence = primary.calibrated_confidence;

        if primary.signal.tool.is_none() || confidence < self.thresholds.no_match {
            return IntentAction::GeneralChat;
        }

        if confidence >= self.thresholds.auto_execute && !has_conflict {
            return IntentAction::Execute;
        }

        if confidence >= self.thresholds.requires_confirmation {
            return IntentAction::RequestConfirmation;
        }

        // Low confidence but has a match - request confirmation
        IntentAction::RequestConfirmation
    }

    fn find_missing_params(
        &self,
        tool: &UnifiedTool,
        provided: &serde_json::Value,
    ) -> Vec<ParameterRequirement> {
        let mut missing = vec![];

        if let Some(schema) = &tool.parameters_schema {
            if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
                for param in required {
                    if let Some(param_name) = param.as_str() {
                        if !provided.get(param_name).map(|v| !v.is_null()).unwrap_or(false) {
                            // Parameter is missing
                            let properties = schema.get("properties").and_then(|p| p.get(param_name));
                            let description = properties
                                .and_then(|p| p.get("description"))
                                .and_then(|d| d.as_str())
                                .unwrap_or(param_name);

                            missing.push(ParameterRequirement {
                                name: param_name.to_string(),
                                param_type: "string".to_string(),
                                required: true,
                                description: description.to_string(),
                                clarification_prompt: format!("请提供 {} 参数：", description),
                                suggestions: vec![],
                            });
                        }
                    }
                }
            }
        }

        missing
    }
}
```

### 5. Confidence Calibrator

**Responsibility**: Adjust raw confidence based on various factors

```rust
pub struct ConfidenceCalibrator {
    /// Global thresholds
    global: ConfidenceThresholds,
    /// Tool-specific configurations
    tool_configs: HashMap<String, ToolConfidenceConfig>,
    /// Historical calibration data
    history: Arc<RwLock<CalibrationHistory>>,
}

pub struct CalibratedSignal {
    pub signal: IntentSignal,
    pub raw_confidence: f32,
    pub calibrated_confidence: f32,
    pub calibration_factors: Vec<CalibrationFactor>,
}

pub struct CalibrationFactor {
    pub name: String,
    pub adjustment: f32,
    pub reason: String,
}

impl ConfidenceCalibrator {
    pub fn calibrate(&self, signal: IntentSignal, ctx: &RoutingContext) -> CalibratedSignal {
        let raw = signal.confidence;
        let mut calibrated = raw;
        let mut factors = vec![];

        // 1. Layer-specific calibration
        let layer_factor = self.apply_layer_calibration(&signal, &mut calibrated);
        if let Some(f) = layer_factor {
            factors.push(f);
        }

        // 2. Tool-specific calibration
        if let Some(ref tool) = signal.tool {
            if let Some(config) = self.tool_configs.get(&tool.name) {
                let tool_factor = self.apply_tool_calibration(config, &mut calibrated);
                if let Some(f) = tool_factor {
                    factors.push(f);
                }
            }
        }

        // 3. Context-based calibration
        let context_factor = self.apply_context_calibration(ctx, &signal, &mut calibrated);
        if let Some(f) = context_factor {
            factors.push(f);
        }

        // 4. History-based boost
        if let Some(ref tool) = signal.tool {
            let history_factor = self.apply_history_boost(&tool.name, ctx, &mut calibrated);
            if let Some(f) = history_factor {
                factors.push(f);
            }
        }

        // Clamp to [0, 1]
        calibrated = calibrated.clamp(0.0, 1.0);

        CalibratedSignal {
            signal,
            raw_confidence: raw,
            calibrated_confidence: calibrated,
            calibration_factors: factors,
        }
    }

    fn apply_layer_calibration(
        &self,
        signal: &IntentSignal,
        confidence: &mut f32,
    ) -> Option<CalibrationFactor> {
        match signal.layer {
            RoutingLayerType::L1Regex => {
                // L1 is already well-calibrated, no adjustment
                None
            }
            RoutingLayerType::L2Semantic => {
                // L2 keyword matching can be over-confident
                // Apply slight dampening for non-exact matches
                if *confidence > 0.7 && *confidence < 0.95 {
                    let adjustment = -0.05;
                    *confidence += adjustment;
                    Some(CalibrationFactor {
                        name: "l2_dampening".to_string(),
                        adjustment,
                        reason: "L2 semantic match dampening for non-exact match".to_string(),
                    })
                } else {
                    None
                }
            }
            RoutingLayerType::L3Inference => {
                // L3 confidence is model-dependent
                // Some models are overconfident, apply correction
                let adjustment = -0.1; // Conservative by default
                *confidence += adjustment;
                Some(CalibrationFactor {
                    name: "l3_model_correction".to_string(),
                    adjustment,
                    reason: "L3 AI model confidence correction".to_string(),
                })
            }
            RoutingLayerType::Default => None,
        }
    }

    fn apply_tool_calibration(
        &self,
        config: &ToolConfidenceConfig,
        confidence: &mut f32,
    ) -> Option<CalibrationFactor> {
        // Apply tool-specific minimum threshold
        if *confidence < config.min_threshold {
            let adjustment = config.min_threshold - *confidence;
            *confidence = config.min_threshold;
            return Some(CalibrationFactor {
                name: "tool_min_threshold".to_string(),
                adjustment,
                reason: "Tool-specific minimum threshold applied".to_string(),
            });
        }

        None
    }

    fn apply_context_calibration(
        &self,
        ctx: &RoutingContext,
        signal: &IntentSignal,
        confidence: &mut f32,
    ) -> Option<CalibrationFactor> {
        // Boost confidence if tool was used recently in conversation
        if let Some(ref conv) = ctx.conversation {
            if let Some(ref tool) = signal.tool {
                let recent_uses = conv.recent_tool_uses(&tool.name, 3);
                if recent_uses > 0 {
                    let boost = 0.05 * recent_uses as f32;
                    *confidence += boost;
                    return Some(CalibrationFactor {
                        name: "recent_use_boost".to_string(),
                        adjustment: boost,
                        reason: format!("Tool used {} times in last 3 turns", recent_uses),
                    });
                }
            }
        }

        None
    }

    fn apply_history_boost(
        &self,
        tool_name: &str,
        ctx: &RoutingContext,
        confidence: &mut f32,
    ) -> Option<CalibrationFactor> {
        // Check if this exact pattern has succeeded before
        // This is a simplified version - full implementation would use pattern matching
        if let Ok(history) = self.history.try_read() {
            if let Some(success_rate) = history.get_success_rate(tool_name, &ctx.input) {
                if success_rate > 0.8 {
                    let boost = 0.1 * success_rate;
                    *confidence += boost;
                    return Some(CalibrationFactor {
                        name: "history_boost".to_string(),
                        adjustment: boost,
                        reason: format!("Historical success rate: {:.0}%", success_rate * 100.0),
                    });
                }
            }
        }

        None
    }
}
```

### 6. Clarification Integrator

**Responsibility**: Manage clarification flow without losing context

```rust
pub struct ClarificationIntegrator {
    /// Pending clarifications
    pending: Arc<RwLock<HashMap<String, PendingClarification>>>,
    /// Timeout for clarifications
    timeout: Duration,
}

pub struct PendingClarification {
    /// Session ID
    pub session_id: String,
    /// Original routing context
    pub original_context: RoutingContext,
    /// Aggregated intent that needs clarification
    pub intent: AggregatedIntent,
    /// Specific parameter being clarified
    pub clarifying_param: ParameterRequirement,
    /// When created
    pub created_at: Instant,
}

impl ClarificationIntegrator {
    /// Start a clarification flow
    pub async fn start_clarification(
        &self,
        intent: AggregatedIntent,
        context: RoutingContext,
    ) -> ClarificationRequest {
        let session_id = uuid::Uuid::new_v4().to_string();
        let param = intent.missing_parameters[0].clone();

        let pending = PendingClarification {
            session_id: session_id.clone(),
            original_context: context,
            intent: intent.clone(),
            clarifying_param: param.clone(),
            created_at: Instant::now(),
        };

        self.pending.write().await.insert(session_id.clone(), pending);

        ClarificationRequest {
            session_id,
            prompt: param.clarification_prompt,
            suggestions: param.suggestions,
            input_type: if param.suggestions.is_empty() {
                ClarificationInputType::Text
            } else {
                ClarificationInputType::Select
            },
        }
    }

    /// Resume with user input
    pub async fn resume(
        &self,
        session_id: &str,
        user_input: &str,
    ) -> Result<ResumeResult, ClarificationError> {
        let pending = self.pending.write().await.remove(session_id)
            .ok_or(ClarificationError::SessionNotFound)?;

        // Check timeout
        if pending.created_at.elapsed() > self.timeout {
            return Err(ClarificationError::Timeout);
        }

        // Augment the original context with the clarified parameter
        let mut augmented_context = pending.original_context.clone();
        let mut params = pending.intent.primary.parameters.clone();
        params[&pending.clarifying_param.name] = serde_json::Value::String(user_input.to_string());

        // Create updated intent
        let mut updated_intent = pending.intent.clone();
        updated_intent.primary.parameters = params;
        updated_intent.missing_parameters.remove(0);
        updated_intent.parameters_complete = updated_intent.missing_parameters.is_empty();

        // Update action if parameters now complete
        if updated_intent.parameters_complete {
            updated_intent.action = if updated_intent.final_confidence >= 0.9 {
                IntentAction::Execute
            } else {
                IntentAction::RequestConfirmation
            };
        }

        Ok(ResumeResult {
            context: augmented_context,
            intent: updated_intent,
        })
    }

    /// Cleanup expired clarifications
    pub async fn cleanup_expired(&self) -> usize {
        let mut pending = self.pending.write().await;
        let now = Instant::now();
        let before_count = pending.len();

        pending.retain(|_, v| now.duration_since(v.created_at) < self.timeout);

        before_count - pending.len()
    }
}
```

## Configuration

```toml
[routing.pipeline]
# Enable the new intent routing pipeline
enabled = true

# Execution mode: "sequential" | "parallel" | "l1_only"
execution_mode = "sequential"

# Cache configuration
[routing.pipeline.cache]
enabled = true
max_size = 10000
ttl_seconds = 3600
decay_half_life_seconds = 1800
cache_auto_execute_threshold = 0.95

# Layer configuration
[routing.pipeline.layers]
# L1: Regex matching
l1_enabled = true
l1_auto_accept_threshold = 0.95

# L2: Semantic matching
l2_enabled = true
l2_skip_l3_threshold = 0.85

# L3: AI inference
l3_enabled = true
l3_timeout_ms = 5000
l3_min_confidence = 0.3
l3_parallel_param_extraction = true

# Confidence thresholds
[routing.pipeline.confidence]
no_match = 0.3
requires_confirmation = 0.7
auto_execute = 0.9

# Tool-specific overrides
[routing.pipeline.tools.search]
min_threshold = 0.5
auto_execute_threshold = 0.85
enable_repeat_boost = true

[routing.pipeline.tools.video]
min_threshold = 0.6
auto_execute_threshold = 0.9
enable_repeat_boost = false

# Clarification configuration
[routing.pipeline.clarification]
timeout_seconds = 60
max_pending = 10
```

## Data Flow Examples

### Example 1: Cache Hit (Fast Path)

```
User: "/search 北京天气"

1. InputPreprocessor
   - normalized: "/search 北京天气"
   - command_prefix: Some("search")
   - content: "北京天气"

2. Cache Lookup
   - hash("search 北京天气") = 0x1234...
   - CACHE HIT: confidence=0.92, tool="search"
   - confidence > 0.95? No

3. Layer Execution (skipped due to cache)
   - Using cached intent

4. Intent Aggregation
   - primary: search, confidence=0.92
   - action: Execute (from cache)

5. Execution
   - Execute search("北京天气")
   - Record success

Result: Search executed in <50ms (cache hit)
```

### Example 2: L1 Early Exit

```
User: "/translate Hello world to Chinese"

1. InputPreprocessor
   - command_prefix: Some("translate")
   - content: "Hello world to Chinese"

2. Cache Lookup
   - MISS

3. Layer Execution
   - L1: pattern "^/translate" matches
   - L1 confidence: 1.0

4. Early Exit (L1 confidence >= 0.95)
   - Skip L2/L3

5. Intent Aggregation
   - primary: translate, confidence=1.0
   - action: Execute

6. Execution
   - Execute translate("Hello world to Chinese")
   - Add to cache

Result: Translated in <100ms
```

### Example 3: Full Pipeline with Clarification

```
User: "天气怎么样"

1. InputPreprocessor
   - no command prefix
   - content: "天气怎么样"

2. Cache Lookup
   - MISS

3. Layer Execution
   - L1: No pattern match
   - L2: keyword "天气" → tool=search, confidence=0.75
   - L3: AI inference → tool=search, confidence=0.8

4. Intent Aggregation
   - Signals: [L2: 0.75, L3: 0.8]
   - primary: search (L3), confidence=0.8
   - Missing params: location
   - action: RequestClarification

5. Clarification
   - Prompt: "请问您想查询哪个城市的天气？"
   - Suggestions: ["北京", "上海", "深圳"]
   - User selects: "北京"

6. Resume
   - Augmented: search(query="北京天气")
   - action: Execute

7. Execution
   - Execute search("北京天气")
   - Add to cache with full params

Result: Search executed after clarification
```

## Migration Path

The new pipeline will be implemented alongside existing code and can be enabled via config flag:

```rust
// In AlephCore
pub async fn process_input(&self, input: &str) -> Result<String> {
    if self.config.routing.pipeline.enabled {
        // New pipeline
        self.pipeline.process(input, self.build_context()).await
    } else {
        // Existing dispatcher flow
        self.dispatcher.route_unified(input, &tools, &handler, conv).await
    }
}
```

This allows gradual rollout and A/B testing.

## Metrics and Observability

The pipeline will emit the following metrics:

- `intent_cache_hit_rate` - Cache hit percentage
- `intent_cache_size` - Current cache entries
- `layer_latency_ms` - Per-layer latency histogram
- `l3_call_rate` - Percentage of requests reaching L3
- `clarification_rate` - Percentage requiring clarification
- `confidence_distribution` - Histogram of final confidence scores
- `tool_execution_success_rate` - Per-tool success rate

These metrics will inform confidence calibration tuning.
