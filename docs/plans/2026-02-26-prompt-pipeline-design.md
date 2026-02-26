# PromptPipeline Design — Trait-per-Layer Standard Pipeline

**Date**: 2026-02-26
**Status**: Approved
**Scope**: Refactor `prompt_builder.rs` (1724 lines) into a composable PromptPipeline with 20 independent Layers

---

## Background

Plan A (Feb 9, 2026) added 4 new prompt sections (RuntimeContext, ProtocolTokens, OperationalGuidelines, CitationStandards) inline into `prompt_builder.rs`, growing it to 1724 lines. Plan B was deferred as a future evolution direction. This design implements Plan B: the PromptLayer pipeline.

### Goals

1. **Solve bloat** — split `prompt_builder.rs` from 1724 → ~350 lines
2. **Each section independent** — one Layer per file, 10-80 lines each
3. **Backward compatible** — 4 public build entry points unchanged, zero caller changes
4. **Easy to extend** — adding a future section = adding one Layer file + registering it

### Non-Goals

- Unifying the 4 build paths into a single entry point (future work)
- Dynamic runtime Layer registration (current static registration is sufficient)
- Macro-based auto-registration (overkill for 20 layers)

---

## Core Abstractions

### AssemblyPath

```rust
/// Which build paths a layer participates in
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssemblyPath {
    Basic,      // build_system_prompt()
    Hydration,  // build_system_prompt_with_hydration()
    Soul,       // build_system_prompt_with_soul()
    Context,    // build_system_prompt_with_context()
    Cached,     // build_system_prompt_cached() — dynamic part only
}
```

### LayerInput

```rust
/// All possible inputs a layer might need, gated by Option
pub struct LayerInput<'a> {
    pub config: &'a PromptConfig,
    pub tools: Option<&'a [ToolInfo]>,
    pub hydration: Option<&'a HydrationResult>,
    pub soul: Option<&'a SoulManifest>,
    pub context: Option<&'a ResolvedContext>,
}

impl<'a> LayerInput<'a> {
    pub fn basic(config: &'a PromptConfig, tools: &'a [ToolInfo]) -> Self;
    pub fn hydration(config: &'a PromptConfig, hydration: &'a HydrationResult) -> Self;
    pub fn soul(config: &'a PromptConfig, tools: &'a [ToolInfo], soul: &'a SoulManifest) -> Self;
    pub fn context(config: &'a PromptConfig, ctx: &'a ResolvedContext) -> Self;
}
```

### PromptLayer Trait

```rust
/// A single composable prompt section
pub trait PromptLayer: Send + Sync {
    /// Human-readable name for debugging/testing
    fn name(&self) -> &'static str;

    /// Sort order — lower number = appears earlier in prompt
    fn priority(&self) -> u32;

    /// Which assembly paths this layer participates in
    fn paths(&self) -> &'static [AssemblyPath];

    /// Write this layer's content into the output
    fn inject(&self, output: &mut String, input: &LayerInput);
}
```

### PromptPipeline

```rust
pub struct PromptPipeline {
    layers: Vec<Box<dyn PromptLayer>>,  // pre-sorted by priority at construction
}

impl PromptPipeline {
    pub fn new(mut layers: Vec<Box<dyn PromptLayer>>) -> Self {
        layers.sort_by_key(|l| l.priority());
        Self { layers }
    }

    pub fn execute(&self, path: AssemblyPath, input: &LayerInput) -> String {
        let mut output = String::with_capacity(16384);
        for layer in &self.layers {
            if layer.paths().contains(&path) {
                layer.inject(&mut output, input);
            }
        }
        output
    }

    pub fn default_layers(config: &PromptConfig) -> Self {
        Self::new(vec![
            Box::new(SoulLayer),
            Box::new(RoleLayer),
            // ... all 20 layers
        ])
    }
}
```

---

## Layer Inventory

