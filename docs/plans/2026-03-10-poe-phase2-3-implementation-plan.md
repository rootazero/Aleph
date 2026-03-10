# POE Phase 2+3: Recursive POE + Memory Decay + Phase 3 Interfaces

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable POE to handle complex multi-step tasks via recursive decomposition, maintain knowledge quality through memory decay, and lay groundwork for Phase 3 (parallel execution, sandboxing, federation).

**Architecture:** Phase 2 extends PoeManager's execution loop with decomposition detection and sub-task orchestration. Memory decay adds weight tracking to ExperienceStore. Phase 3 introduces trait abstractions (ExecutionEnvironment, ValidatorRole) without implementation.

**Tech Stack:** Rust, async/await, existing POE infrastructure.

**Design Reference:** `docs/plans/2026-03-10-poe-evolution-whitepaper.md`

---

## Task 1: Decomposition Detector (P-stage heuristic)

**Files:**
- Create: `core/src/poe/decomposition/mod.rs`
- Create: `core/src/poe/decomposition/detector.rs`
- Modify: `core/src/poe/mod.rs`

Implement `DecompositionDetector` that analyzes a SuccessManifest to determine if it should be split:

```rust
pub struct DecompositionDetector;

pub enum DecompositionAdvice {
    /// Task is simple enough for single execution
    Proceed,
    /// Task should be decomposed (with suggested sub-objectives)
    Decompose { sub_objectives: Vec<String>, reason: String },
}

impl DecompositionDetector {
    /// P-stage heuristic check (no LLM cost)
    pub fn analyze(manifest: &SuccessManifest) -> DecompositionAdvice
}
```

Heuristic rules:
- `hard_constraints.len() > 5` AND constraints touch 3+ distinct directories → Decompose
- Objective contains compound verbs ("and", "then", "also") with distinct actions → Decompose
- All constraints in same directory/concern → Proceed

Tests (6): simple task proceeds, complex multi-dir decomposes, compound objective detected, single-concern proceeds, threshold boundary cases.

---

## Task 2: Sub-Manifest Generator (LLM-assisted decomposition)

**Files:**
- Create: `core/src/poe/decomposition/generator.rs`
- Modify: `core/src/poe/decomposition/mod.rs`

```rust
pub struct SubManifestGenerator;

impl SubManifestGenerator {
    /// Generate sub-manifests from a parent manifest using LLM
    pub async fn generate(
        parent: &SuccessManifest,
        reason: &str,
        provider: &Arc<dyn AiProvider>,
    ) -> Result<Vec<SuccessManifest>>

    /// Generate sub-manifests from worker's NeedsDecomposition signal
    pub async fn from_worker_signal(
        parent: &SuccessManifest,
        sub_objectives: Vec<String>,
        provider: &Arc<dyn AiProvider>,
    ) -> Result<Vec<SuccessManifest>>
}
```

Tests (4): generates valid sub-manifests, inherits parent task_id prefix, respects max sub-tasks limit, fallback on LLM failure.

---

## Task 3: Recursive PoeManager (sub-task orchestration)

**Files:**
- Modify: `core/src/poe/manager.rs` (add `execute_recursive` method)
- Modify: `core/src/poe/types.rs` (add NeedsDecomposition to WorkerState)

Add `execute_recursive` to PoeManager that:
1. Calls `DecompositionDetector::analyze()` before execution (P-stage)
2. If Decompose → generate sub-manifests → execute each recursively
3. During execution, if Worker returns NeedsDecomposition → split (O-stage)
4. If E-stage detects distance_score stagnation + partial pass/fail → split
5. Tracks `current_depth` and enforces `config.max_depth`

```rust
impl<W: Worker + Clone> PoeManager<W> {
    pub async fn execute_recursive(
        &self,
        task: PoeTask,
        depth: u8,
        provider: &Arc<dyn AiProvider>,
    ) -> Result<PoeOutcome>
}
```

Add `NeedsDecomposition` to `WorkerState`:
```rust
NeedsDecomposition {
    sub_objectives: Vec<String>,
    reason: String,
},
```

Tests (5): simple task runs normally, P-stage decomposition triggers, max_depth enforced, sub-task results aggregate, worker NeedsDecomposition handled.

---

## Task 4: E-stage Decomposition Trigger

**Files:**
- Modify: `core/src/poe/manager.rs` (add decomposition detection in E-stage)

In the execute loop, after stuck detection but before retry, check if decomposition is warranted:
- distance_score hasn't improved over 2 attempts AND
- Some hard constraints pass while others consistently fail → different concerns
- Generate sub-manifests splitting passing and failing concerns

```rust
fn should_decompose_on_evaluation(
    budget: &PoeBudget,
    verdict: &Verdict,
    manifest: &SuccessManifest,
) -> Option<Vec<String>>
```

Tests (3): triggers on mixed pass/fail pattern, doesn't trigger on uniform failure, respects min attempts before triggering.

---

## Task 5: Memory Decay Core Types

**Files:**
- Create: `core/src/poe/memory_decay/mod.rs`
- Create: `core/src/poe/memory_decay/decay.rs`
- Modify: `core/src/poe/mod.rs`

