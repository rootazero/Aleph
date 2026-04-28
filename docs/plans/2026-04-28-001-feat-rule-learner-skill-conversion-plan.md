---
title: "P0-1: Enable RuleLearner → Skill Conversion"
type: feat
status: draft
date: 2026-04-28
origin: docs/brainstorms/2026-04-28-aleph-rule-learner-enablement.md
last_review: 2026-04-28
reviewers: feasibility, adversarial, coherence, design-lens, librarian
---

# P0-1: Enable RuleLearner → Skill Conversion

## ⚠️ CRITICAL CORRECTIONS — READ BEFORE PROCEEDING

**This plan was built on incorrect assumptions about the codebase. The original plan assumed:**
- `RuleLearner` generates `SkillManifest`
- `LearningAgent` integrates with `SkillSystem`
- `DreamDaemon` actively processes skills

**Actual architecture (verified):**

| Assumption | Reality | Impact |
|------------|----------|--------|
| RuleLearner → SkillManifest | RuleLearner → `KeywordRule` (L2 routing) | Completely different output type |
| LearningAgent → SkillSystem | LearningAgent → ReflexLayer (L2) | No SkillSystem integration exists |
| DreamDaemon runs pipeline | `run_dream()` is a STUB — returns empty report | SkillDistillStage never executes |
| `learn()` method exists | Methods are `learn_success()` / `learn_failure()` | Method signatures wrong |
| `LearningAgent` has `skill_system` field | `LearningAgent` has `learner` + `reflex_layer` only | Architectural change required |
| `SkillSystem::install_skill()` exists | Only `register_external(Vec<SkillManifest>)` exists | Wrong API |
| `RuleLearner::generate_rules()` → SkillManifest | Returns `Vec<KeywordRule>` | Wrong type |
| `MIN_EXECUTIONS=3` in RuleLearner | `MIN_EXECUTIONS=3`, `MIN_CONFIDENCE=0.8` in RuleLearner; `MIN_OBSERVATIONS=100` in LearningAgent | Different thresholds |
| `ReflexLayer::execute_l3()` exists | No such method — L3 routing is internal | Wrong hook point |

---

## Problem Frame (Corrected)

Aleph has **two completely separate learning systems** that were conflated in the original plan:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    SYSTEM A: L2 Routing (RuleLearner)                    │
│                                                                         │
│  ReflexLayer (L1/L2/L3 routing)                                       │
│       │                                                                 │
│       ├── L2: KeywordRule (fast-path routing)                         │
│       │                                                                  │
│       └── LearningAgent → RuleLearner → KeywordRule → ReflexLayer      │
│                              ↓                                          │
│                    PatternRecord (in-memory)                             │
│                                                                         │
│  OUTPUT: L2 KeywordRule (speeds up routing, not a "skill")            │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                    SYSTEM B: Skill System (DreamDaemon)                  │
│                                                                         │
│  DreamDaemon (background synthesis)                                     │
│       │                                                                 │
│       ├── NoteLayer → NoteIndexer → KnowledgeNote                       │
│       │                                                                 │
│       └── SkillDistillStage → KnowledgeNote (category="skill")          │
│                                                                         │
│  OUTPUT: KnowledgeNote (markdown, NOT SkillManifest)                   │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────┐
│                    SYSTEM C: Skill Registration (SkillSystem)            │
│                                                                         │
│  SkillSystem::register_external(Vec<SkillManifest>)                    │
│       │                                                                 │
│       └── SkillRegistry                                                 │
│                                                                         │
│  INPUT: SkillManifest (not KnowledgeNote)                               │
└─────────────────────────────────────────────────────────────────────────┘
```

**There is NO bridge between these systems.** The original plan incorrectly assumed RuleLearner could produce SkillManifest directly.

---

## Two Valid Integration Paths

Given the actual architecture, there are two valid approaches:

### Path A: L2 Routing (Simpler — RuleLearner already does this)

```
L3 Success → LearningAgent::on_l3_success() → RuleLearner::learn_success()
    → PatternRecord → KeywordRule → ReflexLayer (L2)
