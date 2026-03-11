# Cognitive Evolution Beta: Immune-Complete Loop Design

**Date**: 2026-03-11
**Route**: Beta (Immune-Complete Loop)
**Scope**: Close the open-loop evolution pipeline with validation, shadow deployment, and reflective dreaming

---

## 1. Design Decisions Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Priority | D: A+B hybrid | Open-loop + self-modifying code = uncontrollable degradation |
| Validation depth | C: Tiered (L1→L2→L3) | Risk profile drives validation cost |
| Sample extraction | B+C: Cluster + boundary | Coverage + stress resilience |
| Provider coupling | B: `PatternSynthesisBackend` trait | Dependency inversion + testability |
| Pattern model | Enhanced Sequence + Macros | Markdown affinity + trace-to-logic folding |
| Deployment | C: Shadow deployment (`evolved_skills/`) | Closed-loop verification + auto-promote/demote |

---

## 2. PatternSynthesisBackend Trait

### Goal
Bridge PatternExtractor's existing prompt logic to actual LLM calls via dependency-inverted trait.

### Files
- **New**: `core/src/poe/crystallization/synthesis_backend.rs`
- **Modify**: `core/src/poe/crystallization/pattern_extractor.rs` (inject backend)
- **New**: `core/src/poe/crystallization/provider_backend.rs` (ProviderManager impl)

### Core Interface

```rust
pub struct PatternSynthesisRequest {
    pub objective: String,
    pub tool_sequences: Vec<ToolSequenceTrace>,
    pub env_context: Option<String>,
    pub existing_patterns: Vec<String>,
}

pub struct PatternSuggestion {
    pub description: String,
    pub steps: Vec<PatternStep>,
    pub parameter_mapping: ParameterMapping,
    pub pattern_hash: String,
    pub confidence: f32,
}

#[async_trait]
pub trait PatternSynthesisBackend: Send + Sync {
    async fn synthesize_pattern(&self, request: PatternSynthesisRequest) -> Result<PatternSuggestion>;
    async fn evaluate_confidence(&self, pattern: &ExtractedPattern, occurrences: &[PoeExperience]) -> Result<f32>;
}
```

### PatternExtractor Refactoring
- Holds `Arc<dyn PatternSynthesisBackend>` instead of stub
- Existing prompt-building logic (`build_request()`) preserved
- `MockSynthesisBackend` for unit/integration tests

### Dependency Direction
`PatternExtractor` → `PatternSynthesisBackend` (trait) ← `ProviderBackend` (impl) — P4 compliant.

---

## 3. Enhanced Sequence Pattern Model

### Goal
Upgrade static `(intent, tool_sequence, parameter_mapping)` to support conditional branching and bounded loops.

### Files
- **New**: `core/src/poe/crystallization/pattern_model.rs`
- **Modify**: `core/src/poe/crystallization/experience.rs`
- **Modify**: `core/src/poe/crystallization/pattern_extractor.rs`

### Core Types

```rust
pub enum Predicate {
    Semantic(String),                          // LLM-evaluated at runtime
    MetricThreshold { metric: CognitiveMetric, op: CompareOp, threshold: f32 },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
}

pub enum CognitiveMetric { Entropy, TrustScore, RemainingBudgetRatio, AttemptCount }

pub enum PatternStep {
    Action { tool_call: ToolCallTemplate, params: ParameterMapping },
    Conditional { predicate: Predicate, then_steps: Vec<PatternStep>, else_steps: Vec<PatternStep> },
    Loop { predicate: Predicate, body: Vec<PatternStep>, max_iterations: u32 },
    SubPattern { pattern_id: String },
}

pub struct PatternSequence {
    pub description: String,
    pub steps: Vec<PatternStep>,
    pub expected_outputs: Vec<String>,
}
```

### Trace-to-Logic Folding Rules

| Trace Pattern | Folded Result |
|---------------|---------------|
| 3+ similar consecutive ToolCalls | `PatternStep::Loop` |
| Two success traces diverge at step N | `PatternStep::Conditional` (LLM extracts predicate) |
| Single linear execution | `PatternStep::Action` sequence |
| Contains solidified sub-pattern | `PatternStep::SubPattern` reference |

### Constraints
- `max_iterations`: > 0, ≤ 10
- `SubPattern` nesting depth: ≤ 3
- `Predicate::Semantic` string: ≤ 200 chars

### SKILL.md Mapping
```markdown
## Steps
1. Action: search_codebase(query: "{input_query}")
2. IF entropy > 0.5:
   2a. Action: search_more(scope: "expanded")
3. ELSE:
   3a. Action: apply_fix(target: "{detected_file}")
4. LOOP WHILE result_incomplete (max 3):
   4a. Action: refine_output()
```

