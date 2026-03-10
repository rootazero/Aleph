# Aleph POE Architecture Evolution Whitepaper 2026

> POE (Principle-Operation-Evaluation) 架构演进白皮书
> Date: 2026-03-10
> Status: Approved Design

## 1. Executive Summary

将 POE 从"功能完备的单循环 Agent 框架"演进为"鲁棒、可递归、自进化的多主体智能架构"。

**演进路径**：

```
Phase 1: 鲁棒性 (敢不敢用)  →  Phase 2: 复杂性 (能不能做)  →  Phase 3: 极致性 (做得多好)
   BlastRadius + Taboo           Recursive POE + Decay          Parallel + Sandbox + Federation
```

**核心约束**：遵守 R1 (脑肢分离)、R3 (核心轻量化)、P6 (KISS/YAGNI)，每个 Phase 只引入该阶段必需的复杂度。

## 2. Current Architecture Assessment

### 2.1 Completeness

POE 架构建立了端到端自主学习型 Agent 闭环：

- **P (Principle)**: SuccessManifest 建立第一性原理和契约约束
- **O (Operation)**: Worker 层提供执行抽象，解耦具体执行模型
- **E (Evaluation)**: CompositeValidator 结合硬性（确定性检查）和软性（语义模型）检查
- **外围生态**: Budget（熵减预算）、Trust（渐进式授权）、Crystallization/Dreaming（经验结晶与睡梦学习）、Meta-Cognition（失败反思）、Event Sourcing（Projectors）

### 2.2 Validity

- **物理隔离杜绝"裁判下场踢球"**：PoeManager 编排，Validator 作为独立 Critic，解决传统 Agent 自我评判导致的幻觉通过问题
- **数学视角的死循环切断**：PoeBudget 通过 entropy_history (distance_score) 进行停滞检测，非简单 max_attempts 阈值

### 2.3 Advanced Nature

- **System 1 + System 2**: Worker + HardValidator 构成快速反馈（System 1），SemanticValidator + LLM Meta-cognition 构成慢系统（System 2）
- **跨会话演化**: Crystallization + Dreaming 沉淀 TaskPattern/SolutionPath，TrustEvaluator 实现渐进式自主

## 3. Phase 1: Robustness & Defensive Enhancement

> 核心目标：解决"敢不敢用"的问题

### 3.1 BlastRadius (Blast Radius Estimation)