```

This already exists in the code (dead code — not wired). It speeds up L2 routing, not skill creation.

**Pros**: Already implemented, minimal new code
**Cons**: Produces L2 routing rules, not skills visible to the skill system

### Path B: Skill Creation (DreamDaemon → SkillSystem)

```
DreamDaemon::run_dream() [STUB — needs fixing]
    → DreamPipeline → SkillDistillStage [NEVER CALLED]
    → KnowledgeNote (category="skill") [NOTE: wrong type]
    → [NEW] KnowledgeNote → SkillManifest conversion
    → SkillSystem::register_external()
```

**Pros**: Produces actual skills discoverable by the skill system
**Cons**: DreamDaemon is a stub, SkillDistillStage outputs wrong type

### Path C: Hybrid (Recommended)

```
L3 Success → LearningAgent → RuleLearner
    ├── High-confidence pattern → [NEW] PatternRecord → SkillManifest → SkillSystem
    └── All patterns → KeywordRule → ReflexLayer (L2)
```

RuleLearner accumulates patterns. When a pattern reaches `MIN_EXECUTIONS=3, MIN_CONFIDENCE=0.8`, convert it to a SkillManifest and register with SkillSystem. Simultaneously continue producing L2 KeywordRules for ReflexLayer.

---

## Requirements (Corrected & Renumbered)

- **R1**: RuleLearner module is active (`#[allow(dead_code)]` removed from `engine/mod.rs`)
- **R2**: `LearningAgent::on_l3_success()` is wired to L3 execution (callback hook in Harness)
- **R3**: RuleLearner learns from L3 success via `learn_success(input, action)`
- **R4**: RuleLearner accumulates `PatternRecord`s in `DashMap`
- **R5**: When `PatternRecord` meets `MIN_EXECUTIONS=3, MIN_CONFIDENCE=0.8`, generate a `SkillManifest`
- **R6**: [NEW] Implement `RuleLearner::generate_skill_manifests()` — returns `Vec<(SkillManifest, SkillContent)>`
- **R7**: [NEW] LearningAgent needs `skill_system: Arc<SkillSystem>` field to register skills
- **R8**: Generated skill registered via `SkillSystem::register_external(Vec<SkillManifest>)`
- **R9**: Skill content stored as markdown in `~/.aleph/skills/learned/`
- **R10**: Unit tests cover PatternRecord accumulation and SkillManifest generation
- **R11**: Integration test verifies L3 success → LearningAgent → SkillSystem registration
- **R12**: DreamDaemon `run_dream()` stub is fixed OR we bypass DreamDaemon entirely (Path C hybrid)

---

## Scope Boundaries

- **In scope**: Path C Hybrid — wiring LearningAgent + new PatternRecord → SkillManifest conversion
- **Out of scope**: DreamDaemon stub fix (deferred to separate P0)
- **Out of scope**: KnowledgeNote → SkillManifest conversion (bypassed by Path C)
- **Out of scope**: Failure feedback learning (P1-3)
- **Out of scope**: RuleLearner state persistence across restarts (D3 deferred)

---

## Key Method Signatures (Verified)

| Component | Method | Actual Signature |
|----------|--------|-----------------|
| `RuleLearner` | `learn_success` | `fn learn_success(&self, input: &str, action: AtomicAction)` |
| `RuleLearner` | `learn_failure` | `fn learn_failure(&self, input: &str, action: AtomicAction)` |
| `RuleLearner` | `generate_rules` | `fn generate_rules(&self) -> Vec<KeywordRule>` (NOT SkillManifest!) |
| `RuleLearner` | `stats` | `fn stats(&self) -> LearnerStats` |
| `LearningAgent` | `on_l3_success` | `async fn on_l3_success(&self, input: &str, action: AtomicAction, latency: Duration)` |
| `LearningAgent` | `on_l3_failure` | `async fn on_l3_failure(&self, input: &str, action: AtomicAction, error: String)` |
| `LearningAgent` | `generate_and_deploy_rules` | `async fn generate_and_deploy_rules(&self) -> usize` |
| `SkillSystem` | `register_external` | `async fn register_external(&self, manifests: Vec<SkillManifest>)` (returns `()`) |
| `SkillManifest` | constructor | `pub fn new(id, name, description, content, source) -> Self` |
| `AtomicAction` | `from_tool_name` | `fn from_tool_name(name: &str) -> Option<AtomicAction>` (NEW — maps tool names to AtomicAction variants) |
| `AtomicAction` | `action_type` | `fn action_type(&self) -> &'static str` (EXISTING — returns "bash", "read", etc.) |

