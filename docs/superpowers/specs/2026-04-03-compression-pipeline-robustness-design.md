# Compression Pipeline Robustness Design

> Phase B of memory system optimization: trigger mechanism optimization, cascade degradation, post-compression critical information recovery.

## Background

Aleph's context compression pipeline has a solid foundation — `SessionCompactor` (d0/d1/d2 hierarchical summarization), `ContextBudget` (pressure sensing + circuit breaker), `CompressionService` (signal-driven fact extraction), and `DreamDaemon` (background consolidation). However, comparative analysis with Claude Code reveals key gaps in robustness:

1. **No lightweight pre-compression** — jumps straight to LLM summarization
2. **Abrupt degradation** — from "everything fine" (70%) to "force stop" (85%) with nothing in between
3. **No post-compression recovery** — critical context lost after compaction
4. **Rigid DreamDaemon trigger** — fixed interval + idle detection, no event-driven triggers
5. **tool_use/tool_result splitting** — chunking by token size can break tool call pairs
6. **Scattered post-compact cleanup** — only `scheduler.reset_turns()`, other modules not coordinated

## Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | Microcompact + multi-tier cascade | Microcompact is zero-LLM-cost; multi-tier prevents sudden jumps |
| D2 | Constraint injection + semantic recovery | Deterministic fallback + LanceDB-powered deep recovery |
| D3 | Hybrid gating for DreamDaemon | Event-driven + timer fallback with cheap-to-expensive gate chain |
| D4 | 3D tool output scoring (age x size x importance) | Smart prioritization with simple rules, not complex matrices |
| D5 | Pair-aware chunking with semantic units | Tool rounds as atomic units improve both safety and summary quality |
| D6 | Centralized PostCompactCleanup trait | Compile-time contracts, ordered execution, extensible |

## Architecture Overview

```
ContextBudget.sense_pressure()
        │
        ▼
CompactionOrchestrator
  ├─ evaluate pressure level (Calm/Preventive/Warning/High/Critical)
  ├─ select applicable strategies
  ├─ execute in priority order:
  │     1. MicroCompactor        (tool output pruning, zero LLM cost)
  │     2. SessionCompactorStrategy (d0/d1/d2 with ToolAwareChunker)
  │     3. LlmSummaryStrategy   (side-channel full summarization)
  │  after each: re-evaluate pressure, stop if target reached
  ├─ PostCompactCleanup chain:
  │     10: SignalDetectorCleanup
  │     20: CircuitBreakerCleanup
  │     30: SchedulerCleanup
  │     40: MetricsCleanup
  │     50: ConstraintInjector    (inject dynamic constraints)
  │     60: DreamGateEvaluator    (evaluate background consolidation)
  └─ return Directive
```

## Section 1: CompactionOrchestrator

### Responsibility

The orchestrator is the compression pipeline's decision-maker: sense pressure, evaluate available strategies, select execution combination, coordinate cleanup. It contains no compression logic itself.

### Pressure Levels & Strategy Mapping

| Level | Token % | Strategy Combination |
|-------|---------|---------------------|
| Calm | < 60% | No action |
| Preventive | 60-70% | Preventive microcompact (large + old tool outputs only) |
| Warning | 70-80% | Microcompact + async d0 summarization |
| High | 80-85% | Forced microcompact + sync LLM summary + constraint injection |
| Critical | 85%+ | Full compaction + constraint injection + FinalReply |

### Core Traits

```rust
/// Compression strategy trait — unified interface for all strategies
/// Uses manual async dispatch (Pin<Box<dyn Future>>) for object safety,
/// since dyn CompactionStrategy must be usable as trait object in Vec<Arc<dyn ...>>.
pub trait CompactionStrategy: Send + Sync {
    /// Strategy name for logging and metrics
    fn name(&self) -> &str;

    /// Estimate how many tokens this strategy can free (lightweight, no execution)
    fn estimate_savings(&self, ctx: &CompactionContext) -> TokenEstimate;

    /// Execute compression, return actual freed tokens
    fn execute<'a>(
        &'a self,
        ctx: &'a mut CompactionContext,
    ) -> Pin<Box<dyn Future<Output = Result<CompactionResult>> + Send + 'a>>;

    /// Whether this strategy is applicable to current context
    fn is_applicable(&self, ctx: &CompactionContext) -> bool;
}

/// Post-compaction cleanup trait
pub trait PostCompactCleanup: Send + Sync {
    fn cleanup_order(&self) -> u32;
    fn on_compact_complete(&self, result: &CompactionResult);
}

/// Orchestrator
pub struct CompactionOrchestrator {
    strategies: Vec<Arc<dyn CompactionStrategy>>,  // sorted by priority
    cleanups: Vec<Arc<dyn PostCompactCleanup>>,
    budget: Arc<ContextBudget>,
}
```