#### 3.1.1 Data Structure

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlastRadius {
    /// Impact scope (0.0-1.0): file count, module depth, user coverage
    pub scope: f32,

    /// Destructiveness (0.0-1.0): data deletion, prod config changes, core protocol modifications
    pub destructiveness: f32,

    /// Reversibility (0.0-1.0): 1.0 = fully reversible (git-tracked), 0.0 = irreversible (remote API calls)
    pub reversibility: f32,

    /// Computed risk level
    pub level: RiskLevel,

    /// Human-readable reasoning for UI display
    pub reasoning: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    Negligible, // e.g., modify README
    Low,        // e.g., add test cases
    Medium,     // e.g., refactor non-core module
    High,       // e.g., modify auth middleware, DB migration
    Critical,   // e.g., wipe data directory, modify system environment
}
```

`BlastRadius` is embedded as an **optional field** in `SuccessManifest`, ensuring risk information flows with the contract through all layers.

#### 3.1.2 Hybrid Assessment Engine

**System 1: StaticSafetyScanner (Deterministic)**

Zero LLM cost, pattern-based detection:

- **Keyword filtering**: Detect sensitive commands in `ValidationRule::CommandPasses` (rm, drop, delete, format, chmod, sudo)
- **Path sensitivity**: Detect operations on `.env`, `id_rsa`, `/etc/`, bulk `node_modules` changes
- **Rule combination**: Multiple delete rules in a single manifest auto-escalate risk weight

**System 2: SemanticRiskAnalyzer (LLM-based)**

Invoked only when System 1 returns indeterminate (gray zone):

- **Context-aware**: Evaluate objective vs codebase architecture (modifying `core/src/lib.rs` >> `examples/`)
- **Side-effect reasoning**: Detect cascading impacts (e.g., modifying a common Error enum breaks dozens of modules)

**Critical Principle: System 1 conclusions are NEVER downgraded by System 2.**

#### 3.1.3 Safety Boundary Tiers

| Tier | Definition | Examples | Behavior |
|------|-----------|----------|----------|
| **Tier 0: Hard Reject** | Destructive enough to damage the host OS or cause irrecoverable physical damage | `rm -rf /`, fork bomb `:(){ :\|:& };:`, `dd if=... of=/dev/sda` | Immediate termination → `PoeError::SecurityViolation` |
| **Tier 1: Mandatory Signature** | Significant data loss, credential exposure, network boundary breach, external publishing | `sudo *`, `DROP TABLE`, `git push --force`, read/write `~/.ssh/`, `npm publish` | Force `RiskLevel::Critical`, ignore all Trust scores, require explicit user confirmation |

Tier 0 has **no override mechanism**. Simplicity is security.

**Tier 1 Detailed Rules:**

- **Data destruction**: Any `rm` on non-`target/` non-git-tracked files; SQL `DROP TABLE`, `TRUNCATE`
- **Credential access**: Read/write files matching `id_rsa`, `.env`, `*secret*`, `*key*`
- **Privilege escalation**: `sudo`, `chown`, `chmod +s`
- **Environment mutation**: Modify `package.json` dependencies (not devDependencies); modify core deps in `Cargo.toml`
- **Network side-effects**: `curl -X POST`, `wget`, `ssh`, `docker push`, `npm publish`

#### 3.1.4 LLM Fallback: Presumption of Guilt

| System 2 Failure Scenario | Fallback Behavior |
|---|---|
| LLM timeout | → `RiskLevel::High` + mandatory signature |
| JSON parse failure | → `RiskLevel::High` + mandatory signature |
| Confidence < 0.6 | → `RiskLevel::High` + mandatory signature |
| LLM returns `Negligible` but System 1 flagged suspicious | → Take the **higher** of System 1 and System 2 |

#### 3.1.5 Reversibility Compensation

If PoeManager confirms the workspace is in a Clean Git State and the task only affects version-controlled files, `High` can be downgraded to `Medium` due to high reversibility. This encourages Workers to establish strong rollback points via `Worker::snapshot()` before high-risk operations.

#### 3.1.6 Decision Matrix

| Risk Level | TrustEvaluator Guidance | Final Behavior |
|---|---|---|
| Negligible | Ignore experience score | Auto-approve |
| Low / Medium | Depend on success rate | Auto-execute if trust > 0.8, else require signature |
| High | Suspect even with rich experience | Mandatory signature + UI risk warning |
| Critical | No autonomy allowed | Mandatory user line-by-line confirmation |

#### 3.1.7 Integration Workflow

```
ManifestBuilder.build(instruction)
    → SuccessManifest (without BlastRadius)
    → StaticSafetyScanner.scan(manifest)
        → if Critical pattern: BlastRadius { level: Critical } → skip LLM
        → if Negligible pattern: BlastRadius { level: Negligible } → skip LLM
        → else: SemanticRiskAnalyzer.analyze(manifest, codebase_context)
            → BlastRadius { scope, destructiveness, reversibility, level, reasoning }
    → manifest.blast_radius = Some(blast_radius)
    → TrustEvaluator.evaluate(manifest)
        → Decision based on Decision Matrix
```

### 3.2 Taboo Crystallization (Anti-Pattern Learning)

#### 3.2.1 Dual-Threshold Two-Stage Trigger

**Stage 1: Micro-Taboo (In-Loop Interception)**

- **Trigger**: N=3 consecutive failures with semantically identical RootCause (via MetaCognition tagging)
- **Detection**: TabooBuffer sliding window matches failure features (PermissionDenied, DependencyMismatch, LogicInconsistency)
- **Action**: Generate a Transient Taboo and inject into next Worker prompt:
  > "You have attempted method A three times with error B. This path is FORBIDDEN. Find a completely different strategy."
- **Value**: Real-time token savings by breaking repetitive loops

**Stage 2: Macro-Taboo (Post-Mortem on Exhaustion)**

- **Trigger**: `PoeOutcome::BudgetExhausted`
- **Action**:
  1. MetaCognition traverses full StepLog chain
  2. Extract failure pattern: why ALL attempts (including post-StrategySwitch paths) failed
  3. Persist as `AntiPattern` in ExperienceStore (vector DB)
- **Value**: Long-term immunity — next similar TaskPattern gets "avoid" guidance injected at P-phase

#### 3.2.2 TabooBuffer Component

```rust
pub struct TabooBuffer {
    /// Sliding window of recent N verdicts with RootCause tags
    window: VecDeque<TaggedVerdict>,
    /// Threshold for triggering Micro-Taboo
    repetition_threshold: usize, // default: 3
}