---

## Implementation Units (Corrected)

### Unit 0: Create L3 Callback Hook (LearningCallback)

**Goal:** Create a bridge between tool execution and LearningAgent.

**Actual findings:**
- `execute_tool_batch` uses `LoopCallback` but is ONLY called in tests (`orchestrator.rs:476,523,573`)
- Production tool execution is via `ToolPipeline.execute()` in `session/streaming.rs:276-293` — no callback mechanism
- `on_l3_success` is dead code — never called from production

**Approach:** Create a new `LearningCallback` type that bridges `LoopCallback` events to `LearningAgent`.

```rust
// src/engine/learning_agent.rs — NEW type
pub struct LearningCallback {
    learning_agent: Arc<LearningAgent>,
}

impl LearningCallback {
    pub fn new(learning_agent: Arc<LearningAgent>) -> Self {
        Self { learning_agent }
    }
}

impl LoopCallback for LearningCallback {
    fn on_tool_call_done(&mut self, event: &ToolCallEndEvent, result: &ToolResult) {
        // ToolResult::Success variant indicates successful execution
        let ToolResult::Success { output: _ } = result else { return; };
        let Some(action) = AtomicAction::from_tool_name(&event.tool_name) else { return; };
        let input = serde_json::from_value::<Value>(event.input.clone())
            .unwrap_or(Value::Null);
        let latency = Duration::from_millis(event.duration_ms as u64);
        // Fire and forget — don't block tool execution
        let agent = self.learning_agent.clone();
        tokio::spawn(async move {
            agent.on_l3_success(&input.to_string(), action, latency).await;
        });
    }
}
```

**Key insight:** `AtomicAction::from_tool_name(tool_name: &str)` doesn't exist yet — we need to create it. This converts tool names like "read_file" → `AtomicAction::Read`, "bash" → `AtomicAction::Bash`, etc.

**Implementation requires:**
1. Add `AtomicAction::from_tool_name(&str) -> Option<AtomicAction>` to `atomic_action.rs` — maps tool names to variants:
   - "bash" / "shell" → `AtomicAction::Bash`
   - "read_file" / "read" → `AtomicAction::Read`
   - "search" / "grep" / "search_files" → `AtomicAction::Search`
   - "write_file" / "write" → `AtomicAction::Write`
   - "edit_file" / "edit" → `AtomicAction::Edit`
   - "replace_in_file" / "replace" → `AtomicAction::Replace`
   - "move_file" / "move" → `AtomicAction::Move`
2. Create `LearningCallback` in `learning_agent.rs` with imports:
   ```rust
   use crate::harness::loop_callback::LoopCallback;
   use crate::harness::trace::{ToolCallEndEvent, ToolResult};
   use crate::tools::runtime::ToolResult;
   ```
3. Add `learning_callback: Option<Arc<Mutex<dyn LoopCallback>>>` field to `StreamingToolExecutor` in `session/streaming.rs` and call it after successful execution

**Files:**
- Modify: `src/engine/atomic_action.rs` — add `from_tool_name()`
- Modify: `src/engine/learning_agent.rs` — add `LearningCallback` type
- Modify: `src/session/streaming.rs` — add `learning_callback: Option<Arc<Mutex<dyn LoopCallback>>>` field to `StreamingToolExecutor` and call it after successful execution

**Verification:**
- `cargo check -p alephcore` compiles
- Integration: successful tool execution triggers `on_l3_success`

---

### Unit 1: Enable RuleLearner Module

**Goal:** Remove `#[allow(dead_code)]` from RuleLearner and LearningAgent entries in `engine/mod.rs`.

**Requirements:** R1

**Files:**
- Modify: `src/engine/mod.rs`

**Note:** Internal item-level `#[allow(dead_code)]` in `rule_learner.rs` and `learning_agent.rs` should be reviewed — some may need to stay.

**Verification:**
- `cargo check -p alephcore 2>&1 | grep -E "rule_learner|learning_agent" shows no dead_code warnings at module level`

---

### Unit 2: Add SkillSystem to LearningAgent

**Goal:** LearningAgent needs a `skill_system` field to register generated skills.

**Requirements:** R7

**Files:**
- Modify: `src/engine/learning_agent.rs` — add `skill_system: Arc<SkillSystem>` field

