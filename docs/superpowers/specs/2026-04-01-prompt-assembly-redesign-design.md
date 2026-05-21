# Prompt Assembly Redesign — Section Registry + Cache Partitioning

**Date:** 2026-04-01
**Status:** Draft
**Scope:** `src/agent_loop/prompt_builder.rs` rewrite + `src/context/` new module

---

## Problem

Aleph's Agent Loop `PromptBuilder` is a simple linear assembler that lacks critical capabilities compared to both Claude Code's `prompts.ts` and Aleph's own Thinker `PromptPipeline`:

1. **No cache boundary** — no stable/dynamic partitioning, wasting LLM provider cache
2. **No environment info** — LLM doesn't know OS, CWD, git status, date
3. **No memory integration** — parameter exists but always passed as `None`
4. **No behavioral discipline** — missing engineering constraints (don't over-abstract, blast radius thinking, tool usage grammar)
5. **No token budget control** — no degradation strategy for oversized prompts
6. **No session-specific guidance** — no dynamic rules based on current tool/capability set
7. **Monolithic BASE_BEHAVIOR** — 82-line hardcoded constant mixing 7+ concerns

## Approach

**Option C: Unified base, independent entry points.** Extract shared context modules that both Agent Loop and Thinker can consume. Redesign PromptBuilder as a Section Registry with Cache Partitioning. Only modify Agent Loop side; Thinker remains unchanged for now.

### Alternatives Considered

- **A: Section Functions (Claude Code direct port)** — Simple but not extensible. Adding sections requires modifying `build()`. No budget control.
- **B: Mini-Layer System (Thinker lite)** — Over-engineered for Agent Loop. Unnecessary trait object dispatch.
- **C: Section Registry + Cache Partitioning (chosen)** — Best balance of extensibility, cache economics, and simplicity.

## Architecture

### Module Layout

```
src/
├── context/                          # NEW: shared data source module
│   ├── mod.rs
│   ├── environment.rs                # OS/platform/CWD/git detection
│   ├── memory_context.rs             # Memory context retrieval (reuses MemoryContextProvider)
│   └── session_info.rs               # Session info (date, model, capabilities)
│
├── agent_loop/
│   ├── prompt_builder.rs             # REWRITE: Section Registry + Cache Partitioning
│   ├── prompt_sections/              # NEW: independent section renderers
│   │   ├── mod.rs
│   │   ├── identity.rs               # Soul identity + default
│   │   ├── system_rules.rs           # Runtime reality (permissions, tags, hooks, context limits)
│   │   ├── doing_tasks.rs            # Engineering discipline (ref: Claude Code getSimpleDoingTasksSection)
│   │   ├── actions.rs                # Blast radius / risk action rules
│   │   ├── tool_usage.rs             # Tool usage grammar (dedicated tools over Bash)
│   │   ├── tone_and_style.rs         # Output style/emoji/reference format
│   │   ├── output_efficiency.rs      # Conciseness guidance
│   │   ├── environment.rs            # Environment info (consumes context::environment)
│   │   ├── session_guidance.rs       # Dynamic rules based on current tool set
│   │   ├── memory.rs                 # Memory context (consumes context::memory_context)
│   │   ├── skills.rs                 # Skill listing + invocation guidance
│   │   ├── tools.rs                  # Available tools listing
│   │   ├── model_behavior.rs         # LLM family-specific behavior
│   │   ├── custom_instructions.rs    # User custom instructions
│   │   └── memory_protocol.rs        # Memory save/search/extract protocol
│   └── ...existing files...
```

### Dependency Direction

```
agent_loop/prompt_builder ──► context/environment
                           ──► context/memory_context
                           ──► context/session_info

thinker/layers/* ──► (unchanged, can adopt context/ later)
```

`context/` depends on neither `agent_loop/` nor `thinker/`. Both can consume it independently.

## Core Data Structures

### PromptSection

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// Session-stable, cacheable by LLM provider
    Stable,
    /// May change per turn, not cacheable
    Dynamic,
}