pub struct TaggedVerdict {
    pub verdict: Verdict,
    pub root_cause: Option<RootCause>,
    pub semantic_tag: String, // e.g., "PermissionDenied", "DependencyMismatch"
}
```

Resides in PoeManager. Lightweight, no LLM cost for detection (reuses MetaCognition tags).

#### 3.2.3 Recall Path

```
New Task → ManifestBuilder.build()
    → ExperienceStore.search(task_pattern)
        → Returns both SolutionPaths (do this) AND AntiPatterns (avoid this)
    → ManifestBuilder injects AntiPatterns as negative constraints
    → SuccessManifest includes "avoid" guidance in context
```

#### 3.2.4 Why (c) Over (b) for Trigger Source

- `StrategySwitch` is a **normal scheduling behavior** (switch strategy, keep trying) — not necessarily a failure signal
- Only "repeatedly hitting the same wall" reveals true thinking inertia or logical dead-loops
- Reuses existing MetaCognition `RootCause` detection — no new infrastructure needed

## 4. Phase 2: Complex Task Handling & Knowledge Maintenance

> 核心目标：解决"能不能做复杂任务"和"长期运行效率"的问题

### 4.1 Recursive POE Tree

#### 4.1.1 Trinity Trigger Matrix

| Phase | Role | Signal | Value |
|-------|------|--------|-------|
| **P** | Top-level planner | Heuristic rules (cross-dir > 3, constraints > 5, keywords like "refactor and add") | Static entropy reduction before execution |
| **O** | Frontline executor | Worker returns `NeedsDecomposition` (discovers hidden dependencies at runtime) | Dynamic discovery of Unknown Unknowns |
| **E** | Quality gatekeeper | distance_score stagnation + constraint conflict (partial PASS, partial consecutive FAIL) | Adaptive correction as last defense |

#### 4.1.2 Data Structure Extension

```rust
pub enum PoeOutcome {
    Success(Verdict),
    /// Decomposition triggered: suspend current task, execute sub-tasks
    DecompositionRequired {
        sub_manifests: Vec<SuccessManifest>,
        reason: String,
    },
    StrategySwitch { /* ... */ },
    BudgetExhausted { /* ... */ },
}
```

#### 4.1.3 Parent-Child Execution Model

1. Parent task enters `Pending` state when `DecompositionRequired` fires
2. Each sub-manifest executes as an independent POE cycle
3. Sub-task artifacts and experiences flow back as parent's heuristic input for next attempt
4. Parent's validation rule: "all child POE contracts fulfilled"

#### 4.1.4 Safety Valve

```rust
pub struct PoeConfig {
    /// Maximum recursion depth for nested POE tasks
    pub max_depth: u8, // recommended: 3
    // ...
}
```

At max depth, system forces single-level mode and requests human intervention.

#### 4.1.5 Why O-Stage Introspection Matters

In complex coding tasks, Unknown Unknowns only surface when the Worker actually reads code and analyzes call stacks:

- P-only: Over-decomposes simple tasks "just in case"
- E-only: Wastes tokens trying to force a single Worker through logically isolated sub-tasks
- O-stage: "On-demand decomposition" — the optimal balance point

### 4.2 Memory Decay & Forgetting

#### 4.2.1 Decay Formula

```
effective_weight = base_weight
    × performance_factor(success_rate_last_N)
    × drift_factor(related_files_change_ratio)
    × time_decay(age, half_life=90d)
```

#### 4.2.2 Factor Weights

| Factor | Role | Weight | Rationale |
|--------|------|--------|-----------|
| **Performance** | Primary | High | Recent N reuse success/failure ratio — a pattern that fails on reuse is actively harmful |
| **Environment Drift** | Primary | High | Git diff stat of related files — major version upgrades invalidate old patterns |
| **Time** | Weak correction | Low | Tiebreaker for equal conditions — prefer fresh over stale, but don't discard proven patterns |

#### 4.2.3 Lifecycle

```
Active (effective_weight > threshold)
    → Decaying (weight declining on reuse failure or env drift)
    → Archived (weight < threshold, excluded from retrieval)