**Approach:**
LearningAgent currently has `learner: Arc<RuleLearner>` and `reflex_layer: Arc<RwLock<ReflexLayer>>`. Add `skill_system: Arc<SkillSystem>` to enable skill registration.

```rust
pub struct LearningAgent {
    learner: Arc<RuleLearner>,
    reflex_layer: Arc<RwLock<ReflexLayer>>,
    skill_system: Arc<SkillSystem>,  // NEW
    events: DashMap<String, Vec<LearningEvent>>,
    last_generation: Arc<RwLock<Instant>>,
    stats: Arc<RwLock<AgentStats>>,
}
```

**Constructor update:**
```rust
pub fn new(
    learner: Arc<RuleLearner>,
    reflex_layer: Arc<RwLock<ReflexLayer>>,
    skill_system: Arc<SkillSystem>,  // NEW
) -> Self
```

**Verification:**
- `cargo check -p alephcore` passes

---

### Unit 3: Implement PatternRecord → SkillManifest Conversion

**Goal:** Add `generate_skill_manifests()` method to RuleLearner.

**Requirements:** R5, R6

**Files:**
- Modify: `src/engine/rule_learner.rs` — add new method

**Approach:**
```rust
impl RuleLearner {
    /// Generate SkillManifest objects from high-confidence patterns.
    /// Returns manifests ready for SkillSystem::register_external().
    pub fn generate_skill_manifests(&self) -> Vec<SkillManifest> {
        self.records.iter()
            .filter(|entry| entry.value().is_ready())  // count >= MIN_EXECUTIONS && confidence >= MIN_CONFIDENCE
            .filter_map(|entry| self.pattern_to_skill(entry.value()))
            .collect()
    }

    fn pattern_to_skill(&self, record: &PatternRecord) -> Option<SkillManifest> {
        // Extract keywords → when_to_use regex
        let keywords = self.feature_extractor.extract(&record.pattern);
        let when_to_use = if keywords.is_empty() {
            None
        } else {
            Some(keywords.join("|"))
        };

        // Use existing action_type() method: AtomicAction::Bash { command } => "bash", etc.
        let action_type = record.action.action_type();
        let hash = self.short_hash(&record.pattern);
        let name = format!("learned_{}_{}", action_type, hash);
        let id = SkillId::new(uuid::Uuid::new_v4().to_string());

        // bound_tool uses action_type string directly
        let bound_tool = Some(action_type.to_string());

        // Build description
        let description = format!(
            "Learned from {} successful executions of: {}",
            record.successes, record.pattern
        );

        // Build content (embedded in manifest via SkillContent::new)
        let content = SkillContent::new(format!(
            "# {}\n\n{}\n\n## Pattern\n{}\n\n## Action\n```\n{:?}\n```",
            name, description, record.pattern, record.action
        ));

        let mut manifest = SkillManifest::new(
            id,
            name,
            description,
            content,
            SkillSource::Workspace,
        );

        // Set optional fields
        manifest.set_when_to_use(when_to_use);
        manifest.set_bound_tool(bound_tool);

        Some(manifest)
    }

    fn short_hash(&self, input: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        format!("{:x}", hasher.finish())[..8].to_string()
    }
}
```

**Note:** `SkillContent` is embedded inside `SkillManifest` via `SkillManifest::new()`. No separate tuple needed.

**Verification:**
- `cargo test -p alephcore rule_learner` passes

---

### Unit 4: Wire LearningAgent → SkillSystem Registration

**Goal:** After `generate_and_deploy_rules()`, also register any new skills.

**Requirements:** R8

**Files:**
- Modify: `src/engine/learning_agent.rs`

**Approach:**
In `generate_and_deploy_rules()`, after deploying L2 rules, also call `generate_skill_manifests()` and register each with `skill_system.register_external()`.

```rust
pub async fn generate_and_deploy_rules(&self) -> usize {
    // ... existing L2 rule deployment ...

    // NEW: Check for skills to register
    let skills = self.learner.generate_skill_manifests();
    for manifest in skills {
        let name = manifest.name().to_string();
        self.skill_system.register_external(vec![manifest]).await;
        tracing::info!("Registered learned skill: {}", name);
    }

    skills.len()
}
```