#[derive(Debug, Clone)]
pub struct PromptSection {
    pub name: &'static str,
    pub stability: Stability,
    pub priority: u32,       // Lower = higher importance, rendered first
    pub protected: bool,     // true = never removed by budget enforcement
    pub content: String,
}
```

### PromptBudget

```rust
#[derive(Debug, Clone)]
pub struct PromptBudget {
    pub max_chars: usize,    // ~4 chars ≈ 1 token; default 80,000 (~20K tokens)
}
```

### PromptResult

```rust
pub struct PromptResult {
    pub prompt: String,
    pub cache_boundary_offset: usize,      // Byte offset where Dynamic zone starts
    pub truncated_sections: Vec<&'static str>, // Sections removed by budget enforcement
}
```

**Design decisions:**
- `PromptSection` is a plain data struct, not a trait object — rendering happens before registration
- `cache_boundary_offset` is a byte offset, not a text marker — provider bridge decides how to mark it (Anthropic: `cache_control`, OpenAI: transparent, Gemini: `cachedContent`)
- `PromptBudget` uses char count, not token count — avoids tokenizer dependency

## PromptBuilder API

```rust
pub struct PromptBuilder {
    sections: Vec<PromptSection>,
    budget: PromptBudget,
}

impl PromptBuilder {
    pub fn new() -> Self;
    pub fn register(&mut self, section: PromptSection) -> &mut Self;  // Overwrites by name
    pub fn register_all(&mut self, sections: Vec<PromptSection>) -> &mut Self;
    pub fn remove(&mut self, name: &str) -> &mut Self;
    pub fn with_budget(mut self, budget: PromptBudget) -> Self;
    pub fn build(&self) -> PromptResult;
    pub fn build_stable_only(&self) -> String;

    // Convenience constructors (internally call register)
    pub fn with_soul(mut self, soul: &SoulManifest) -> Self;
    pub fn with_environment(mut self, env: &EnvironmentInfo) -> Self;
    pub fn with_memory(mut self, ctx: &MemoryContext) -> Self;
    pub fn with_tools(mut self, tools: &[ToolInfo]) -> Self;
    pub fn with_skills(mut self, skills: &[SkillManifest], active_tools: &[&str]) -> Self;
    pub fn with_default_behavior_sections(mut self) -> Self;
    pub fn with_session_guidance(mut self, tools: &[ToolInfo]) -> Self;
}
```

**Key behaviors:**
- `register()` overwrites existing section with same name
- `build()` takes `&self`, not `self` — supports repeated rebuilds within the loop
- `with_default_behavior_sections()` registers all 6 behavioral sections at once: `system_rules`, `doing_tasks`, `actions`, `tool_usage`, `tone_and_style`, `output_efficiency`

## Section Catalog

### Stable Zone (cacheable within session)

| Priority | Name | Protected | Source | Content |
|----------|------|-----------|--------|---------|
| 50 | `identity` | Yes | SoulManifest | "You are Aleph, ..." identity declaration |
| 100 | `tone` | No | SoulManifest.voice | Communication style/tone |
| 150 | `directives` | No | SoulManifest.directives + anti_patterns | Behavioral directive bullet list |
| 200 | `model_behavior` | No | model_behaviors/ files | LLM family-specific behavior |
| 300 | `system_rules` | Yes | New, ref Claude Code `getSimpleSystemSection` | Runtime reality: permissions, tags, hooks, context compression |
| 400 | `doing_tasks` | Yes | New, ref Claude Code `getSimpleDoingTasksSection` | Engineering discipline: no over-abstraction, read before modify, no unnecessary refactoring, security awareness |
| 500 | `actions` | No | New, ref Claude Code `getActionsSection` | Blast radius thinking: confirm risky actions, don't use destructive ops as shortcuts |
| 600 | `tool_usage` | Yes | New, ref Claude Code `getUsingYourToolsSection` | Tool usage grammar: dedicated tools over Bash, parallel calls |
| 700 | `tone_and_style` | No | New, ref Claude Code `getSimpleToneAndStyleSection` | Emoji policy, code ref format, conciseness |
| 800 | `output_efficiency` | No | New, ref Claude Code `getOutputEfficiencySection` | Lead with answer, skip filler, short direct sentences |
| 900 | `tools` | Yes | ToolInfo list | Available tools with descriptions |
| 1000 | `skills` | No | SkillManifest list | Skill listing + invocation guidance |
| 1100 | `memory_protocol` | No | Extracted from BASE_BEHAVIOR | When to save/search/extract memory |
| 1200 | `custom_instructions` | No | SoulManifest.addendum / user config | User-defined instructions |

### Dynamic Zone (may change per turn)

| Priority | Name | Protected | Source | Content |
|----------|------|-----------|--------|---------|
| 1500 | `session_guidance` | No | Dynamic from tool set | Agent tool usage, skill triggers, verification requirements |
| 1600 | `environment` | Yes | `context::environment` | OS, CWD, git status, date, model info |
| 1700 | `memory` | No | `context::memory_context` | Semantically retrieved facts and past conversations |
| 1800 | `discovered_skills` | No | Async prefetch | Runtime-discovered skills |

### Budget Enforcement Order

When prompt exceeds `max_chars`, remove non-protected sections from highest priority number first:

```
discovered_skills(1800) → memory(1700) → session_guidance(1500) →
custom_instructions(1200) → memory_protocol(1100) → skills(1000) →
output_efficiency(800) → tone_and_style(700) → actions(500) →
model_behavior(200) → directives(150) → tone(100)
```

**Never removed:** `identity`(50), `system_rules`(300), `doing_tasks`(400), `tool_usage`(600), `tools`(900), `environment`(1600)

## Shared Context Module

### EnvironmentInfo

```rust
pub struct EnvironmentInfo {
    pub cwd: String,
    pub is_git: bool,
    pub git_branch: Option<String>,
    pub os: String,
    pub os_version: String,
    pub shell: String,
    pub date: String,
    pub model_name: Option<String>,
    pub knowledge_cutoff: Option<String>,
}