```

Archived experiences remain in storage for audit/analysis but are invisible to retrieval queries.

#### 4.2.4 Design Rationale

Time alone is a crude proxy. In software engineering:
- A 2-year-old correct pattern with unchanged environment is still gold
- A 1-week-old bad habit should be discarded immediately

Performance and environment drift capture the actual information content.

## 5. Phase 3: Interface Reservations & Long-Term Vision

> 核心目标：解决"效率极限"与"绝对安全"的问题
> Phase 1/2 中预留接口，Phase 3 实施时不需推倒重来

### 5.1 Speculative Parallel Execution

**Vision**: On StrategySwitch, spawn N parallel Workers (e.g., one tries Python, another tries Bash). First to pass CompositeValidator wins; others are cancelled.

**Interface Reservations (Phase 1/2)**:

- Worker trait: Add `supports_isolation() -> bool` capability declaration for clone/fork support
- StateSnapshot: Support branching snapshots beyond single-directory `git stash` — enable Workspace Isolation for truly parallel workers
- PoeManager: Ensure execution loop can be extended with `spawn_parallel_workers()` without breaking existing single-worker flow

### 5.2 Sandboxing & Adversarial Verification

**Vision**: Container/WASM execution isolation + Red Team LLM adversarial validation.

**Interface Reservations (Phase 1/2)**:

- Extract command execution into `ExecutionEnvironment` trait:
  ```rust
  pub trait ExecutionEnvironment: Send + Sync {
      async fn execute_command(&self, cmd: &str, args: &[String], timeout_ms: u64) -> Result<CommandOutput>;
  }
  ```
  Phase 1 implements `HostEnvironment` only
- Add `role: ValidatorRole` field to semantic ValidationRules:
  ```rust
  pub enum ValidatorRole {
      NormalCritic,       // Default: "did it work?"
      AdversarialCritic,  // Phase 3: "find everything wrong with it"
  }
  ```
  Default to `NormalCritic`; Phase 3 extends without modifying Validator dispatch

### 5.3 Multi-Agent POE Federation

**Vision**: Planner Agent generates SuccessManifest, Coder Agent (Worker) executes, QA Agent (Validator) verifies, Security Agent (HardValidator) gates. All communicate via PoeEventBus.

**Interface Reservations (Phase 1/2)**:

- PoeEventEnvelope: Ensure `correlation_id` exists (already present)
- PoeTask: Add `metadata: HashMap<String, String>` for future federation routing labels (e.g., `required_capability: "rust_expert"`)
- PoeEventBus: Keep event protocol generic enough for different Agents to subscribe/publish without modifying event types

## 6. Architecture Principles

| Principle | Embodiment |
|-----------|-----------|
| System 1 never downgraded by System 2 | BlastRadius deterministic authority |
| Presumption of guilt | LLM failure → fail-safe to High |
| Simplicity is security | Tier 0/1 two-layer, no override mechanism |
| On-demand decomposition | O-stage introspection avoids over/under-splitting |
| Performance > Time | Memory Decay measures actual results, not age |
| Reserve, don't pre-implement | Phase 3 stubs only, no dead code |

## 7. Dependency Graph

```
Phase 1 (independent)
├── BlastRadius ← extends ManifestBuilder + TrustEvaluator
└── Taboo ← extends Crystallization + MetaCognition + PoeManager (TabooBuffer)

Phase 2 (depends on Phase 1)
├── Recursive POE ← extends PoeManager + PoeOutcome + Worker
│   └── uses: BlastRadius (sub-task risk assessment)
│   └── uses: Taboo (sub-task avoids known anti-patterns)
└── Memory Decay ← extends ExperienceStore + MemoryProjector
    └── uses: Taboo recall path (AntiPatterns have decay too)

Phase 3 (depends on Phase 2)
├── Speculative Parallel ← extends PoeManager + StateSnapshot + Worker
├── Sandboxing ← extends HardValidator + ExecutionEnvironment trait
├── Adversarial Validation ← extends SemanticValidator + ValidatorRole
└── Federation ← extends PoeEventBus + PoeTask metadata
```

## 8. Success Criteria

| Phase | Metric | Target |
|-------|--------|--------|
| Phase 1 | Tier 0 detection rate for known destructive patterns | 100% (zero false negatives) |
| Phase 1 | Micro-Taboo token savings on repetitive failures | >50% reduction vs baseline |
| Phase 2 | Complex task completion rate (multi-directory refactors) | >70% without human intervention |
| Phase 2 | Stale experience retrieval rate after decay | <5% of total retrievals |
| Phase 3 | Parallel execution wall-clock speedup | >1.5x on multi-strategy tasks |
| Phase 3 | Adversarial validation escape rate | <10% of injected test vulnerabilities |
