# Cognitive Evolution Gamma: Vitality Engine + Sandbox Execution

**Date**: 2026-03-11
**Route**: Gamma (Vitality-Driven Evolution)
**Scope**: Vitality Score, Auto-de-solidification, KnowledgeConsolidator, L3 Sandbox Executor
**Deferred to Delta**: Meta-Evolution (StrategyOptimizer), Cross-Project Synapse, Active Hypothesis Testing

---

## 1. Design Decisions Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Gamma scope | 4 features (Layer 1 + L3 Sandbox) | Layer 1 is incremental on Beta; L3 fills the stub gap; Layer 3 needs runtime data first |
| Sandbox isolation | B+: Software-defined sandbox (restricted toolset + shadow FS) | R3 compliant; no Docker dependency; defense against logic bugs, not malicious attacks |
| De-solidification trigger | Triple-tiered (absolute + entropy + user feedback) | Defense in depth: circuit breaker + canary + human alignment |
| Retired skill handling | Archive to graveyard, not delete | Failed patterns become negative constraints for future generation |

---

## 2. Vitality Score Engine

### Goal
Replace the simple threshold checks in `SkillLifecycleState` with a continuous vitality metric that captures health, usage, cost, and human alignment.

### Files
- **New**: `core/src/skill_evolution/vitality.rs`
- **Modify**: `core/src/skill_evolution/lifecycle.rs` (use VitalityScore for transitions)
- **Modify**: `core/src/skill_evolution/tracker.rs` (add vitality computation query)

### Core Formula

```rust
pub struct VitalityScore {
    pub value: f32,           // 0.0..=1.0
    pub components: VitalityComponents,
}

pub struct VitalityComponents {
    pub success_rate: f32,              // successful / total executions
    pub frequency_score: f32,           // normalized invocation frequency (0..1)
    pub maintenance_cost_inverse: f32,  // 1.0 / normalized(avg_tokens + retry_overhead)
    pub user_feedback_multiplier: f32,  // 1.0 default, decreased by negative feedback
}

// vitality = success_rate × frequency_score × maintenance_cost_inverse × user_feedback_multiplier
```

### Frequency Score
- `frequency_score = min(invocations_last_30_days / expected_frequency, 1.0)`
- `expected_frequency` defaults to 10 (configurable)
- Unused skills (0 invocations in 30 days) get `frequency_score = 0.0`

### Maintenance Cost
- `raw_cost = avg_tokens_per_invocation + (avg_retries × retry_penalty)`
- `retry_penalty = 500` tokens equivalent
- `maintenance_cost_inverse = 1.0 / (1.0 + raw_cost / cost_normalizer)`
- `cost_normalizer = 2000` (baseline single LLM call)

### User Feedback Multiplier
- Default: `1.0`
- Each negative feedback event: `multiplier *= 0.7`
- Each positive feedback event: `multiplier = min(multiplier * 1.1, 1.0)` (recovery, capped)
- Decay: multiplier trends toward 1.0 over 7 days without feedback

### Thresholds

| Threshold | Default | Action |
|-----------|---------|--------|
| `vitality_healthy` | 0.5 | No action needed |
| `vitality_warning` | 0.3 | Enter observation period |
| `vitality_demotion` | 0.15 | Trigger demotion |
| `vitality_retirement` | 0.05 | Trigger retirement |

---

## 3. Auto-de-solidification (Triple-Tiered)

### Goal
Automatically detect and handle skill degradation through three defense layers.

### Files
- **New**: `core/src/skill_evolution/desolidification.rs`
- **Modify**: `core/src/skill_evolution/lifecycle.rs` (add Observation state)
- **Modify**: `core/src/poe/crystallization/dreaming.rs` (periodic vitality check)

### Layer 1: Circuit Breaker (Immediate)

```rust
pub struct CircuitBreakerConfig {
    pub consecutive_failure_limit: u32,  // default: 3
    pub success_rate_floor: f32,         // default: 0.5
    pub window_size: u32,                // default: 10 (last N executions)
}
```

- Checks after every execution via `EvolutionTracker::log_execution()`
- Triggers: consecutive failures >= limit OR success rate over window < floor
- Action: **Immediate demotion** (bypass vitality check)

### Layer 2: Entropy Canary (Early Warning)

```rust
pub struct EntropyCanaryConfig {
    pub trend_window: usize,         // default: 6 executions
    pub duration_degradation: f32,   // default: 0.5 (50% increase)
}
```

- Uses `CognitiveEntropyTracker::compute_trend()` per skill
- Triggers: entropy trend `Increasing` for 2 consecutive checks OR avg duration increased > 50%
- Action: **Reduce vitality score** by `entropy_penalty = 0.2`, potentially pushing below warning threshold

### Layer 3: Human Alignment Signal (Accelerator)

```rust
pub struct UserFeedbackEvent {
    pub skill_id: String,
    pub feedback_type: FeedbackType,  // Positive | Negative | ManualEdit
    pub timestamp: i64,
}
```

