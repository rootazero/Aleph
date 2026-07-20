# Prompt System Unification & Enhancement (Round 2)

**Date**: 2026-04-06
**Status**: Approved
**Scope**: Prompt system architecture — unify to PromptPipeline, add Claude Code-inspired capabilities, clean up legacy code

---

## Background

Aleph currently has two parallel prompt systems:

- **PromptBuilder** (old): `src/agent_loop/prompt_builder.rs` + `src/agent_loop/prompt_sections/*.rs` — Section Registry pattern, ~900 lines, used by `loop_core.rs` and `subagent_runner.rs`
- **PromptPipeline** (new): `src/thinker/prompt_pipeline.rs` + `src/thinker/layers/*.rs` — Layer trait pattern, 27 layers, supports AssemblyPath/PromptMode/TokenBudget

The new system is architecturally superior (mode filtering, assembly paths, per-layer stability, structured budget enforcement) but the old system is still actively used by `agent_loop`. This creates maintenance burden and prevents unified optimization.

Additionally, studying Claude Code's prompt architecture reveals capabilities Aleph lacks:
1. **Tool Usage Priority Grammar** — behavioral encoding of "prefer tool X over Y for task Z"
2. **Section-level caching** — avoid recomputing stable layers each turn
3. **Hybrid memory injection** — structured index + vector retrieval (Aleph's LanceDB advantage)

## Decisions

| # | Decision | Choice |
|---|----------|--------|
| 1 | Dual system unification | Unify to PromptPipeline |
| 2 | Tool usage grammar | Introduce as new Layer |
| 3 | Cache optimization | Section caching now, Delta mechanism later |
| 4 | Memory upgrade | Hybrid retrieval (structured + vector) |
| 5 | Legacy cleanup | Full delete, no deprecated stubs |

## Non-Goals

- Delta attachment mechanism for MCP/tool list changes (deferred)
- Prompt version management / A/B testing framework
- Profile-based prompt override (separate effort)

---

## Phase 1: Unify to PromptPipeline + Delete Legacy

### Goal

Switch all `agent_loop` call sites from old `PromptBuilder` to `thinker::PromptPipeline`, then delete the old system entirely.

### Call Sites to Migrate

| File | Current Usage | Migration |
|------|--------------|-----------|
| `src/agent_loop/loop_core.rs` | `PromptBuilder::new().with_default_behavior_sections().with_soul().build()` | `thinker::PromptBuilder::new(config).build_system_prompt_with_full_context(...)` |
| `src/agent_loop/subagent_runner.rs` | `PromptBuilder::for_agent(&agent_def)` | New `thinker::PromptBuilder::build_for_agent()` method |

### New Components

#### AgentRoleLayer (priority 55)

Replaces the old `resolve(name)` mechanism for agent-specific sections.

```rust
pub struct AgentRoleLayer;

impl PromptLayer for AgentRoleLayer {
    fn name(&self) -> &'static str { "agent_role" }
    fn priority(&self) -> u32 { 55 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Soul, AssemblyPath::Context]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let agent = match input.agent_def {
            Some(a) => a,
            None => return,  // not a sub-agent context
        };
        // Read agent_def.prompt_sections to determine which constraints to inject.
        // Migration mapping from old resolve(name):
        //   "explore_constraints"  → read-only exploration rules (no file writes)
        //   "coder_guidelines"     → coding best practices and style
        //   "researcher_protocol"  → research-focused behavior
        //   "verify_protocol"      → adversarial verification contract
        //   "plan_protocol"        → planning-only, no implementation
        //
        // Each maps to a static &str block within this layer.
        // AgentDef.prompt_sections: Vec<String> drives which blocks are included.
        for section_name in &agent.prompt_sections {
            if let Some(block) = Self::resolve_section(section_name) {
                output.push_str(block);
                output.push_str("\n\n");
            }
        }
    }
}
```

#### LayerInput Extension

```rust
pub struct LayerInput<'a> {
    // ... existing fields
    pub agent_def: Option<&'a AgentDef>,  // NEW
}
```

#### PromptBuilder::build_for_agent()

```rust
impl PromptBuilder {
    pub fn build_for_agent(
        &self,
        agent_def: &AgentDef,
        tools: &[ToolInfo],
        soul: &SoulManifest,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_agent_def(agent_def);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }
}
```

### Files to Delete

| File | Lines | Reason |
|------|-------|--------|
| `src/agent_loop/prompt_builder.rs` | ~776 | Replaced by thinker::PromptBuilder |
| `src/agent_loop/prompt_sections/mod.rs` | ~68 | Section router no longer needed |
| `src/agent_loop/prompt_sections/*.rs` | ~150+ | All 15+ section files |

### Verification

- Snapshot current old-system output for representative inputs
- Generate new-system output for same inputs, diff for semantic equivalence
- All `cargo test -p alephcore` must pass after migration

---

## Phase 2: Claude Code-Inspired Enhancements

### 2.1 ToolUsageGrammarLayer (priority 550)

Encodes tool usage conventions into the prompt — "prefer tool X over alternative Y for task Z".

**Data-driven design** (not hardcoded templates):

```rust
pub struct ToolUsageHint {
    /// Scenarios where this tool should be preferred
    pub prefer_for: Vec<String>,
    /// Alternatives this tool supersedes
    pub prefer_over: Vec<String>,
}

// Extension to existing ToolInfo
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters_schema: Option<serde_json::Value>,
    pub usage_hint: Option<ToolUsageHint>,  // NEW
}
```

The layer reads `usage_hint` from registered tools and generates guidelines:

```
## Tool Usage Guidelines
- To read files, use `file_read` instead of shell `cat/head/tail`
- To search code, use `code_search` instead of shell `grep/rg`
- Prefer parallel tool calls when tasks are independent
```

**Design rationale**: Grammar is driven by tool registration metadata, not prompt templates. Aligns with R9 (Everything is a Tool) — tools self-describe their usage conventions.

### 2.2 Section-Level Caching

Add output caching to `PromptPipeline` for Stable layers:

```rust
pub struct PromptPipeline {
    layers: Vec<Box<dyn PromptLayer>>,
    cache: RwLock<HashMap<&'static str, CachedSection>>,  // NEW
}

struct CachedSection {
    content: String,
    generation: u64,
}

impl PromptPipeline {
    /// Invalidate a specific layer's cache
    pub fn invalidate(&self, layer_name: &str);

    /// Invalidate all cached sections
    pub fn invalidate_all(&self);

    /// Cache hit/miss statistics
    pub fn cache_stats(&self) -> CacheStats;
}
```

**Caching rules**:
- `LayerStability::Stable` → cache, reuse across requests
- `LayerStability::Dynamic` → always recompute
- Invalidation triggers: tool list change → `"tools"`, soul change → `"soul"`, session reset → all

**Benefit**: ~20 of 27 layers are Stable. After first request, only ~7 Dynamic layers recompute per turn.

### 2.3 Hybrid Memory Injection

Upgrade `MemoryAugmentationLayer` (priority 1740) to dual-path injection:

**Path 1: Structured Index**
- Load `.aleph/MEMORY.md` from workspace (if exists)
- Truncate to 200 lines / 25KB
- Provide memory taxonomy guidance (user/project/feedback/reference categories)

**Path 2: Vector Retrieval** (Aleph's LanceDB advantage)
- Semantic search against current user input
- Top-K relevant memory fragments
- Each fragment annotated with source and relevance score

**Injection format**:
```
## Memory Context

### Index (structured)
[truncated MEMORY.md content or facts index]

### Relevant Memories (semantic)
- [0.92] [2026-03-15] User prefers concise responses...
- [0.87] [2026-04-01] Project uses CalVer versioning...

### Memory Guidelines
[taxonomy + when to save/access behavioral guidance]
```

**Token budget**: Total memory injection ≤ `max_per_file_chars` (default 20,000 chars). Structured index gets 50%, vector retrieval gets 50%.

---

## Phase 3: Cleanup & Integration Verification

### Dead Code Cleanup

| Target | Action |
|--------|--------|
| `agent_loop/mod.rs` mod declarations for `prompt_builder`/`prompt_sections` | Delete |
| `use crate::agent_loop::prompt_builder::*` across codebase | Delete or redirect to `thinker` |
| `PromptSection`, `Stability` type references | Replace with new system types |
| Old system test files | Delete (migrate assertions to new system tests) |

### Type Unification

After cleanup, `thinker::PromptBuilder` is the sole prompt builder. If `agent_loop` needs prompt access, it imports from `thinker`.

### Verification Checklist

| Item | Method |
|------|--------|
| Main loop prompt generation correct | `cargo test -p alephcore --lib` prompt tests |
| SubAgent prompt generation correct | subagent integration tests |
| Cache boundary calculation correct | Unit test: Stable zone end = boundary offset |
| Section cache hit rate | `pipeline.cache_stats()` logged |
| Hybrid memory injection format | Unit test: both paths inject |
| Tool grammar dynamic generation | Unit test: tools with `usage_hint` produce correct output |
| Token budget enforcement | Property test: random inputs → prompt length ≤ budget |
| PromptMode filtering correct | Compact/Minimal mode layer count assertions |

### Documentation

- Update `ARCHITECTURE.md` prompt system section to reflect unified architecture
- Add rustdoc to `PromptPipeline` and key Layers (core API only)

---

## Final Architecture

```
PromptPipeline (sole entry point)
  │
  ├─ Stable Zone (cached across requests)
  │   ├─ [50]   SoulLayer
  │   ├─ [55]   AgentRoleLayer (NEW)
  │   ├─ [75]   ProfileLayer
  │   ├─ [100]  RoleLayer
  │   ├─ [300]  EnvironmentLayer
  │   ├─ [400]  RuntimeCapabilitiesLayer
  │   ├─ [500]  ToolsLayer
  │   ├─ [501]  HydratedToolsLayer
  │   ├─ [550]  ToolUsageGrammarLayer (NEW)
  │   ├─ [600]  SecurityLayer
  │   ├─ [700]  ProtocolTokensLayer
  │   ├─ [710]  HeartbeatLayer
  │   ├─ [800]  OperationalGuidelinesLayer
  │   ├─ [900]  CitationStandardsLayer
  │   ├─ [1000] GenerationModelsLayer
  │   ├─ [1050] SkillInstructionsLayer
  │   ├─ [1100] SpecialActionsLayer
  │   ├─ [1200] ResponseFormatLayer
  │   ├─ [1300] GuidelinesLayer
  │   ├─ [1350] ThinkingGuidanceLayer
  │   ├─ [1400] SkillModeLayer
  │   ├─ [1500] CustomInstructionsLayer
  │   └─ [1600] LanguageLayer
  │
  ├─ ─ ─ [CACHE BOUNDARY] ─ ─ ─
  │
  └─ Dynamic Zone (recomputed per request)
      ├─ [1700] InboundContextLayer
      ├─ [1710] VoiceModeLayer
      ├─ [1720] RuntimeContextLayer
      ├─ [1730] IdentityFilesLayer
      ├─ [1740] MemoryAugmentationLayer (ENHANCED: hybrid retrieval)
      └─ [1750] SessionContextGuideLayer
```

### New Components Summary

| Component | Phase | Type |
|-----------|-------|------|
| `AgentRoleLayer` | 1 | Layer (priority 55) |
| `LayerInput.agent_def` | 1 | Field |
| `PromptBuilder::build_for_agent()` | 1 | Method |
| `ToolUsageGrammarLayer` | 2 | Layer (priority 550) |
| `ToolInfo.usage_hint` | 2 | Field |
| `PromptPipeline.cache` | 2 | Field + methods |
| `MemoryAugmentationLayer` enhancement | 2 | Modification |
| `pipeline.cache_stats()` | 3 | Method |

### Deleted Components Summary

| Component | Phase | Lines |
|-----------|-------|-------|
| `agent_loop/prompt_builder.rs` | 1 | ~776 |
| `agent_loop/prompt_sections/*.rs` | 1 | ~220+ |
| Residual type references | 3 | scattered |