### Orchestration Flow

1. `ContextBudget.sense_pressure()` returns `PressureLevel`
2. Filter applicable strategies by pressure level
3. Call `estimate_savings()` on each for cost-benefit preview
4. Execute strategies in priority order, re-evaluate pressure after each
5. If pressure drops below target → stop, return success
6. If all strategies exhausted and still over → return `FinalReply`
7. Execute `PostCompactCleanup` chain in order

### Integration with Existing Code

- **Extend** `ContextBudget` — add `Calm` and `Preventive` pressure levels; retain existing `Warning` (70%) and `Critical` (85%)
- **Wrap** `SessionCompactor` — existing d0/d1/d2 compression registers as a `CompactionStrategy`
- **Wrap** `context_compactor` — existing LLM side-channel summary registers as another strategy
- **New** `MicroCompactor` — new strategy implementation
- **Replace** direct compaction calls in `agent_loop/context_compactor.rs` with orchestrator invocations

## Section 2: MicroCompactor

### Responsibility

Free context space by pruning tool outputs without any LLM calls. The lowest-cost compression available — always preferred before LLM summarization.

### 3D Compressibility Scoring

Each tool output gets a `compressibility` score (higher = prune first):

```rust
pub struct ToolOutputEntry {
    pub turn_age: u32,
    pub token_size: usize,
    pub importance: Importance,
    pub tool_name: String,
    pub message_index: usize,
}

pub enum Importance {
    Low,      // search results, file reads (already digested)
    Medium,   // general tool calls
    High,     // error outputs, user feedback, config changes
}

impl ToolOutputEntry {
    pub fn compressibility(&self) -> f64 {
        let age_score = (self.turn_age as f64 / 20.0).min(1.0);
        let size_score = (self.token_size as f64 / 3000.0).min(1.0);
        let importance_penalty = match self.importance {
            Importance::Low => 0.0,
            Importance::Medium => 0.3,
            Importance::High => 0.7,
        };
        (age_score * 0.4 + size_score * 0.4) * (1.0 - importance_penalty)
    }
}
```

### Importance Classification Rules

- **High**: tool_result contains error/panic/failure keywords; tool_name in {user_feedback, config, memory}
- **Medium**: default
- **Low**: tool_name in {read_file, search, glob, grep, ls} AND turn_age >= 5

### Compacted Output Format

Replace with structured placeholder (not deletion):

```
[Tool output compacted: {tool_name}]
- Size: {original_tokens} tokens -> compacted
- Key fields: {extracted_top_level_keys}  // JSON results only
- Status: {success|error}
```

### Execution Flow

1. Scan message list, collect all tool_results as `ToolOutputEntry`
2. Skip tool outputs within fresh_tail (last 6 messages)
3. Sort by compressibility descending
4. Prune from highest to lowest, re-estimate total tokens after each
5. Stop when pressure drops below target
6. Return `CompactionResult { freed_tokens, compacted_count }`

### Pair Safety

Microcompact only replaces `tool_result` content, never deletes the message itself. tool_use messages (parameters) are never pruned (typically small). This naturally preserves tool_use/tool_result pairing.

## Section 3: ToolAwareChunker

### Responsibility

Replace token-based chunking in `SessionCompactor.post_turn_compress()` with semantic-unit-aware chunking. Ensures tool_use/tool_result pairs are never split, and improves d0 summary quality.

### Semantic Unit Definition

```rust
pub enum SemanticUnit {
    /// User message (independent unit)
    UserMessage { index: usize },

    /// Assistant text reply (independent unit)
    AssistantText { index: usize },

    /// Tool round: assistant(tool_use) + user(tool_result) + assistant(follow-up)
    /// Atomic unit, never split
    ToolRound {
        tool_use_index: usize,
        tool_result_index: usize,
        follow_up_index: Option<usize>,
        tool_name: String,
    },
}

impl SemanticUnit {
    pub fn token_size(&self, messages: &[Message]) -> usize { ... }
    pub fn message_indices(&self) -> Vec<usize> { ... }
}
```

### Chunking Strategy

1. Parse message list into `Vec<SemanticUnit>`
2. Exclude units within fresh_tail
3. Greedy chunking: accumulate units until approaching `chunk_token_limit`
   - If next unit would exceed limit and current chunk non-empty → cut
   - If single ToolRound exceeds `chunk_token_limit` → microcompact its tool_result first, then add
4. Each chunk contains complete semantic units only

### Quality Improvement

```
// Before (possible chunk break mid-tool-call):
[user: "read config.rs", assistant: tool_use(read_file)]
// tool_result in next chunk — LLM doesn't see the result

// After (guaranteed complete):
[user: "read config.rs", assistant: tool_use(read_file),
 user: tool_result("pub struct Config {...}"),
 assistant: "Config struct contains..."]
```

### Integration