- Negative feedback / manual edit → `user_feedback_multiplier *= 0.7`
- Not a standalone trigger — amplifies Layer 1 and Layer 2 signals
- Stored in EvolutionTracker for lineage tracking

### Lifecycle Extension

Add `Observation` state between Shadow/Promoted and Demoted:

```
Draft → Shadow → Promoted → Observation → Demoted → Retired
                     ↑           │
                     └───────────┘ (recovery if vitality improves)
```

```rust
pub enum SkillLifecycleState {
    Draft,
    Shadow(ShadowState),
    Promoted { promoted_at: i64, shadow_duration_days: u32 },
    Observation { entered_at: i64, reason: ObservationReason, previous_vitality: f32 },
    Demoted { reason: String, demoted_at: i64, previous_state: String },
    Retired { reason: String, retired_at: i64 },
}

pub enum ObservationReason {
    VitalityWarning,
    EntropyIncreasing,
    UserFeedback,
}
```

- Recovery: if vitality rises above `vitality_healthy` during observation → return to Promoted
- Escalation: if vitality drops below `vitality_demotion` during observation → demote

---

## 4. KnowledgeConsolidator (Semantic Deduplication + Skill Merging)

### Goal
Prevent skill explosion by detecting semantically similar skills and merging them.

### Files
- **New**: `core/src/skill_evolution/consolidator.rs`
- **Modify**: `core/src/skill_evolution/pipeline.rs` (pre-deployment dedup check)
- **Modify**: `core/src/poe/crystallization/clustering.rs` (expose similarity API)

### Deduplication Flow

```
New Skill Candidate
    │
    ▼
Embedding Similarity Check (threshold: 0.85)
    │
    ├─ No similar skill found → proceed to deployment
    │
    └─ Similar skill(s) found
         │
         ├─ Existing skill has higher vitality → reject candidate, log as duplicate
         │
         └─ Candidate has higher vitality → trigger merge
```

### Merge Strategy

```rust
pub struct MergeDecision {
    pub winner_id: String,       // skill with higher vitality
    pub loser_id: String,        // skill to be absorbed
    pub merge_type: MergeType,
}

pub enum MergeType {
    /// Winner absorbs loser's parameter mappings and test cases
    Absorb,
    /// Both are retired, a new synthesized skill replaces them
    Synthesize,
}
```

- **Absorb** (default): winner skill gains loser's parameter mappings as aliases; loser is retired with `reason: "merged_into:{winner_id}"`
- **Synthesize** (when both have vitality > 0.5 but different strengths): call `PatternSynthesisBackend` to generate a unified skill from both patterns

### Similarity Computation
- Use `ExperienceStore::vector_search()` with skill description embeddings
- Threshold: cosine similarity > 0.85
- Runs during dreaming cycle (idle time only), not on hot path

---

## 5. L3 Sandbox Executor (Software-Defined Sandbox)

### Goal
Replace the Beta stub with a real isolated execution environment for High-risk skill validation.

### Files
- **New**: `core/src/skill_evolution/validation/sandbox_executor.rs`
- **New**: `core/src/skill_evolution/validation/restricted_tools.rs`
- **New**: `core/src/skill_evolution/validation/shadow_fs.rs`
- **Modify**: `core/src/skill_evolution/validation/tiered_validator.rs` (wire L3)

### Architecture: Software-Defined Sandbox

Three layers of isolation, all in userspace (no root, no Docker):

#### 5a. Shadow Filesystem

```rust
pub struct ShadowFs {
    /// Read-only source (original workspace)
    source_dir: PathBuf,
    /// Writable overlay (tempdir)
    overlay_dir: PathBuf,
}
```

- **Read operations**: transparently proxy to `source_dir` (read-only)
- **Write operations**: redirect to `overlay_dir`
- After execution: diff `overlay_dir` against expected outputs from test set
- Cleanup: `drop` impl removes `overlay_dir`

#### 5b. Restricted Toolset

```rust
pub struct RestrictedToolset {
    /// Allowed tool names (whitelist)
    allowed_tools: HashSet<String>,
    /// Root directory constraint (all path operations bounded)
    root_dir: PathBuf,
    /// Network access allowed
    allow_network: bool,
}

impl RestrictedToolset {
    /// Wrap a tool call, enforcing sandbox constraints.
    pub fn validate_call(&self, tool_name: &str, params: &serde_json::Value) -> Result<(), SandboxViolation>;
}
```

- Path validation: all file paths must resolve within `root_dir` (canonicalize + starts_with check)
- Tool whitelist: only tools required by the skill's pattern steps
- Network: disabled by default for High-risk skills

#### 5c. Process Executor

```rust
pub struct SandboxExecutor {
    shadow_fs: ShadowFs,
    restricted_tools: RestrictedToolset,
    timeout: Duration,           // default: 60s
    max_output_bytes: usize,     // default: 1MB
}

pub struct SandboxResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub modified_files: Vec<PathBuf>,   // files written to overlay
    pub duration: Duration,
    pub tool_calls: Vec<ToolCallRecord>,
    pub violations: Vec<SandboxViolation>,
}
```