| # | Layer | Priority | Basic | Hydration | Soul | Context | Cached | Gate |
|---|-------|----------|:-----:|:---------:|:----:|:-------:|:------:|------|
| 1 | `SoulLayer` | 50 | | | Y | | | `soul.is_empty()` |
| 2 | `RoleLayer` | 100 | Y | Y | Y | Y | | always |
| 3 | `RuntimeContextLayer` | 200 | | | | Y | | `runtime_context.is_some()` |
| 4 | `EnvironmentLayer` | 300 | | | | Y | | always |
| 5 | `RuntimeCapabilitiesLayer` | 400 | Y | Y | Y | Y | Y | `config.runtime_capabilities.is_some()` |
| 6 | `ToolsLayer` | 500 | Y | | Y | Y | Y | always (may emit empty message) |
| 7 | `HydratedToolsLayer` | 500 | | Y | | | | always (Hydration exclusive) |
| 8 | `SecurityLayer` | 600 | | | | Y | | has disabled_tools or security_notes |
| 9 | `ProtocolTokensLayer` | 700 | | | | Y | | SilentReply capability |
| 10 | `OperationalGuidelinesLayer` | 800 | | | | Y | | Background/CLI paradigm |
| 11 | `CitationStandardsLayer` | 900 | | | Y | Y | | always |
| 12 | `GenerationModelsLayer` | 1000 | Y | Y | Y | Y | Y | `config.generation_models.is_some()` |
| 13 | `SkillInstructionsLayer` | 1050 | Y | Y | | | | `config.skill_instructions.is_some()` |
| 14 | `SpecialActionsLayer` | 1100 | Y | Y | Y | Y | Y | always |
| 15 | `ResponseFormatLayer` | 1200 | Y | Y | Y | Y | Y | always |
| 16 | `GuidelinesLayer` | 1300 | Y | Y | Y | Y | Y | always |
| 17 | `ThinkingGuidanceLayer` | 1350 | Y | Y | Y | | | `config.thinking_transparency` |
| 18 | `SkillModeLayer` | 1400 | Y | Y | Y | Y | Y | `config.skill_mode` |
| 19 | `CustomInstructionsLayer` | 1500 | Y | Y | Y | Y | Y | `config.custom_instructions.is_some()` |
| 20 | `LanguageLayer` | 1600 | Y | Y | Y | Y | Y | `config.language.is_some()` |

### Cached Path Special Handling

`build_system_prompt_cached` returns `Vec<SystemPromptPart>` (not `String`). The static header (~15 lines) is hardcoded outside the pipeline. The dynamic part uses `pipeline.execute(Cached, input)`.

### ToolsLayer vs HydratedToolsLayer

Same priority (500), mutually exclusive paths. Both live in `layers/tools.rs`. Context path's ToolsLayer reads tools from `input.context.unwrap().available_tools`.

### Condition Gates

All gate logic is inside `inject()` (early return), not at Pipeline level:

```rust
impl PromptLayer for ProtocolTokensLayer {
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context { Some(c) => c, None => return };
        if !ctx.environment_contract.active_capabilities.contains(&Capability::SilentReply) {
            return;
        }
        output.push_str(&ProtocolToken::to_prompt_section());
    }
}
```

---

## PromptBuilder Refactoring

After refactoring, `prompt_builder.rs` retains only:
- Type definitions (`SystemPromptPart`, `PromptConfig`, `PromptBuilder`, `Message`, `MessageRole`)
- 5 public build entry points (delegating to Pipeline)
- `build_messages()` and `build_observation()` (message building logic unchanged)
- Helper functions (`truncate_str`, `format_attachment`)

Estimated: **~350 lines** (down from 1724)

```rust
pub struct PromptBuilder {
    config: PromptConfig,
    pipeline: PromptPipeline,
}

impl PromptBuilder {
    pub fn new(config: PromptConfig) -> Self {
        let pipeline = PromptPipeline::default_layers(&config);
        Self { config, pipeline }
    }

    pub fn build_system_prompt(&self, tools: &[ToolInfo]) -> String {
        let input = LayerInput::basic(&self.config, tools);
        self.pipeline.execute(AssemblyPath::Basic, &input)
    }

    pub fn build_system_prompt_with_hydration(&self, hydration: &HydrationResult) -> String {
        let input = LayerInput::hydration(&self.config, hydration);
        self.pipeline.execute(AssemblyPath::Hydration, &input)
    }

    pub fn build_system_prompt_with_soul(&self, tools: &[ToolInfo], soul: &SoulManifest) -> String {
        let input = LayerInput::soul(&self.config, tools, soul);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }

    pub fn build_system_prompt_with_context(&self, ctx: &ResolvedContext) -> String {
        let input = LayerInput::context(&self.config, ctx);
        self.pipeline.execute(AssemblyPath::Context, &input)
    }

    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        let header = Self::build_static_header();
        let input = LayerInput::basic(&self.config, tools);
        let dynamic = self.pipeline.execute(AssemblyPath::Cached, &input);
        vec![
            SystemPromptPart { content: header, cache: true },
            SystemPromptPart { content: dynamic, cache: false },
        ]
    }
    // build_messages(), build_observation() — unchanged
}
```

---

## File Structure