- **Replace** token-based chunking in `post_turn_compress()`
- **Preserve** d0/d1/d2 hierarchy and fanout triggers unchanged
- `ToolAwareChunker` is an internal component of `SessionCompactor`, no public API change

## Section 4: Constraint Injection + Semantic Recovery

### Layer 1: ConstraintInjector

Automatically injects critical dynamic constraints after each compaction. Zero LLM cost.

```rust
pub struct ConstraintInjector {
    static_constraints: Vec<Constraint>,
    dynamic_sources: Vec<Arc<dyn ConstraintSource>>,
}

pub trait ConstraintSource: Send + Sync {
    fn collect_constraints(&self) -> Vec<Constraint>;
}

pub struct Constraint {
    pub category: ConstraintCategory,
    pub content: String,
    pub priority: u8,
}

pub enum ConstraintCategory {
    ActiveTask,     // current task description + progress
    ActiveTools,    // currently available tool list
    UserPreference, // high-frequency preferences from memory
}
```

**Injection format** (appended after compression summary):

```
<post-compaction-context>
## Active Constraints (auto-restored after compaction)

### Task Context
- Current task: {task_description}
- Progress: {completed_steps} / {total_steps}

### Active Tools
{tool_list}

### Key Preferences
{top_3_user_preferences_from_memory}
</post-compaction-context>
```

Note: Red line constraints (R1-R10) are in the system prompt and not re-injected. Only **dynamic context** that compression might lose is injected.

### Layer 2: SemanticRecoveryTool

Exposes a built-in tool for the LLM to retrieve pre-compression conversation details on demand.

```rust
pub struct SemanticRecoveryTool {
    database: MemoryBackend,  // reuse LanceDB
}
```

**Tool definition** (exposed to LLM):

```json
{
    "name": "recall_context",
    "description": "Retrieve pre-compression conversation details. Use when you need to recall specific code, error messages, or decision details from earlier in the conversation.",
    "parameters": {
        "query": "string - description of what to recall",
        "max_results": "integer - max results, default 3"
    }
}
```

### Data Flow

```
Pre-compression:
  SessionCompactor.post_turn_compress()
    → ToolAwareChunker splits into chunks
    → Each chunk: generate d0 summary AND store raw chunk to LanceDB
      path: aleph://session/{id}/raw/{seq}
      scope: SessionLocal
      embedding: generated from chunk content

Post-compression:
  ConstraintInjector.inject() → inject dynamic constraints

Runtime:
  LLM calls recall_context("what was the config.rs error")
    → LanceDB vector search on aleph://session/{id}/raw/*
    → Return most relevant raw chunk fragments
```

### Cost Control

- Constraint injection: zero LLM cost, pure string concatenation
- Raw chunk storage: reuses existing LanceDB write pipeline; overhead is only embedding computation
- Semantic recovery: only costs tokens when LLM actively calls the tool; constraint injection suffices in most cases
- Raw chunk lifecycle: same as session summaries (`session_fact_retention_hours`), decays after session ends

## Section 5: DreamDaemon Hybrid Gating

### Responsibility

Upgrade DreamDaemon trigger from "fixed interval + idle detection" to "event-driven + timer fallback" with a cheap-to-expensive gate chain.

### Trigger Points

1. **Session end** event
2. **CompactionCompleted** event (via PostCompactCleanup trait)
3. **Timer fallback** — every 4 hours (replaces original 1 hour)

### Three-Level Gate Chain (Cheap → Expensive)

```
Gate 1: Time Gate [cost: 1 timestamp comparison]
  └─ hours since last consolidation < min_hours (default 6h)? → skip

Gate 2: Count Gate [cost: 1 database count query]
  └─ pending unconsolidated facts < min_facts (default 20)? → skip

Gate 3: Semantic Drift Gate [cost: vector similarity computation]
  └─ avg semantic distance of new facts from consolidated < drift_threshold (0.3)? → skip

All pass → execute DreamDaemon 6-stage pipeline
```

### Core Structures

```rust
pub struct DreamGate {
    config: DreamGateConfig,
    last_consolidation: AtomicI64,
}

pub struct DreamGateConfig {
    pub min_hours: f64,                 // default 6.0
    pub min_pending_facts: usize,       // default 20
    pub drift_threshold: f32,           // default 0.3 (cosine distance)
    pub background_interval: Duration,  // fallback interval, default 4h
}

pub enum GateResult {
    Pass,
    Blocked(BlockReason),
}

pub enum BlockReason {
    TooRecent { hours_since: f64 },
    InsufficientFacts { count: usize },
    LowDrift { avg_distance: f32 },
}
```

### Integration with Existing Code