---

## 4. Tiered Validation Gate

### Goal
Risk-aware validation between SkillGenerator and deployment; ensures "no regression".

### Files
- **New**: `core/src/skill_evolution/validation/mod.rs`
- **New**: `core/src/skill_evolution/validation/risk_profiler.rs`
- **New**: `core/src/skill_evolution/validation/structural_linter.rs` (L1)
- **New**: `core/src/skill_evolution/validation/semantic_replayer.rs` (L2)
- **New**: `core/src/skill_evolution/validation/sandbox_executor.rs` (L3, stub)
- **New**: `core/src/skill_evolution/validation/test_set_generator.rs`
- **Modify**: `core/src/skill_evolution/pipeline.rs`

### Skill Risk Profile

```rust
pub enum SkillRiskLevel {
    Low,    // read-only, info processing, format conversion
    Medium, // local file writes, cross-plugin, complex branching
    High,   // network, shell, delete/overwrite, credentials
}
```

Tool classification: `ReadOnly → Low`, `FileWrite | CrossPlugin → Medium`, `Shell | Network | Destructive → High`. Loops with `max_iterations > 5` escalate to Medium. SubPattern references escalate to Medium.

### Validation Levels

| Level | Name | Required For | Time Budget |
|-------|------|-------------|-------------|
| L1 | Structural Linter | All skills | < 100ms |
| L2 | Semantic Replay | Medium + High risk | < 5s |
| L3 | Sandbox Execution | High risk (Beta: stub) | < 60s |

### Test Set Generator (Cluster + Boundary Sampling)

1. Cluster successful experiences by cosine similarity (threshold: 0.95)
2. Take representative from each cluster (highest satisfaction)
3. Add boundary cases: max duration, max params, max steps (up to 2)
4. Total samples: `min(cluster_count + 2, 8)`

### L2 Semantic Replayer
- Simulates pattern execution with historical input context
- Compares output via embedding similarity (threshold: 0.8)
- Must pass ≥ 80% of test samples

### Differential Testing (Efficiency Gate)
- Compares new skill's estimated steps/tokens against historical baseline
- Step cost estimation: Action=1.0, Conditional=avg(branches)+0.5, Loop=body×(max/2)+0.5, SubPattern=3.0
- Rejects skills that are not more efficient (10% tolerance)
- Runs BEFORE tiered validation — prove "worth validating" first

### High Risk + No Sandbox
- Sets `requires_human_review = true`
- Routes to ApprovalManager

---

## 5. Shadow Deployment & Skill Lifecycle

### Goal
Validated skills enter a probation period; promoted or demoted based on real-world performance.

### Files
- **New**: `core/src/skill_evolution/shadow_deployer.rs`
- **New**: `core/src/skill_evolution/lifecycle.rs`
- **Modify**: `core/src/skill_evolution/pipeline.rs`
- **Modify**: `core/src/skill_evolution/tracker.rs`

### Lifecycle State Machine

```
Draft → Shadow → Promoted
                ↘ Demoted → Retired
```

```rust
pub enum SkillLifecycleState {
    Draft,
    Shadow { deployed_at, invocation_count, success_count, baseline_comparison },
    Promoted { promoted_at, shadow_duration_days },
    Demoted { reason, demoted_at, previous_state },
    Retired { reason, retired_at },
}
```

### Thresholds

| Parameter | Default | Purpose |
|-----------|---------|---------|
| `min_invocations` | 5 | Minimum usage before promotion |
| `min_success_rate` | 0.85 | Quality floor for promotion |
| `min_shadow_days` | 2 | Minimum probation period |
| `must_beat_baseline` | true | Must outperform raw LLM approach |
| `consecutive_failures` | 3 | Demotion trigger |
| `success_rate_floor` | 0.5 | Demotion trigger |
| `max_shadow_days` | 30 | Auto-retire if never promoted |

### Deployment Flow
1. Write `SKILL.md` + `metadata.json` to `evolved_skills/{skill_id}/`
2. Git commit to shadow directory
3. `record_invocation()` called after each real usage
4. Promotion: atomic `rename` from `evolved_skills/` to `skills/`
5. Demotion: archive metadata to EvolutionTracker, then delete

### Metadata Sidecar (Evolution Lineage)

```rust
pub struct SkillMetadata {
    pub skill_id: String,
    pub lifecycle: SkillLifecycleState,
    pub origin: SkillOrigin,          // pattern_id, source_experiences, generator_version
    pub risk_profile: SkillRiskLevel,
    pub validation_history: Vec<ValidationSummary>,
}
```

---

## 6. CortexDreaming Triggers & Clustering Storage