```
core/src/thinker/
├── mod.rs                        # + pub mod layers; pub mod prompt_pipeline; pub mod prompt_layer;
├── prompt_builder.rs             # Slimmed: ~350 lines
├── prompt_pipeline.rs            # NEW: Pipeline engine + LayerInput + AssemblyPath (~100 lines)
├── prompt_layer.rs               # NEW: PromptLayer trait (~30 lines)
├── layers/                       # NEW: 20 Layer files
│   ├── mod.rs                    # re-exports (~25 lines)
│   ├── soul.rs                   # ~70 lines
│   ├── role.rs                   # ~15 lines
│   ├── runtime_context.rs        # ~15 lines
│   ├── environment.rs            # ~45 lines
│   ├── runtime_capabilities.rs   # ~25 lines
│   ├── tools.rs                  # ToolsLayer + HydratedToolsLayer (~80 lines)
│   ├── security.rs               # ~50 lines
│   ├── protocol_tokens.rs        # ~15 lines
│   ├── operational_guidelines.rs # ~40 lines
│   ├── citation_standards.rs     # ~15 lines
│   ├── generation_models.rs      # ~10 lines
│   ├── skill_instructions.rs     # ~15 lines
│   ├── special_actions.rs        # ~20 lines
│   ├── response_format.rs        # ~80 lines
│   ├── guidelines.rs             # ~15 lines
│   ├── thinking_guidance.rs      # ~45 lines
│   ├── skill_mode.rs             # ~30 lines
│   ├── custom_instructions.rs    # ~10 lines
│   └── language.rs               # ~25 lines
├── context.rs                    # unchanged
├── runtime_context.rs            # unchanged
├── protocol_tokens.rs            # unchanged
├── soul.rs                       # unchanged
└── ...                           # other files unchanged
```

---

## Testing Strategy

### Three-Layer Pyramid

1. **Unit tests** — each `layers/*.rs` file tests its own inject output and gate logic
2. **Pipeline tests** — `prompt_pipeline.rs` tests sorting, path filtering, execution
3. **Integration tests** — existing tests in `prompt_builder.rs` remain, testing public API

Existing 15+ integration tests migrate in-place (same file, same assertions). No new BDD feature files — pure refactor, no behavior change.

---

## Migration Path (Strangler Fig)

### Step 1: Scaffold (no existing code modified)

New files: `prompt_layer.rs`, `prompt_pipeline.rs`, `layers/mod.rs`. Add 3 `pub mod` lines to `mod.rs`.

**Gate**: `cargo build` passes, all existing tests green.

### Step 2: Simple Layers (10 layers, no external dependencies)

RoleLayer, GuidelinesLayer, SpecialActionsLayer, CitationStandardsLayer, GenerationModelsLayer, RuntimeCapabilitiesLayer, CustomInstructionsLayer, LanguageLayer, SkillInstructionsLayer, SkillModeLayer.

Build methods NOT changed yet — Layers exist but are not wired in.

**Gate**: `cargo test` all green (new unit tests + existing tests unchanged).

### Step 3: Complex Layers (10 layers, with dependencies/gates)

SoulLayer, ToolsLayer, HydratedToolsLayer, ResponseFormatLayer, ThinkingGuidanceLayer, RuntimeContextLayer, EnvironmentLayer, SecurityLayer, ProtocolTokensLayer, OperationalGuidelinesLayer.

**Gate**: All 20 Layer unit tests pass.

### Step 4: Wire Pipeline into build methods

Replace build method internals one at a time (lowest risk first):
1. `build_system_prompt()` → Basic
2. `build_system_prompt_cached()` → Cached
3. `build_system_prompt_with_hydration()` → Hydration
4. `build_system_prompt_with_soul()` → Soul
5. `build_system_prompt_with_context()` → Context

**Gate**: Full `cargo test` after each replacement.

### Step 5: Test migration

Move redundant unit tests from `prompt_builder.rs` to Layer files. Add Pipeline-level tests.

**Gate**: Test count does not decrease, coverage does not drop.

### Step 6: Cleanup

Delete all migrated `append_*` methods from `prompt_builder.rs`. Final ~350 lines.

**Gate**: `cargo test` green, `cargo clippy` clean.

### Rollback

Each step is an independent commit. Steps 1-3: delete new files. Step 4: revert individual build method changes. Steps 5-6: revert cleanup commits.

### Estimated Changes

| Step | Lines Added | Lines Modified | Lines Deleted |
|------|------------|---------------|--------------|
| 1 | ~150 | ~3 | 0 |
| 2 | ~300 | 0 | 0 |
| 3 | ~400 | 0 | 0 |
| 4 | ~30 | ~200 | ~900 |
| 5 | ~80 | ~50 | ~100 |
| 6 | 0 | 0 | ~200 |
| **Total** | **~960** | **~253** | **~1200** |

Net: ~240 fewer lines, distributed from 1 x 1724-line file into 20+ files of 10-80 lines each.