impl EnvironmentInfo {
    pub async fn detect() -> Self;   // Async for git operations
    pub fn for_test() -> Self;       // Test constructor
}
```

### MemoryContext

```rust
pub struct MemoryContext {
    pub facts: Vec<MemoryFact>,
    pub past_conversations: Vec<ConversationSnippet>,
}

impl MemoryContext {
    /// Reuses logic from existing crate::memory::store MemoryStore trait.
    /// Does NOT rewrite LanceDB queries — wraps MemoryContextProvider::fetch().
    pub async fn fetch(query: &str, store: &crate::memory::store::MemoryStore) -> Self;
    pub fn is_empty(&self) -> bool;
}
```

### SessionInfo

```rust
pub struct SessionInfo {
    pub session_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub capabilities: Vec<String>,
}
```

## Section Rendering Pattern

Each `prompt_sections/*.rs` exports a pure function:

```rust
pub fn render(/* structured data */) -> PromptSection {
    PromptSection {
        name: "section_name",
        stability: Stability::Stable, // or Dynamic
        priority: N,
        protected: bool,
        content: format!("# Section Title\n\n{}", formatted_content),
    }
}
```

- Input: structured data (not raw strings)
- Output: complete `PromptSection` with metadata
- Empty content → `build()` skips automatically
- `# Header` controlled by each section, not by builder
- Content for behavioral sections (`doing_tasks`, `actions`, `tool_usage`, `tone_and_style`, `output_efficiency`, `system_rules`) sourced from Claude Code `prompts.ts` at `/Volumes/TBU/Github/claude-code/src/constants/prompts.ts`, adapted to Aleph context:
  - Replace Claude Code tool names with Aleph equivalents (e.g. `FileRead` → Aleph's tool names)
  - Remove Claude Code-specific rules (git commit format, PR creation, coauthorship)
  - Add Aleph-specific rules where needed (e.g. Aleph's skill invocation, memory protocol)
  - Preserve the behavioral discipline intent — the "what" stays, the "how" adapts

## Integration Changes

### Call Chain — Before

```
LoopFactory::build()
  → PromptBuilder::from_soul(soul)
  → AgentLoop::new(bridge, registry, prompt_builder, ...)

loop_core::run_with_history_messages()
  → system_prompt = prompt_builder.build(&tool_infos, None)
```

### Call Chain — After

```
LoopFactory::build()  [now async — NOTE: callers must be updated]
  // LoopFactory::build() becomes async because EnvironmentInfo::detect() and
  // MemoryContext::fetch() are async. Existing callers:
  //   - LoopFactory::build_from_server() — already async, trivial change
  //   - gateway/execution_engine/run_loop.rs — already in async context
  //   - Any test code using LoopFactory::build() — wrap in tokio::test
  → env_info = EnvironmentInfo::detect().await
  → memory_ctx = MemoryContext::fetch(query, store).await
  → prompt_builder = PromptBuilder::new()
      .with_soul(&soul)
      .with_default_behavior_sections()
      .with_environment(&env_info)
      .with_budget(PromptBudget::default())
  → AgentLoop::new(bridge, registry, prompt_builder, ...)

loop_core::run_with_history_messages()
  → prompt_builder.register(tools::render(&tool_infos))
  → prompt_builder.register(skills::render(&skills, &tool_names))
  → prompt_builder.register(session_guidance::render(&tool_names))
  → prompt_builder.register(memory::render(&memory_ctx))
  → result = prompt_builder.build()
  → system_prompt = result.prompt
  → cache_offset = result.cache_boundary_offset
```

### Cache Boundary Propagation

`PromptResult.cache_boundary_offset` propagates to `RequestPayload`:

```rust
pub struct RequestPayload<'a> {
    pub system_prompt: &'a str,
    pub cache_boundary_offset: Option<usize>,  // NEW
    // ...existing fields
}
```

Provider implementations use existing `cache.rs` infrastructure to apply provider-specific caching.

## Files Changed

| File | Change | Description |
|------|--------|-------------|
| `src/context/mod.rs` | **New** | Module declaration |
| `src/context/environment.rs` | **New** | Environment detection |
| `src/context/memory_context.rs` | **New** | Memory context retrieval |
| `src/context/session_info.rs` | **New** | Session info |
| `src/agent_loop/prompt_builder.rs` | **Rewrite** | Section Registry + Cache Partitioning |
| `src/agent_loop/prompt_sections/*.rs` | **New** | 15 section renderers |
| `src/agent_loop/factory.rs` | **Modify** | Wire new builder API, collect env/memory |
| `src/agent_loop/loop_core.rs` | **Modify** | Use register + build instead of old build call |
| `src/agent_loop/mod.rs` | **Modify** | Export new modules |
| `src/lib.rs` | **Modify** | Register `context` module |

## Code to Delete

| Location | Content | Reason |
|----------|---------|--------|
| `prompt_builder.rs:29-82` | `BASE_BEHAVIOR` constant | Split into 6+ independent sections |
| `prompt_builder.rs:25` | `DEFAULT_IDENTITY` constant | Moved to `prompt_sections/identity.rs` |
| `prompt_builder.rs:86-96` | Old `PromptBuilder` struct fields | Replaced by `sections: Vec<PromptSection>` |
| `prompt_builder.rs:220-318` | Old `build()` method | Replaced by Section Registry version |
| `prompt_builder.rs:200-214` | `update_skill_info()` + Mutex | Replaced by `register()` |

## Code NOT Changed

| Location | Reason |
|----------|--------|
| `chain_context.rs` | Unrelated to prompt assembly |
| `context_compactor.rs` | Independent concern (context compression) |
| `context_budget.rs` | In-loop token budget, different from prompt budget |
| `safety.rs` | Runtime safety guard |
| `provider_bridge.rs` | Consumes `cache_boundary_offset` but interface stable |
| All Thinker code | Out of scope for this iteration |

## Testing Strategy

- **Migrated tests:** All 13 existing `prompt_builder.rs` tests adapted to new API
- **Section unit tests:** Each `prompt_sections/*.rs` has independent tests
- **Context unit tests:** `EnvironmentInfo::for_test()`, `MemoryContext::default()`
- **Integration test:** `PromptBuilder::new().with_default_behavior_sections().build()` → verify full output structure, stable/dynamic partitioning, budget enforcement
- **Cache boundary test:** Verify `cache_boundary_offset` points to correct position between stable and dynamic zones
- **Budget enforcement test:** Register sections exceeding budget, verify correct truncation order

## Non-Goals

- Modifying Thinker's PromptPipeline (future iteration)
- Adding new tools or capabilities
- Changing provider bridge interfaces beyond `cache_boundary_offset`
- Prompt content optimization (content comes from Claude Code reference; Aleph-specific tuning is a separate task)