### Goal
Replace placeholder idle detection; make dreaming priority-driven by cognitive entropy.

### Files
- **New**: `core/src/poe/crystallization/idle_detector.rs`
- **New**: `core/src/poe/crystallization/cognitive_entropy.rs`
- **Modify**: `core/src/poe/crystallization/dreaming.rs`
- **Modify**: `core/src/poe/crystallization/clustering.rs`
- **Modify**: `core/src/poe/crystallization/experience_store.rs`

### Idle Detector
- Tracks `last_activity` via `AtomicU64` (unix timestamp)
- Subscribes to `PoeEventBus` for automatic activity tracking
- `is_idle()` = elapsed > `min_idle_seconds` (default: 300s)
- Activity sources: POE execution, gateway requests, user interaction

### Cognitive Entropy Tracker
- Per-pattern entropy = avg `distance_score` over last N executions
- Trend detection: first-half vs second-half mean comparison (±0.1 threshold)
- `Increasing` trend → `DistillationPriority::High`

| Entropy Trend | Priority | Action |
|---------------|----------|--------|
| Increasing | High | Urgent distillation |
| Volatile | Normal | Needs more data |
| Stable | Low | Converged, no action |
| Decreasing | Skip | System is learning |

### Dreaming Cycle (Revised)
1. Check `idle_detector.is_idle()` — prerequisite
2. Query `CognitiveEntropyTracker` for high-entropy patterns (up to 3, priority High)
3. Query `CortexValueEstimator` for batch candidates (priority Normal)
4. Submit merged tasks to `DistillationService` with rate limiting
5. Existing `max_distillations_per_minute = 10` preserved

### ExperienceStore API Extensions

```rust
// New methods for clustering merge
async fn delete(&self, experience_id: &str) -> Result<bool>;
async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<PoeExperience>>;
```

### Clustering Merge
- Keep cluster representative (highest satisfaction)
- Delete redundant members via new `delete()` API
- Archive threshold remains at similarity 0.95

---

## 7. Complete Pipeline Flow

```
POE Execution
    │
    ▼
ExperienceRecorder (ChannelCrystallizer)
    │
    ▼
EvolutionTracker (SQLite) ◄──── ShadowDeployer.record_invocation()
    │
    ├─────────────────────────────────────────┐
    │                                         │
    ▼                                         ▼
SolidificationDetector          CortexDreamingService
(threshold check)               (idle + entropy-driven)
    │                                         │
    └──────────────┬──────────────────────────┘
                   ▼
         DistillationService (priority queue)
                   │
                   ▼
         PatternExtractor + PatternSynthesisBackend
         (Enhanced Sequence output)
                   │
                   ▼
         SkillGenerator (SKILL.md)
                   │
                   ▼
         DifferentialEngine ──► reject if less efficient
                   │
                   ▼
         TieredValidator
         ├─ L1 Structural ──► reject if schema mismatch
         ├─ L2 Semantic ──► reject if output diverges
         └─ L3 Sandbox (future)
                   │ pass
                   ▼
         ShadowDeployer → evolved_skills/
                   │
                   ▼
         GitCommitter (shadow only)
                   │
                   │ real usage over time
                   ▼
         Lifecycle Manager
         ├─ Promote → skills/
         ├─ Demote → archive + delete
         └─ Retire → cleanup (30 days)
                   │
                   └──► back to EvolutionTracker (closed loop)
```

---

## 8. Key Constraints & Invariants

1. **No skill deploys without validation** — TieredValidator is mandatory, never bypassed
2. **Efficiency gate before validation** — prove "worth validating" before spending resources
3. **Shadow before official** — no direct-to-main skill deployment
4. **Dreaming only when idle** — never competes with POE execution
5. **Bounded loops only** — `max_iterations ≤ 10`, no infinite execution
6. **Metadata always accompanies skill** — `metadata.json` required for deployment
7. **Demotion archives, not discards** — failed skills become learning material
8. **High risk without sandbox → human review** — safety valve in Beta phase

---

## 9. Future (Gamma) Extensions

These are explicitly deferred and depend on Beta stability:

- **KnowledgeConsolidator**: Semantic deduplication + skill merging (similarity > 85%)
- **L3 Sandbox Executor**: Full isolated execution environment
- **Vitality Score**: success_rate × invocation_frequency / maintenance_cost
- **Auto-de-solidification**: Continuous failure → revert to Experience state
- **Meta-Evolution (StrategyOptimizer)**: Optimize detector thresholds and generator prompts
- **Cross-Project Synapse**: Federated pattern fingerprint exchange
- **Active Hypothesis Testing**: Proactive exploration when confidence is low
