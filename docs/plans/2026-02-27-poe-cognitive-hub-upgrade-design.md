# POE 认知中枢升级设计

> **Status**: Approved
> **Date**: 2026-02-27
> **Scope**: Phase 1 — Core (AgentLoop fusion + Cortex absorption + PromptPipeline injection)

---

## 1. Background

POE (Principle-Operation-Evaluation) was designed as Aleph's goal-oriented execution framework. All functional development was completed between 2026-02-01 and 2026-02-03 (17 commits). In the subsequent 24 days, the rest of Aleph underwent explosive growth:

| Subsystem | Commits Since POE | Key Evolutions |
|-----------|-------------------|----------------|
| Memory | 198 | LanceDB migration, Event Sourcing, Embedding Provider abstraction |
| Gateway | 157 | New RPC methods, Session management, EventBus |
| Dispatcher | 64 | ToolIndex semantic inference, ExperienceReplayLayer |
| Thinker | 49 | PromptPipeline (19 layers), Soul/Embodiment |
| Agent Loop | 43 | RunContext, Session Protocol, ThinkingParser |
| Cortex | 35 | New subsystem — experience distillation, Meta-Cognition |
| **POE** | **20** | Only refactoring and file splits |

Six disconnection points were identified:

1. **POE <-> AgentLoop**: AgentLoop executes without knowledge of SuccessManifest
2. **POE <-> Cortex**: Overlapping experience crystallization/distillation with no communication
3. **POE <-> Thinker PromptPipeline**: Success contracts not injected into 19-layer pipeline
4. **POE <-> Memory Event Sourcing**: POE neither produces nor consumes Memory events
5. **POE <-> Dispatcher ToolIndex**: POE validation feedback doesn't influence tool selection
6. **POE <-> Session Protocol**: New session lifecycle concepts disconnected from POE

## 2. Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Core positioning | **Cognitive Hub** | POE becomes the upper-level orchestrator of AgentLoop |
| POE vs Cortex | **POE absorbs Cortex** | Eliminate dual experience management |
| Integration depth | **Deep fusion** | Every AgentLoop step reports to POE; POE can intervene at any step |
| Implementation scope | **Core first** | Phase 1: AgentLoop fusion + Cortex absorption + PromptPipeline injection |

## 3. Architecture

### 3.1 Positioning Change

**Before (current)**:
```
Gateway -> AgentLoop -> LoopResult
              ^ (occasionally)
             POE -> AgentLoopWorker -> AgentLoop (separate instance)
```

**After (upgraded)**:
```
Gateway -> POE Manager (cognitive hub)
              |
         Principle: Generate/load SuccessManifest
              |
         Operation: AgentLoop.run(
              RunContext { manifest, anchors, ... },
              PoeCallback  <- implements extended LoopCallback
         )
              | (per-step callbacks)
         PoeCallback:
           on_thinking_done -> assess decision direction
           on_action_done   -> lightweight intermediate validation
           on_step_evaluate -> return StepDirective (continue/hint/switch/abort)
              |
         Evaluation: Final validation + experience crystallization
              |
         PoeOutcome
```

### 3.2 Design Principles

1. **POE is an optional enhancement**: Without POE, AgentLoop behavior is 100% unchanged
2. **Interceptor, not Controller**: POE observes and advises; AgentLoop remains the executor
3. **Lightweight intermediate validation**: Each step only runs deterministic checks (no LLM), <10ms
4. **Cortex becomes a POE submodule**: Meta-Cognition capabilities fold into POE

### 3.3 Implementation Approach: Interceptor Pattern

POE integrates with AgentLoop through the existing `LoopCallback` trait, extended with one new method. This requires minimal changes to AgentLoop while enabling deep per-step observation and intervention.

## 4. Module Restructuring