```rust
/// Decay-aware experience wrapper
pub struct DecayableExperience {
    pub experience: PoeExperience,
    pub effective_weight: f32,
    pub performance_factor: f32,
    pub drift_factor: f32,
    pub time_factor: f32,
    pub last_reused: Option<i64>,
    pub reuse_count: u32,
    pub reuse_success_count: u32,
}

pub struct DecayConfig {
    pub time_half_life_days: u32,     // default: 90
    pub min_reuses_for_decay: u32,    // default: 3
    pub archive_threshold: f32,       // default: 0.1
    pub performance_window: u32,      // default: 5 (last N reuses)
}

pub struct DecayCalculator;

impl DecayCalculator {
    /// Calculate effective weight from all decay factors
    pub fn calculate(experience: &DecayableExperience, config: &DecayConfig) -> f32

    /// Calculate performance factor from recent reuse history
    pub fn performance_factor(success_count: u32, total_count: u32) -> f32

    /// Calculate environment drift factor
    pub fn drift_factor(related_files_changed: usize, total_related_files: usize) -> f32

    /// Calculate time decay factor
    pub fn time_factor(age_days: f64, half_life_days: u32) -> f32

    /// Check if experience should be archived
    pub fn should_archive(weight: f32, config: &DecayConfig) -> bool
}
```

Decay formula: `effective_weight = base × performance × drift × time_decay`

Tests (8): performance factor calculations, drift factor, time decay (half-life), combined formula, archive threshold, edge cases (zero reuses, brand new experience).

---

## Task 6: Reuse Tracker

**Files:**
- Create: `core/src/poe/memory_decay/reuse_tracker.rs`
- Modify: `core/src/poe/memory_decay/mod.rs`

Track when experiences are reused and whether the reuse led to success or failure:

```rust
pub struct ReuseRecord {
    pub experience_id: String,
    pub reused_at: i64,
    pub led_to_success: bool,
    pub task_id: String,
}

pub struct InMemoryReuseTracker {
    records: HashMap<String, Vec<ReuseRecord>>,
}

impl InMemoryReuseTracker {
    pub fn record_reuse(&mut self, record: ReuseRecord)
    pub fn get_recent(&self, experience_id: &str, limit: usize) -> Vec<&ReuseRecord>
    pub fn success_rate(&self, experience_id: &str, window: usize) -> f32
}
```

Tests (5): record and retrieve, success rate calculation, window limiting, empty returns 1.0 (benefit of doubt), multiple experiences tracked independently.

---

## Task 7: Decay-Aware Experience Retrieval

**Files:**
- Create: `core/src/poe/memory_decay/filtered_store.rs`
- Modify: `core/src/poe/memory_decay/mod.rs`

Wraps ExperienceStore with decay filtering:

```rust
pub struct DecayFilteredStore<S: ExperienceStore> {
    inner: S,
    decay_config: DecayConfig,
    reuse_tracker: Arc<RwLock<InMemoryReuseTracker>>,
}

impl<S: ExperienceStore> DecayFilteredStore<S> {
    /// Search with decay-weighted results, filtering archived experiences
    pub async fn weighted_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        min_similarity: f64,
    ) -> Result<Vec<(PoeExperience, f64, f32)>>  // (experience, similarity, effective_weight)
}
```

Tests (4): low-weight experiences filtered, high-weight preserved, weight affects ranking, archived excluded.

---

## Task 8: ExecutionEnvironment Trait (Phase 3 Foundation)

**Files:**
- Create: `core/src/poe/execution_env/mod.rs`
- Create: `core/src/poe/execution_env/host.rs`
- Modify: `core/src/poe/mod.rs`

```rust
/// Abstraction over command execution environment.
/// Phase 1: HostEnvironment (direct execution)
/// Phase 3: SandboxEnvironment (container/WASM isolation)
#[async_trait]
pub trait ExecutionEnvironment: Send + Sync {
    async fn execute_command(
        &self,
        cmd: &str,
        args: &[String],
        timeout_ms: u64,
        working_dir: Option<&Path>,
    ) -> Result<CommandOutput>;

    fn name(&self) -> &str;
}

pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

/// Direct host execution (current behavior)
pub struct HostEnvironment;
```

Migrate HardValidator's command execution to use ExecutionEnvironment trait.

Tests (3): HostEnvironment executes successfully, timeout works, captures exit code.

---

## Task 9: ValidatorRole Enum (Phase 3 Foundation)

**Files:**
- Modify: `core/src/poe/types.rs` (add ValidatorRole)
- Modify: `core/src/poe/validation/composite.rs` (accept role parameter)
- Modify: `core/src/poe/mod.rs`

```rust
/// Role of a validator in the evaluation pipeline.
/// Phase 1: NormalCritic (default)
/// Phase 3: AdversarialCritic ("find everything wrong")
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ValidatorRole {
    #[default]
    NormalCritic,
    AdversarialCritic,
}
```

Add `role: ValidatorRole` field to `CompositeValidator` (default NormalCritic). Add builder method `with_role()`. The role is currently unused — Phase 3 will modify the semantic validation prompt based on it.

Tests (2): default role is NormalCritic, with_role sets correctly.

---

## Task 10: Integration Tests + Final Verification

**Files:**
- Create: `core/tests/poe_phase2_integration.rs`

End-to-end tests:
1. DecompositionDetector correctly identifies complex manifests
2. DecayCalculator produces expected weights
3. Memory decay formula convergence (experience decays over time)
4. ExecutionEnvironment trait works with HostEnvironment
5. ValidatorRole default behavior
6. Full recursive POE with mock decomposition

---

## Summary

| Task | Component | Phase | Tests |
|------|-----------|-------|-------|
| 1 | DecompositionDetector | 2 | 6 |
| 2 | SubManifestGenerator | 2 | 4 |
| 3 | Recursive PoeManager | 2 | 5 |
| 4 | E-stage decomposition | 2 | 3 |
| 5 | Memory Decay types | 2 | 8 |
| 6 | Reuse Tracker | 2 | 5 |
| 7 | Decay-Aware Retrieval | 2 | 4 |
| 8 | ExecutionEnvironment | 3 | 3 |
| 9 | ValidatorRole | 3 | 2 |
| 10 | Integration tests | 2+3 | 6 |
| **Total** | | | **46** |