- Execute skill steps sequentially in sandbox context
- Each tool call validated through `RestrictedToolset::validate_call()`
- Any violation → abort execution, record in `SandboxResult`
- Timeout enforced via `tokio::time::timeout`

### Validation Integration

Update `TieredValidator` to wire L3:

```
High-risk skills:
  L1 (Structural Linter) → L2 (Semantic Replay) → L3 (Sandbox Execution)

  L3 pass criteria:
  - Zero sandbox violations
  - Exit code 0
  - Modified files match expected outputs (> 80% overlap)
  - Duration < 60s

  L3 fail → reject skill (no human review fallback in Gamma)
```

---

## 6. Skill Graveyard (Failed Pattern Archive)

### Goal
Archive retired/demoted skills as negative constraints for future skill generation.

### Files
- **New**: `core/src/skill_evolution/graveyard.rs`
- **Modify**: `core/src/skill_evolution/shadow_deployer.rs` (archive on demotion/retirement)

### Structure

```rust
pub struct GraveyardEntry {
    pub skill_id: String,
    pub skill_md: String,           // original SKILL.md content
    pub failure_traces: Vec<String>, // IDs of failed experiences
    pub reason: String,             // why it was retired
    pub retired_at: i64,
    pub vitality_at_death: f32,
}

pub struct SkillGraveyard {
    entries: Vec<GraveyardEntry>,  // persisted to graveyard.json
}
```

### Integration with SkillGenerator
- When generating a new skill, query graveyard for similar patterns (embedding similarity > 0.7)
- Include matched graveyard entries as **negative examples** in the LLM prompt:
  ```
  AVOID these patterns that previously failed:
  - {graveyard_entry.skill_md} — Failed because: {reason}
  ```

### Storage
- File-based: `{evolved_skills_dir}/.graveyard/graveyard.json`
- Maximum entries: 100 (FIFO eviction of oldest)
- Graveyard entries are never auto-deleted — only evicted when full

---

## 7. Complete Pipeline Flow (Gamma)

```
POE Execution
    │
    ▼
ExperienceRecorder → EvolutionTracker
    │                      │
    │                      ├── VitalityScore computation (periodic)
    │                      │
    │                      ├── CircuitBreaker check (per execution)
    │                      │     └─ immediate demotion if triggered
    │                      │
    │                      └── UserFeedback recording
    │
    ├─────────────────────────────────────────┐
    │                                         │
    ▼                                         ▼
SolidificationDetector          CortexDreamingService (idle)
    │                                │
    │                                ├── Entropy Canary (Layer 2)
    │                                ├── KnowledgeConsolidator
    │                                └── Vitality periodic sweep
    │                                         │
    └──────────────┬──────────────────────────┘
                   ▼
         DistillationService
                   │
                   ▼
         PatternExtractor + ProviderBackend
                   │
                   ▼
         SkillGenerator ◄── Graveyard (negative constraints)
                   │
                   ▼
         DifferentialEngine
                   │
                   ▼
         TieredValidator
         ├─ L1 Structural
         ├─ L2 Semantic Replay
         └─ L3 Sandbox Executor (NEW — real execution)
             ├─ ShadowFs (read source, write overlay)
             ├─ RestrictedToolset (path + tool whitelist)
             └─ Process timeout (60s)
                   │
                   ▼
         KnowledgeConsolidator (dedup check)
         ├─ No similar → deploy
         └─ Similar found → merge or reject
                   │
                   ▼
         ShadowDeployer → evolved_skills/
                   │
                   ▼
         Lifecycle Manager (Gamma-enhanced)
         ├─ Promote (vitality > healthy)
         ├─ Observe (vitality warning)
         ├─ Demote (vitality < demotion OR circuit breaker)
         └─ Retire → Graveyard (archive, not delete)
```

---

## 8. Key Constraints & Invariants

1. **Vitality is continuous, not binary** — scores drive gradual transitions, not cliff-edge decisions
2. **Circuit breaker is immediate** — bypasses vitality for catastrophic failures
3. **Sandbox is userspace only** — no root, no Docker, no chroot; R3 compliant
4. **Path operations bounded** — all sandbox file access canonicalized and checked against root_dir
5. **Graveyard is append-only** — entries never deleted, only evicted when full (FIFO, max 100)
6. **Consolidation runs idle-only** — never competes with POE execution (same as dreaming)
7. **Observation is reversible** — skills can recover from observation back to promoted
8. **User feedback amplifies, never solely triggers** — human signal is a multiplier, not a switch

---

## 9. Future (Delta) Extensions

Explicitly deferred, depend on Gamma stability and runtime data:

- **Meta-Evolution (StrategyOptimizer)**: Optimize solidification thresholds, generator prompts, vitality weights based on historical success/failure patterns
- **Cross-Project Synapse**: Federated pattern fingerprint exchange with privacy-preserving hashing
- **Active Hypothesis Testing**: Proactive exploration when pattern confidence is low — generate controlled experiments in sandbox
- **Graveyard Resurrection**: Periodically re-evaluate graveyard entries against new environment state; auto-resurrect if conditions changed