```
core/src/poe/                          # POE Cognitive Hub
|-- mod.rs                             # Public API
|-- types.rs                           # Core types (unchanged)
|-- manager.rs                         # PoeManager (upgraded to cognitive hub orchestrator)
|-- manifest.rs                        # ManifestBuilder (unchanged)
|-- budget.rs                          # PoeBudget (unchanged)
|-- contract.rs                        # Contract types (unchanged)
|-- contract_store.rs                  # Contract storage (unchanged)
|-- trust.rs                           # Trust evaluation (unchanged)
|
|-- interceptor/                       # NEW: Interceptor layer
|   |-- mod.rs
|   |-- callback.rs                    # PoeLoopCallback (implements extended LoopCallback)
|   |-- step_evaluator.rs             # Lightweight intermediate evaluator
|   |-- directive.rs                   # StepDirective type definition
|   +-- manifest_injector.rs          # Injects manifest into PromptPipeline
|
|-- validation/                        # Validation engine (unchanged)
|   |-- hard.rs
|   |-- semantic.rs
|   +-- composite.rs
|
|-- worker/                            # Worker abstraction (simplified)
|   |-- mod.rs                         # Worker trait (retained but de-emphasized)
|   +-- tests/
|
|-- meta_cognition/                    # NEW: Migrated from Cortex
|   |-- types.rs                       # BehavioralAnchor, AnchorScope
|   |-- reactive.rs                    # ReactiveReflector (failure learning)
|   |-- critic.rs                      # CriticAgent (excellence learning)
|   |-- anchor_store.rs               # AnchorStore (SQLite persistence)
|   |-- anchor_retriever.rs           # Tag-based anchor retrieval
|   |-- tag_extractor.rs              # Intent -> tag mapping
|   +-- conflict_detector.rs          # Anchor conflict detection
|
|-- crystallization/                   # UPGRADED: Absorbs Cortex distillation
|   |-- mod.rs                         # ExperienceRecorder trait
|   |-- distillation.rs               # Migrated from Cortex DistillationService
|   |-- pattern_extractor.rs          # Migrated from Cortex PatternExtractor
|   |-- experience_store.rs           # Experience storage
|   +-- dreaming.rs                   # Batch distillation (idle time)
|
|-- prompt_layer.rs                    # NEW: PoePromptLayer (PromptPipeline integration)
|
|-- handler_types/                     # Gateway RPC types (unchanged)
+-- services/                          # Gateway service layer (unchanged)
```

## 5. Interceptor Layer Design

### 5.1 LoopCallback Extension

Add one new method to the existing `LoopCallback` trait:

```rust
/// Called after each step completes (action + result available).
/// Returns a StepDirective telling AgentLoop what to do next.
/// Default implementation returns Continue (backward compatible).
async fn on_step_evaluate(
    &self,
    step: &LoopStep,
    state: &LoopState,
) -> StepDirective {
    StepDirective::Continue
}
```

### 5.2 StepDirective

```rust
pub enum StepDirective {
    /// Normal continuation
    Continue,

    /// Continue but inject a hint into the next Think step
    ContinueWithHint {
        hint: String,
    },

    /// Suggest strategy switch (warning, not forced termination)
    SuggestStrategySwitch {
        reason: String,
        suggestion: String,
    },

    /// Force loop termination
    Abort {
        reason: String,
    },
}
```

### 5.3 AgentLoop Integration Point

After `on_action_done` callback, before next iteration:

```rust
callback.on_action_done(&action, &result).await;

let directive = callback.on_step_evaluate(&current_step, &state).await;
match directive {
    StepDirective::Continue => { /* normal */ }
    StepDirective::ContinueWithHint { hint } => {
        state.set_next_hint(hint);
    }
    StepDirective::SuggestStrategySwitch { reason, suggestion } => {
        callback.on_guard_triggered(&GuardViolation::PoeStrategySwitch {
            reason, suggestion
        }).await;
    }
    StepDirective::Abort { reason } => {
        callback.on_aborted().await;
        return LoopResult::PoeAborted { reason };
    }
}
```