**Note:** `register_external` returns `()` (not `Result`), so errors are not recoverable here. The skill is either registered or not — log and continue.

**Verification:**
- `cargo test -p alephcore learning_agent` passes

---

### Unit 5: Wire LearningAgent to Tool Execution (IMPLEMENTED in Unit 0)

**Goal:** LearningAgent receives L3 success callbacks from the execution engine.

**Requirements:** R2, R3

**Status:** This is NOW IMPLEMENTED as part of Unit 0. The `LearningCallback` type bridges `LoopCallback::on_tool_call_done` → `LearningAgent::on_l3_success()`.

**Verification:**
- L3 success events trigger LearningAgent calls (integration test)

---

### Unit 6: Unit Tests

**Goal:** Cover PatternRecord accumulation and SkillManifest generation.

**Requirements:** R10

**Files:**
- Modify: `src/engine/rule_learner.rs` — add tests for `generate_skill_manifests()`

**Test scenarios:**
- 3 successes with same input → generates SkillManifest
- Mixed success/failure → confidence below threshold → no manifest
- High confidence pattern → manifest has correct name format, when_to_use, bound_tool

**Verification:**
- `cargo test -p alephcore rule_learner` — 100% relevant coverage

---

## System Architecture After Changes

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     L3 EXECUTION (Harness)                                │
│  AtomicAction::Bash/Read/Search/etc.                                    │
│         │                                                                │
│         │ on_tool_call_done callback                                     │
│         ▼                                                                │
│  LearningAgent::on_l3_success() ◄── NEW: wired callback                  │
│         │                                                                │
│         ▼                                                                │
│  RuleLearner::learn_success(input, action)                               │
│         │                                                                │
│         ├──► PatternRecord (DashMap)                                    │
│         │                                                                │
│         └──► [NEW] generate_skill_manifests()                             │
│                       │                                                  │
│                       ▼                                                  │
│                  (SkillManifest, SkillContent)                          │
│                       │                                                  │
│                       ▼                                                  │
│  SkillSystem::register_external(vec![manifest]) ◄── NEW: skill_system   │
│         │                                                                │
│         ▼                                                                │
│  SkillRegistry (skill appears in enumeration)                          │
└─────────────────────────────────────────────────────────────────────────┘

Simultaneously:
┌─────────────────────────────────────────────────────────────────────────┐
│  LearningAgent::generate_and_deploy_rules()                             │
│         │                                                                │
│         ▼                                                                │
│  RuleLearner::generate_rules() → Vec<KeywordRule>                      │
│         │                                                                │
│         ▼                                                                │
│  ReflexLayer L2 routing (existing dead code path, now active)          │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Verification

| Unit | Check |
|------|-------|
| Unit 0 | `cargo check -p alephcore` compiles with `LearningCallback` and `AtomicAction::from_tool_name` |
| Unit 1 | `cargo check -p alephcore` no dead_code warnings |
| Unit 2 | `cargo check -p alephcore` compiles with SkillSystem field |
| Unit 3 | `cargo test -p alephcore rule_learner` passes |
| Unit 4 | `cargo test -p alephcore learning_agent` passes |
| Unit 5 | Integrated via Unit 0 |
| Unit 6 | Coverage report shows `generate_skill_manifests` tested |

---

## Dependencies Between Units

```
Unit 0 (LearningCallback + from_tool_name) ─────────────────────┐
                                                               │
Unit 1 (enable modules) ──────────────────────────────────────►│
                                                               │ Unit 3-6
Unit 2 (add SkillSystem) ────────────────────────────────────►│ depend on
                                                               │ 1 & 2
Unit 3 (generate manifests) ─────────────────────────────────►│
                                                               │
Unit 4 (register skills) ────────────────────────────────────►│
                                                               │
Unit 6 (tests) ─────────────────────────────────────────────►│
```

---

## Sources & References

- Actual RuleLearner: `src/engine/rule_learner.rs`
- Actual LearningAgent: `src/engine/learning_agent.rs`
- Actual SkillManifest: `src/domain/skill.rs`
- Actual SkillSystem: `src/skill/mod.rs`
- DreamDaemon stub: `src/memory/dreaming/mod.rs` lines 594-605
- HarnessCallback: `src/harness/callback.rs`
- ReflexLayer: `src/engine/reflex_layer.rs`