- **Delete** `CompressionDaemon` (fixed-interval scheduler) → replaced by `DreamGate`
- **Preserve** DreamDaemon 6-stage pipeline (Collect→Cluster→Summarize→DriftDetect→Consolidate→Decay) unchanged
- **Replace** `CompressionDaemonConfig` with `DreamGateConfig`
- **New** `DreamGate` as front guard for DreamDaemon

```rust
impl PostCompactCleanup for DreamGate {
    fn cleanup_order(&self) -> u32 { 60 }
    fn on_compact_complete(&self, result: &CompactionResult) {
        self.evaluate_and_maybe_trigger();
    }
}
```

### Safety

- **Non-blocking** — gate evaluation on current thread (microseconds); actual consolidation spawned to background tokio task
- **Idempotent** — if consolidation already in progress (`is_running` flag), new triggers skip
- **Failure rollback** — failed consolidation does not update `last_consolidation`, next trigger retries

## Section 6: PostCompactCleanup Chain

### Responsibility

Centralized post-compression state cleanup. All modules reset to consistent state via ordered trait implementations.

### Cleanup Chain Members & Order

| Order | Cleanup | Action |
|-------|---------|--------|
| 10 | SignalDetectorCleanup | Reset signal history buffer, clear matched trigger phrases |
| 20 | CircuitBreakerCleanup | Reset counter if pressure reduced; increment if not |
| 30 | SchedulerCleanup | Reset pending_turns, update last_activity timestamp |
| 40 | MetricsCleanup | Record CompactionSession to database, update CompactorMetrics |
| 50 | ConstraintInjector | Collect and inject dynamic constraints |
| 60 | DreamGateEvaluator | Evaluate 3-level gate, maybe trigger background consolidation |

### Registration

```rust
let orchestrator = CompactionOrchestrator::builder()
    .strategy(Arc::new(MicroCompactor::new(config)))
    .strategy(Arc::new(SessionCompactorStrategy::new(compactor)))
    .strategy(Arc::new(LlmSummaryStrategy::new(provider)))
    .cleanup(Arc::new(SignalDetectorCleanup::new(signal_detector)))
    .cleanup(Arc::new(CircuitBreakerCleanup::new(circuit_breaker)))
    .cleanup(Arc::new(SchedulerCleanup::new(scheduler)))
    .cleanup(Arc::new(MetricsCleanup::new(database)))
    .cleanup(Arc::new(ConstraintInjector::new(sources)))
    .cleanup(Arc::new(DreamGateEvaluator::new(dream_gate)))
    .build();
```

## Code Cleanup Plan

### Files to Delete

- `src/memory/compression_daemon/daemon.rs` — replaced by DreamGate
- `src/memory/compression_daemon/config.rs` — replaced by DreamGateConfig
- `src/memory/compression_daemon/mod.rs` — module removed

### Files to Refactor

- `src/agent_loop/context_compactor.rs` — extract into `LlmSummaryStrategy`, remove direct compaction logic
- `src/agent_loop/context_budget/mod.rs` — extend `PressureLevel` enum with `Calm` and `Preventive`
- `src/memory/session_compactor/mod.rs` — integrate `ToolAwareChunker`, replace token-based chunking
- `src/memory/session_compactor/context_window.rs` — adapt partition logic for semantic units
- `src/memory/compression/scheduler.rs` — implement `PostCompactCleanup` trait
- `src/memory/compression/signal_detector.rs` — implement `PostCompactCleanup` trait
- `src/memory/compression/trigger.rs` — adapt to new pressure levels

### New Files

- `src/agent_loop/compaction_orchestrator.rs` — CompactionOrchestrator + CompactionStrategy trait
- `src/agent_loop/micro_compactor.rs` — MicroCompactor strategy
- `src/agent_loop/tool_aware_chunker.rs` — ToolAwareChunker + SemanticUnit
- `src/agent_loop/constraint_injector.rs` — ConstraintInjector + ConstraintSource trait
- `src/agent_loop/post_compact_cleanup.rs` — PostCompactCleanup trait + CompactionResult
- `src/memory/dreaming/gate.rs` — DreamGate + DreamGateConfig
- `src/builtin_tools/recall_context.rs` — SemanticRecoveryTool (recall_context)

## Differentiation from Claude Code

| Aspect | Claude Code | Aleph (This Design) |
|--------|-------------|---------------------|
| Microcompact | Time-based + cache_edits (vendor-specific) | 3D scoring (age x size x importance), vendor-agnostic |
| Chunking | API invariant preservation (defensive) | Semantic units (quality improvement + safety) |
| Recovery | File transcript + path reference | Constraint injection + LanceDB semantic retrieval |
| Dream trigger | 3-level gate (time + session count + lock) | 3-level gate (time + fact count + semantic drift) |
| Cleanup | Centralized function with hardcoded resets | Trait-based chain with compile-time contracts |
| Cascade | Hardcoded if-else paths | Strategy orchestrator with dynamic evaluation |