### 5.4 PoeLoopCallback

Wraps the original callback (e.g., EventEmittingCallback) and adds POE behavior:

```rust
pub struct PoeLoopCallback {
    manifest: Arc<SuccessManifest>,
    budget: Arc<RwLock<PoeBudget>>,
    step_evaluator: Arc<StepEvaluator>,
    anchor_retriever: Arc<AnchorRetriever>,
    experience_recorder: Option<Arc<dyn ExperienceRecorder>>,
    inner: Arc<dyn LoopCallback>,
}
```

- All observation callbacks: delegate to inner + record to budget
- `on_step_evaluate`: delegates to StepEvaluator
- `on_confirmation_required`: can reject tools based on manifest restrictions

### 5.5 StepEvaluator

Lightweight, deterministic evaluator — no LLM calls, <10ms per step:

```rust
pub struct StepEvaluator {
    hard_validator: Arc<HardValidator>,
}

impl StepEvaluator {
    pub async fn evaluate(
        &self,
        step: &LoopStep,
        state: &LoopState,
        manifest: &SuccessManifest,
        budget: &PoeBudget,
    ) -> StepDirective {
        // 1. Budget check (fast)
        // 2. Quick hard constraint check (only verifiable ones)
        // 3. Progress estimation
        // 4. Stuck detection (entropy-based)
        // 5. Drift detection -> hint generation
    }
}
```

## 6. PromptPipeline Integration

### 6.1 PoePromptLayer

```rust
pub struct PoePromptLayer;

impl PromptLayer for PoePromptLayer {
    fn name(&self) -> &'static str { "poe_success_criteria" }
    fn priority(&self) -> u32 { 505 }  // After ToolsLayer(500)/HydratedToolsLayer(501)
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Hydration,
          AssemblyPath::Soul, AssemblyPath::Context,
          AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        // 1. Inject SuccessManifest (objective, hard constraints, soft metrics)
        // 2. Inject BehavioralAnchors (top 5 by priority, with confidence %)
        // 3. Inject current step hint (if any)
    }
}
```

### 6.2 LayerInput Extension

```rust
pub struct LayerInput<'a> {
    // ... existing fields ...
    pub poe: Option<&'a PoePromptContext>,
}

pub struct PoePromptContext {
    pub manifest: Option<SuccessManifest>,
    pub anchors: Vec<BehavioralAnchor>,
    pub current_hint: Option<String>,
    pub progress_summary: Option<String>,
}
```

### 6.3 Hint Propagation

```
StepEvaluator -> ContinueWithHint { hint }
    -> LoopState.set_next_hint(hint)
    -> Next Think: LayerInput { poe: PoePromptContext { current_hint: hint } }
    -> PoePromptLayer.inject() -> hint appears in prompt
    -> LLM receives prompt with guidance
```

## 7. Cortex Absorption

### 7.1 Migration Map

| Cortex Component | Target | Notes |
|------------------|--------|-------|
| `meta_cognition/reactive.rs` | `poe/meta_cognition/reactive.rs` | Direct migration |
| `meta_cognition/critic.rs` | `poe/meta_cognition/critic.rs` | Direct migration |
| `meta_cognition/types.rs` | `poe/meta_cognition/types.rs` | BehavioralAnchor types |
| `meta_cognition/anchor_store.rs` | `poe/meta_cognition/anchor_store.rs` | SQLite persistence |
| `meta_cognition/injection.rs` | **Delete** | Replaced by PoePromptLayer |
| `distillation.rs` | `poe/crystallization/distillation.rs` | Merge with crystallization |
| `pattern_extractor.rs` | `poe/crystallization/pattern_extractor.rs` | Pattern extraction |
| `clustering.rs` | `poe/crystallization/clustering.rs` | Dedup clustering |
| `dreaming.rs` | `poe/crystallization/dreaming.rs` | Batch distillation |
| `types.rs` (Experience) | `poe/crystallization/experience.rs` | Experience types |
| `integration.rs` | **Delete** | Replaced by PoeManager orchestration |

### 7.2 Overlap Resolution

| POE Original | Cortex Original | Merged Solution |
|-------------|----------------|-----------------|
| `ChannelCrystallizer` | `DistillationService` | Keep `ChannelCrystallizer` mpsc architecture, absorb priority queuing and batch processing |
| `ExperienceRecorder` trait | `CortexIntegration` | Keep `ExperienceRecorder` trait, enrich implementation from Cortex logic |
| `EvolutionTracker` | `PatternExtractor` + `ClusteringService` | Merge into unified `ExperienceEngine`: extract -> cluster -> store |

### 7.3 Unified ExperienceEngine

```rust
pub struct ExperienceEngine {
    pattern_extractor: PatternExtractor,
    clustering: ClusteringService,
    recorder_tx: mpsc::UnboundedSender<ExperienceRecord>,
    experience_store: Arc<ExperienceStore>,
    config: CrystallizationConfig,
}
```

- Real-time recording via mpsc channel (from POE's ChannelCrystallizer)
- Batch distillation during idle time (from Cortex's DreamingService)
- Implements `ExperienceRecorder` trait

### 7.4 Meta-Cognition Connection

- **ReactiveReflector**: Triggered when POE final evaluation fails, generates corrective anchors
- **CriticAgent**: Triggered during distillation when mediocre successes are detected, generates optimization anchors
- Both feed into `AnchorStore`, which is queried by `AnchorRetriever` for PromptPipeline injection

### 7.5 Cortex Deprecation

After migration, `core/src/memory/cortex/mod.rs` becomes a re-export bridge with `#[deprecated]` attributes. Full removal in the next version cycle.

## 8. Complete Data Flow

```
User Request (via Gateway)
    |
    v
PoeManager (Cognitive Hub)

  (1) PRINCIPLE
      |-- ManifestBuilder.build(instruction, context)
      |   +-- ExperienceEngine.search_similar() -> experience injection
      |-- TrustEvaluator.evaluate(manifest)
      |   +-- AutoApprove? -> skip signing / RequireSignature
      +-- AnchorRetriever.retrieve(intent) -> load behavioral rules

  (2) OPERATION
      |-- Build RunContext:
      |   |-- manifest -> PoePromptContext
      |   |-- anchors -> PoePromptContext
      |   +-- abort_signal -> control channel
      |-- Build PoeLoopCallback:
      |   |-- wraps EventEmittingCallback (preserves Gateway streaming)
      |   |-- StepEvaluator (lightweight intermediate validation)
      |   +-- PoeBudget (budget tracking)
      +-- AgentLoop.run(run_context, poe_callback)
          |
          |  Per step:
          |  Think: PoePromptLayer injects manifest + anchors + hint
          |  Act:   Execute tool
          |  Evaluate: StepEvaluator quick check
          |    -> Continue / Hint / Switch / Abort
          |
          v
      LoopResult

  (3) EVALUATION
      |-- CompositeValidator.validate(manifest, output)
      |   |-- Phase 1: HardValidator (deterministic)
      |   +-- Phase 2: SemanticValidator (LLM quality scoring)
      |-- Verdict -> PoeOutcome
      |-- If failed + budget remaining:
      |   |-- ReactiveReflector.handle_failure() -> new anchor
      |   +-- Re-enter (2) OPERATION with failure feedback
      +-- If success or budget exhausted:
          |-- ExperienceEngine.record() -> crystallization
          +-- CriticAgent.analyze() -> optimization suggestions (async)

  (4) OUTCOME
      +-- PoeOutcome -> Gateway -> Client
```

## 9. Lifecycle Comparison

| Phase | Before (Current) | After (Upgraded) |
|-------|-----------------|------------------|
| Request arrival | Gateway -> AgentLoop | Gateway -> PoeManager -> AgentLoop |
| Manifest generation | User manually triggers poe.prepare | PoeManager auto-generates per request |
| Tool selection | Thinker decides independently | Thinker + PoePromptLayer constraints |
| Per-step execution | AgentLoop autonomous | AgentLoop + PoeLoopCallback monitoring |
| Mid-course correction | None | StepEvaluator -> ContinueWithHint |
| Stuck detection | AgentLoop guards (doom loop) | POE budget entropy + AgentLoop guards (dual) |
| Final validation | None (or separate POE) | CompositeValidator mandatory |
| Experience learning | None | ExperienceEngine + ReactiveReflector |
| Prompt injection | None | PoePromptLayer (priority 505) |

## 10. Backward Compatibility

POE is an optional enhancement layer:

```rust
// Without POE (100% unchanged behavior)
let agent_loop = AgentLoop::new(thinker, executor, compressor, config);
let result = agent_loop.run(run_context, &default_callback).await;
// LoopCallback::on_step_evaluate defaults to Continue
// LayerInput::poe defaults to None

// With POE (full cognitive hub lifecycle)
let poe_manager = PoeManager::new(config, provider, ...);
let outcome = poe_manager.execute(instruction, context).await;
```

## 11. Testing Strategy

```
Test Pyramid:

         +-- E2E (BDD) --+       2-3 scenarios
         | Full P->O->E   |      gateway -> poe -> agent_loop -> tool
         +-------+--------+
             +---+-----------+
             | Integration   |    8-10 tests
             | PoeManager +  |    mock worker, real validator
             | AgentLoop     |
             +------+--------+
        +-----------+------------+
        | Unit Tests             |  30+ tests
        | StepEvaluator          |
        | PoeLoopCallback        |
        | PoePromptLayer         |
        | ExperienceEngine       |
        | ReactiveReflector      |
        +------------------------+
```

Key test scenarios:

| Scenario | Verification |
|----------|-------------|
| StepDirective::Continue | Default behavior, no intervention |
| StepDirective::ContinueWithHint | Hint appears in next step's prompt |
| StepDirective::Abort | AgentLoop terminates correctly |
| Budget exhausted | POE issues Abort directive |
| Entropy stuck | POE issues StrategySwitch |
| Manifest constraint met | Progress correctly tracked |
| Without POE | AgentLoop behavior 100% unchanged |
| ReactiveReflector trigger | Failure generates new anchor |
| Anchor prompt injection | PoePromptLayer outputs correctly |

## 12. Phase 1 Implementation Scope

| Priority | Work Item | Changed Files |
|----------|----------|---------------|
| P0 | Extend LoopCallback + StepDirective | `agent_loop/callback.rs`, `agent_loop/agent_loop.rs` |
| P0 | PoeLoopCallback implementation | `poe/interceptor/callback.rs` (new) |
| P0 | StepEvaluator implementation | `poe/interceptor/step_evaluator.rs` (new) |
| P1 | PoePromptLayer implementation | `poe/prompt_layer.rs` (new) |
| P1 | LayerInput + PromptConfig extension | `thinker/prompt_layer.rs`, `thinker/prompt_builder/mod.rs` |
| P1 | RunContext extension (manifest) | `agent_loop/agent_loop.rs` |
| P2 | Cortex -> POE migration | File moves + re-exports |
| P2 | ExperienceEngine merge | `poe/crystallization/` (refactor) |
| P2 | PoeManager upgrade to cognitive hub | `poe/manager.rs` (refactor) |

## 13. Future Phases (Not in Scope)

- **Phase 2**: Memory Event Sourcing integration (POE produces/consumes MemoryEvents)
- **Phase 2**: Dispatcher ToolIndex feedback (validation results influence tool ranking)
- **Phase 3**: Session Protocol alignment (POE lifecycle maps to session lifecycle)
- **Phase 3**: Advanced trust evaluation (V1.5 whitelist, V2.0 experience-based auto-approval)
