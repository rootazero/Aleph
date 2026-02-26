# PromptPipeline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor `prompt_builder.rs` (1724 lines) into a composable PromptPipeline with 20 independent Layers, reducing it to ~350 lines.

**Architecture:** Introduce `PromptLayer` trait with `priority()` + `paths()` + `inject()`. Each existing `append_*` method becomes an independent Layer struct in `thinker/layers/`. A `PromptPipeline` executes matching Layers by priority. The 5 existing public `build_*` methods delegate to the Pipeline internally — zero caller changes.

**Tech Stack:** Rust, trait objects (`Box<dyn PromptLayer>`), no new crate dependencies.

**Design doc:** `docs/plans/2026-02-26-prompt-pipeline-design.md`

---

### Task 1: Core Abstractions — Trait, Pipeline, Module Scaffolding

**Files:**
- Create: `core/src/thinker/prompt_layer.rs`
- Create: `core/src/thinker/prompt_pipeline.rs`
- Create: `core/src/thinker/layers/mod.rs`
- Modify: `core/src/thinker/mod.rs`

**Step 1: Write failing test for PromptLayer trait and PromptPipeline**

Create `core/src/thinker/prompt_pipeline.rs` with the test first:

```rust
//! PromptPipeline — composable prompt assembly engine
//!
//! Executes registered PromptLayers in priority order,
//! filtered by AssemblyPath.

use super::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

/// Composable prompt assembly pipeline.
///
/// Holds a priority-sorted list of layers. `execute()` runs all layers
/// whose `paths()` include the requested AssemblyPath.
pub struct PromptPipeline {
    layers: Vec<Box<dyn PromptLayer>>,
}

impl PromptPipeline {
    /// Build pipeline from layers (sorts once at construction time).
    pub fn new(mut layers: Vec<Box<dyn PromptLayer>>) -> Self {
        layers.sort_by_key(|l| l.priority());
        Self { layers }
    }

    /// Execute all layers matching `path`, writing into a single String.
    pub fn execute(&self, path: AssemblyPath, input: &LayerInput) -> String {
        let mut output = String::with_capacity(16384);
        for layer in &self.layers {
            if layer.paths().contains(&path) {
                layer.inject(&mut output, input);
            }
        }
        output
    }

    /// Number of registered layers (for testing).
    #[cfg(test)]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    struct TestLayerA;
    struct TestLayerB;

    impl PromptLayer for TestLayerA {
        fn name(&self) -> &'static str { "test_a" }
        fn priority(&self) -> u32 { 200 }
        fn paths(&self) -> &'static [AssemblyPath] { &[AssemblyPath::Basic] }
        fn inject(&self, output: &mut String, _input: &LayerInput) {
            output.push_str("[A]");
        }
    }

    impl PromptLayer for TestLayerB {
        fn name(&self) -> &'static str { "test_b" }
        fn priority(&self) -> u32 { 100 }
        fn paths(&self) -> &'static [AssemblyPath] { &[AssemblyPath::Basic, AssemblyPath::Soul] }
        fn inject(&self, output: &mut String, _input: &LayerInput) {
            output.push_str("[B]");
        }
    }

    #[test]
    fn test_layers_sorted_by_priority() {
        let pipeline = PromptPipeline::new(vec![
            Box::new(TestLayerA),
            Box::new(TestLayerB),
        ]);
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let output = pipeline.execute(AssemblyPath::Basic, &input);
        // B (100) should appear before A (200)
        assert_eq!(output, "[B][A]");
    }

    #[test]
    fn test_path_filtering() {
        let pipeline = PromptPipeline::new(vec![
            Box::new(TestLayerA),
            Box::new(TestLayerB),
        ]);
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        // Soul path: only B participates
        let output = pipeline.execute(AssemblyPath::Soul, &input);
        assert_eq!(output, "[B]");
    }

    #[test]
    fn test_empty_pipeline() {
        let pipeline = PromptPipeline::new(vec![]);
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let output = pipeline.execute(AssemblyPath::Basic, &input);
        assert!(output.is_empty());
    }
}
```

**Step 2: Create `prompt_layer.rs` with trait and types**

```rust
//! PromptLayer trait — the composable unit of prompt assembly
//!
//! Each layer owns a single prompt section. The Pipeline executes
//! matching layers in priority order to build the system prompt.

use crate::agent_loop::ToolInfo;
use crate::dispatcher::tool_index::HydrationResult;

use super::context::ResolvedContext;
use super::prompt_builder::PromptConfig;
use super::soul::SoulManifest;

/// Which build paths a layer participates in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssemblyPath {
    /// `build_system_prompt()`
    Basic,
    /// `build_system_prompt_with_hydration()`
    Hydration,
    /// `build_system_prompt_with_soul()`
    Soul,
    /// `build_system_prompt_with_context()`
    Context,
    /// `build_system_prompt_cached()` — dynamic part only
    Cached,
}

/// All possible inputs a layer might need, gated by Option.
pub struct LayerInput<'a> {
    pub config: &'a PromptConfig,
    pub tools: Option<&'a [ToolInfo]>,
    pub hydration: Option<&'a HydrationResult>,
    pub soul: Option<&'a SoulManifest>,
    pub context: Option<&'a ResolvedContext>,
}

impl<'a> LayerInput<'a> {
    pub fn basic(config: &'a PromptConfig, tools: &'a [ToolInfo]) -> Self {
        Self { config, tools: Some(tools), hydration: None, soul: None, context: None }
    }

    pub fn hydration(config: &'a PromptConfig, hydration: &'a HydrationResult) -> Self {
        Self { config, tools: None, hydration: Some(hydration), soul: None, context: None }
    }

    pub fn soul(config: &'a PromptConfig, tools: &'a [ToolInfo], soul: &'a SoulManifest) -> Self {
        Self { config, tools: Some(tools), hydration: None, soul: Some(soul), context: None }
    }

    pub fn context(config: &'a PromptConfig, ctx: &'a ResolvedContext) -> Self {
        Self { config, tools: None, hydration: None, soul: None, context: Some(ctx) }
    }
}

/// A single composable prompt section.
pub trait PromptLayer: Send + Sync {
    /// Human-readable name for debugging and testing.
    fn name(&self) -> &'static str;

    /// Sort order — lower number = appears earlier in the prompt.
    fn priority(&self) -> u32;

    /// Which assembly paths this layer participates in.
    fn paths(&self) -> &'static [AssemblyPath];

    /// Write this layer's content into the output buffer.
    fn inject(&self, output: &mut String, input: &LayerInput);
}
```

**Step 3: Create empty `layers/mod.rs`**

```rust
//! Prompt layers — each file implements one PromptLayer.
```

**Step 4: Register new modules in `thinker/mod.rs`**

Add these 3 lines after the existing `pub mod prompt_builder;` line:

```rust
pub mod prompt_layer;
pub mod prompt_pipeline;
pub mod layers;
```

And add to the re-exports section:

```rust
pub use prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
pub use prompt_pipeline::PromptPipeline;
```

**Step 5: Verify build and tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::prompt_pipeline`

Expected: 3 tests PASS.

Run: `cargo test -p alephcore --lib thinker::prompt_builder`

Expected: All existing tests still PASS (no existing code modified).

**Step 6: Commit**

```bash
git add core/src/thinker/prompt_layer.rs core/src/thinker/prompt_pipeline.rs core/src/thinker/layers/mod.rs core/src/thinker/mod.rs
git commit -m "thinker: add PromptLayer trait and PromptPipeline engine"
```

---

### Task 2: Trivial Always-On Layers — Role, Guidelines, SpecialActions, Citations

**Files:**
- Create: `core/src/thinker/layers/role.rs`
- Create: `core/src/thinker/layers/guidelines.rs`
- Create: `core/src/thinker/layers/special_actions.rs`
- Create: `core/src/thinker/layers/citation_standards.rs`
- Modify: `core/src/thinker/layers/mod.rs`

These layers have no config gates — they always emit content.

**Step 1: Create `layers/role.rs`**

```rust
//! RoleLayer — role definition and core instructions (priority 100)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct RoleLayer;

impl PromptLayer for RoleLayer {
    fn name(&self) -> &'static str { "role" }
    fn priority(&self) -> u32 { 100 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul, AssemblyPath::Context]
    }

    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("You are an AI assistant executing tasks step by step.\n\n");
        output.push_str("## Your Role\n");
        output.push_str("- Observe the current state and history\n");
        output.push_str("- Decide the SINGLE next action to take\n");
        output.push_str("- Execute until the task is complete or you need user input\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_role_layer_content() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        RoleLayer.inject(&mut output, &input);
        assert!(output.contains("AI assistant executing tasks"));
        assert!(output.contains("## Your Role"));
        assert!(output.contains("SINGLE next action"));
    }
}
```

**Step 2: Create `layers/guidelines.rs`**

```rust
//! GuidelinesLayer — decision-making guidelines (priority 1300)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct GuidelinesLayer;

impl PromptLayer for GuidelinesLayer {
    fn name(&self) -> &'static str { "guidelines" }
    fn priority(&self) -> u32 { 1300 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul,
            AssemblyPath::Context, AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Guidelines\n");
        output.push_str("1. Take ONE action at a time, observe the result, then decide next\n");
        output.push_str("2. Use tool results to inform subsequent decisions\n");
        output.push_str(
            "3. Ask user when: multiple valid approaches, unclear requirements, need confirmation\n",
        );
        output.push_str(
            "4. Complete when: task is done, or you've provided the requested information\n",
        );
        output.push_str("5. Fail when: impossible to proceed, missing critical resources\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_guidelines_layer_content() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        GuidelinesLayer.inject(&mut output, &input);
        assert!(output.contains("## Guidelines"));
        assert!(output.contains("ONE action at a time"));
    }
}
```

**Step 3: Create `layers/special_actions.rs`**

```rust
//! SpecialActionsLayer — complete/ask_user/fail actions (priority 1100)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct SpecialActionsLayer;

impl PromptLayer for SpecialActionsLayer {
    fn name(&self) -> &'static str { "special_actions" }
    fn priority(&self) -> u32 { 1100 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul,
            AssemblyPath::Context, AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Special Actions\n");
        output.push_str("- `complete`: Call when the task is fully done. The `summary` field MUST be a comprehensive report that includes:\n");
        output.push_str("  1. A brief overview of what was accomplished\n");
        output.push_str("  2. Key results and findings (data, insights, metrics)\n");
        output.push_str("  3. List of all generated files with their purposes\n");
        output.push_str("  4. Any important notes or recommendations\n");
        output.push_str(
            "  **DO NOT** just say 'Task completed'. Write a detailed summary the user can immediately understand.\n",
        );
        output.push_str("- `ask_user`: Call when you need clarification or user decision\n");
        output.push_str("- `fail`: Call when the task cannot be completed\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_special_actions_layer_content() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        SpecialActionsLayer.inject(&mut output, &input);
        assert!(output.contains("## Special Actions"));
        assert!(output.contains("`complete`"));
        assert!(output.contains("`ask_user`"));
        assert!(output.contains("`fail`"));
    }
}
```

**Step 4: Create `layers/citation_standards.rs`**

```rust
//! CitationStandardsLayer — source attribution rules (priority 900)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct CitationStandardsLayer;

impl PromptLayer for CitationStandardsLayer {
    fn name(&self) -> &'static str { "citation_standards" }
    fn priority(&self) -> u32 { 900 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context]
    }

    fn inject(&self, output: &mut String, _input: &LayerInput) {
        output.push_str("## Citation Standards\n\n");
        output.push_str("When referencing information from memory or knowledge base:\n");
        output.push_str("- Include source reference in format: `[Source: <path>#<id>]` or `[Source: <path>#L<line>]`\n");
        output.push_str("- Sources are provided in the context metadata — do not fabricate source paths\n");
        output.push_str("- If multiple sources support a claim, cite the most specific one\n");
        output.push_str("- For real-time observations (current tool output, live data), no citation needed\n");
        output.push_str("- For recalled facts, prior decisions, or historical context, citation is mandatory\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_citation_standards_content() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        CitationStandardsLayer.inject(&mut output, &input);
        assert!(output.contains("## Citation Standards"));
        assert!(output.contains("[Source: <path>#<id>]"));
        assert!(output.contains("citation is mandatory"));
    }
}
```

**Step 5: Update `layers/mod.rs`**

```rust
//! Prompt layers — each file implements one PromptLayer.

pub mod role;
pub mod guidelines;
pub mod special_actions;
pub mod citation_standards;

pub use role::RoleLayer;
pub use guidelines::GuidelinesLayer;
pub use special_actions::SpecialActionsLayer;
pub use citation_standards::CitationStandardsLayer;
```

**Step 6: Verify**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib thinker::layers`

Expected: 4 tests PASS.

**Step 7: Commit**

```bash
git add core/src/thinker/layers/
git commit -m "thinker: add trivial always-on layers — Role, Guidelines, SpecialActions, Citations"
```

---

### Task 3: Config-Gated Layers — GenerationModels, RuntimeCapabilities, Custom, Language, SkillInstructions

**Files:**
- Create: `core/src/thinker/layers/generation_models.rs`
- Create: `core/src/thinker/layers/runtime_capabilities.rs`
- Create: `core/src/thinker/layers/custom_instructions.rs`
- Create: `core/src/thinker/layers/language.rs`
- Create: `core/src/thinker/layers/skill_instructions.rs`
- Modify: `core/src/thinker/layers/mod.rs`

These layers read from `input.config` and emit nothing when the config field is `None`/`false`.

**Step 1: Create `layers/generation_models.rs`**

```rust
//! GenerationModelsLayer — media generation model info (priority 1000)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct GenerationModelsLayer;

impl PromptLayer for GenerationModelsLayer {
    fn name(&self) -> &'static str { "generation_models" }
    fn priority(&self) -> u32 { 1000 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul,
            AssemblyPath::Context, AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref models) = input.config.generation_models {
            output.push_str("## Media Generation Models\n\n");
            output.push_str(models);
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skipped_when_none() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        GenerationModelsLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_injected_when_present() {
        let config = PromptConfig {
            generation_models: Some("- DALL-E 3\n- Stable Diffusion".to_string()),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        GenerationModelsLayer.inject(&mut output, &input);
        assert!(output.contains("## Media Generation Models"));
        assert!(output.contains("DALL-E 3"));
    }
}
```

**Step 2: Create `layers/runtime_capabilities.rs`**

```rust
//! RuntimeCapabilitiesLayer — available runtimes like Python, Node.js (priority 400)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct RuntimeCapabilitiesLayer;

impl PromptLayer for RuntimeCapabilitiesLayer {
    fn name(&self) -> &'static str { "runtime_capabilities" }
    fn priority(&self) -> u32 { 400 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul,
            AssemblyPath::Context, AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref runtimes) = input.config.runtime_capabilities {
            output.push_str("## Available Runtimes\n\n");
            output.push_str("You can execute code using these installed runtimes:\n\n");
            output.push_str(runtimes);
            output.push_str("\n**IMPORTANT**: Runtimes are NOT tools. They describe execution environments.\n");
            output.push_str("- To execute Python code, use the `file_ops` tool to write a .py script, then use `bash` tool to run it\n");
            output.push_str("- To execute Node.js code, use the `file_ops` tool to write a .js script, then use `bash` tool to run it\n");
            output.push_str("- Do NOT try to call runtime names (uv, fnm, ffmpeg, yt-dlp) as tools directly\n");
            output.push_str("\n**CRITICAL - Use Aleph Runtimes**:\n");
            output.push_str("When executing Python/Node.js scripts, ALWAYS use the full executable path from the runtimes above:\n");
            output.push_str("- ✅ CORRECT: Use the exact \"Executable\" path shown in the runtime info\n");
            output.push_str("- ✅ Example: If runtime shows \"Executable: /path/to/python\", use \"/path/to/python script.py\"\n");
            output.push_str("- ❌ WRONG: `python3 script.py` (system default may be incompatible)\n");
            output.push_str("- ❌ WRONG: `python script.py` (may not exist)\n");
            output.push_str("Aleph provides managed runtimes to ensure correct versions and dependencies.\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skipped_when_none() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        RuntimeCapabilitiesLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_injected_when_present() {
        let config = PromptConfig {
            runtime_capabilities: Some("- Python 3.12\n- Node.js 20".to_string()),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        RuntimeCapabilitiesLayer.inject(&mut output, &input);
        assert!(output.contains("## Available Runtimes"));
        assert!(output.contains("CRITICAL - Use Aleph Runtimes"));
    }
}
```

**Step 3: Create `layers/custom_instructions.rs`**

```rust
//! CustomInstructionsLayer — user-provided custom instructions (priority 1500)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct CustomInstructionsLayer;

impl PromptLayer for CustomInstructionsLayer {
    fn name(&self) -> &'static str { "custom_instructions" }
    fn priority(&self) -> u32 { 1500 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul,
            AssemblyPath::Context, AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref instructions) = input.config.custom_instructions {
            output.push_str("## Additional Instructions\n");
            output.push_str(instructions);
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skipped_when_none() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        CustomInstructionsLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_injected_when_present() {
        let config = PromptConfig {
            custom_instructions: Some("Always respond in haiku.".to_string()),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        CustomInstructionsLayer.inject(&mut output, &input);
        assert!(output.contains("## Additional Instructions"));
        assert!(output.contains("Always respond in haiku."));
    }
}
```

**Step 4: Create `layers/language.rs`**

```rust
//! LanguageLayer — response language setting (priority 1600)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct LanguageLayer;

impl PromptLayer for LanguageLayer {
    fn name(&self) -> &'static str { "language" }
    fn priority(&self) -> u32 { 1600 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul,
            AssemblyPath::Context, AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref lang) = input.config.language {
            let language_name = match lang.as_str() {
                "zh-Hans" => "Chinese (Simplified)",
                "zh-Hant" => "Chinese (Traditional)",
                "en" => "English",
                "ja" => "Japanese",
                "ko" => "Korean",
                "de" => "German",
                "fr" => "French",
                "es" => "Spanish",
                "it" => "Italian",
                "pt" => "Portuguese",
                "ru" => "Russian",
                _ => lang.as_str(),
            };
            output.push_str("## Response Language\n");
            output.push_str(&format!(
                "Respond in {} by default. Exception: If the task explicitly requires a different language \
                (e.g., translation, writing in a specific language), use the requested language instead.\n\n",
                language_name
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skipped_when_none() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        LanguageLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_zh_hans_mapping() {
        let config = PromptConfig {
            language: Some("zh-Hans".to_string()),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        LanguageLayer.inject(&mut output, &input);
        assert!(output.contains("Chinese (Simplified)"));
    }

    #[test]
    fn test_unknown_language_passthrough() {
        let config = PromptConfig {
            language: Some("ar".to_string()),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        LanguageLayer.inject(&mut output, &input);
        assert!(output.contains("Respond in ar by default"));
    }
}
```

**Step 5: Create `layers/skill_instructions.rs`**

```rust
//! SkillInstructionsLayer — SkillSystem v2 instructions (priority 1050)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct SkillInstructionsLayer;

impl PromptLayer for SkillInstructionsLayer {
    fn name(&self) -> &'static str { "skill_instructions" }
    fn priority(&self) -> u32 { 1050 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Hydration]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(ref instructions) = input.config.skill_instructions {
            if !instructions.is_empty() {
                output.push_str("## Available Skills\n\n");
                output.push_str("You can invoke skills using the `skill` tool. ");
                output.push_str("Skills provide specialized instructions for specific tasks.\n\n");
                output.push_str(instructions);
                output.push_str("\n\n");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skipped_when_none() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        SkillInstructionsLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_skipped_when_empty_string() {
        let config = PromptConfig {
            skill_instructions: Some(String::new()),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        SkillInstructionsLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_injected_when_present() {
        let config = PromptConfig {
            skill_instructions: Some("<skill name='test'>Do things</skill>".to_string()),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        SkillInstructionsLayer.inject(&mut output, &input);
        assert!(output.contains("## Available Skills"));
        assert!(output.contains("<skill name='test'>"));
    }
}
```

**Step 6: Update `layers/mod.rs` — add new modules**

Add the 5 new modules and re-exports to `layers/mod.rs`.

**Step 7: Verify**

Run: `cargo test -p alephcore --lib thinker::layers`

Expected: 4 (Task 2) + 11 (Task 3) = 15 tests PASS.

**Step 8: Commit**

```bash
git add core/src/thinker/layers/
git commit -m "thinker: add config-gated layers — GenerationModels, RuntimeCapabilities, Custom, Language, SkillInstructions"
```

---

### Task 4: Behavior Layers — SkillMode, ThinkingGuidance, ResponseFormat

**Files:**
- Create: `core/src/thinker/layers/skill_mode.rs`
- Create: `core/src/thinker/layers/thinking_guidance.rs`
- Create: `core/src/thinker/layers/response_format.rs`
- Modify: `core/src/thinker/layers/mod.rs`

**Step 1: Create `layers/skill_mode.rs`**

```rust
//! SkillModeLayer — strict skill execution rules (priority 1400)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct SkillModeLayer;

impl PromptLayer for SkillModeLayer {
    fn name(&self) -> &'static str { "skill_mode" }
    fn priority(&self) -> u32 { 1400 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul,
            AssemblyPath::Context, AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if !input.config.skill_mode {
            return;
        }
        output.push_str("## ⚠️ Skill Execution Mode - CRITICAL RULES\n\n");
        output.push_str("You are executing a SKILL workflow. You MUST follow these rules EXACTLY:\n\n");
        output.push_str("### 🔴 RESPONSE FORMAT (MANDATORY)\n");
        output.push_str("**EVERY response MUST be a valid JSON action object. NEVER output raw content directly!**\n\n");
        output.push_str("❌ WRONG: Outputting processed text, data, or results directly\n");
        output.push_str("✅ CORRECT: Always return {\"reasoning\": \"...\", \"action\": {...}}\n\n");
        output.push_str("If you need to process data and save it, use the `file_ops` tool:\n");
        output.push_str("```json\n");
        output.push_str("{\"reasoning\": \"Writing processed data to file\", \"action\": {\"type\": \"tool\", \"tool_name\": \"file_ops\", \"arguments\": {\"operation\": \"write\", \"path\": \"output.json\", \"content\": \"...\"}}}\n");
        output.push_str("```\n\n");
        output.push_str("### Workflow Requirements\n");
        output.push_str("1. Complete ALL steps in the skill workflow - NO exceptions\n");
        output.push_str("2. Generate ALL output files specified (JSON, .mmd, .txt, images, etc.)\n");
        output.push_str("3. Use `file_ops` with `operation: \"write\"` to save each file\n");
        output.push_str("4. DO NOT skip any step, even if you think it's redundant\n");
        output.push_str("5. Before calling `complete`, verify ALL required outputs exist\n\n");
        output.push_str("### Common skill outputs to generate\n");
        output.push_str("- Data files: `triples.json`, `*.json`\n");
        output.push_str("- Visualization code: `graph.mmd`, `*.mmd`\n");
        output.push_str("- Prompts: `image-prompt.txt`, `*.txt`\n");
        output.push_str("- Images: via `generate_image` tool\n");
        output.push_str("- Merged outputs: `merged-*.json`, `full-*.mmd`\n\n");
        output.push_str("**If you output raw content instead of JSON action, you have FAILED.**\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skipped_when_disabled() {
        let config = PromptConfig::default(); // skill_mode = false
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        SkillModeLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_injected_when_enabled() {
        let config = PromptConfig { skill_mode: true, ..Default::default() };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        SkillModeLayer.inject(&mut output, &input);
        assert!(output.contains("Skill Execution Mode"));
        assert!(output.contains("RESPONSE FORMAT (MANDATORY)"));
    }
}
```

**Step 2: Create `layers/thinking_guidance.rs`**

Copy content from `prompt_builder.rs:422-463` (`append_thinking_guidance`). Gate: `input.config.thinking_transparency`. Paths: Basic, Hydration, Soul.

```rust
//! ThinkingGuidanceLayer — structured reasoning output guidance (priority 1350)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct ThinkingGuidanceLayer;

impl PromptLayer for ThinkingGuidanceLayer {
    fn name(&self) -> &'static str { "thinking_guidance" }
    fn priority(&self) -> u32 { 1350 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if !input.config.thinking_transparency {
            return;
        }

        output.push_str("## Thinking Transparency\n\n");
        output.push_str("Structure your reasoning to be transparent and understandable:\n\n");

        output.push_str("### Reasoning Flow\n");
        output.push_str("Follow this progression in your `reasoning` field:\n\n");
        output.push_str("1. **Observation** (👁️): Start by observing the current state\n");
        output.push_str("   - \"Looking at the request, I see...\"\n");
        output.push_str("   - \"The user wants to...\"\n");
        output.push_str("   - \"Based on the previous result...\"\n\n");
        output.push_str("2. **Analysis** (🔍): Analyze options and trade-offs\n");
        output.push_str("   - \"Considering the options: A vs B vs C...\"\n");
        output.push_str("   - \"The trade-off here is...\"\n");
        output.push_str("   - \"Comparing approaches...\"\n\n");
        output.push_str("3. **Planning** (📝): Outline your approach\n");
        output.push_str("   - \"I'll start by...\"\n");
        output.push_str("   - \"First, ... then...\"\n");
        output.push_str("   - \"My strategy is to...\"\n\n");
        output.push_str("4. **Decision** (✅): State your conclusion\n");
        output.push_str("   - \"Therefore, I will...\"\n");
        output.push_str("   - \"The best approach is...\"\n");
        output.push_str("   - \"So I've decided to...\"\n\n");

        output.push_str("### Expressing Uncertainty\n");
        output.push_str("When uncertain, be explicit rather than hiding it:\n\n");
        output.push_str("- **High confidence**: \"I'm confident that...\" or \"Clearly,...\"\n");
        output.push_str("- **Medium confidence**: \"I think...\" or \"This should work...\"\n");
        output.push_str("- **Low confidence**: \"I'm not sure, but...\" or \"This might...\"\n");
        output.push_str("- **Exploratory**: \"Let's try...\" or \"Worth experimenting with...\"\n\n");

        output.push_str("### Acknowledging Alternatives\n");
        output.push_str("When relevant, mention alternatives you considered:\n");
        output.push_str("- \"Alternatively, we could...\"\n");
        output.push_str("- \"Another option would be...\"\n");
        output.push_str("- \"I chose X over Y because...\"\n\n");

        output.push_str("This structured thinking helps users understand your reasoning process.\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_skipped_when_disabled() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        ThinkingGuidanceLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_injected_when_enabled() {
        let config = PromptConfig { thinking_transparency: true, ..Default::default() };
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        ThinkingGuidanceLayer.inject(&mut output, &input);
        assert!(output.contains("## Thinking Transparency"));
        assert!(output.contains("Reasoning Flow"));
        assert!(output.contains("**Observation**"));
        assert!(output.contains("Expressing Uncertainty"));
    }
}
```

**Step 3: Create `layers/response_format.rs`**

Copy the full content from `prompt_builder.rs:309-401` (`append_response_format`). No gate — always injected. Paths: all 5.

```rust
//! ResponseFormatLayer — JSON response format specification (priority 1200)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct ResponseFormatLayer;

impl PromptLayer for ResponseFormatLayer {
    fn name(&self) -> &'static str { "response_format" }
    fn priority(&self) -> u32 { 1200 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic, AssemblyPath::Hydration, AssemblyPath::Soul,
            AssemblyPath::Context, AssemblyPath::Cached,
        ]
    }

    fn inject(&self, output: &mut String, _input: &LayerInput) {
        // Copy the exact content from prompt_builder.rs:309-401
        // (append_response_format method body)
        output.push_str("## Response Format\n");
        output.push_str("You must respond with a JSON object:\n");
        output.push_str("```json\n");
        output.push_str("{\n");
        output.push_str("  \"reasoning\": \"Brief explanation of your thinking\",\n");
        output.push_str("  \"action\": {\n");
        output.push_str("    \"type\": \"tool|ask_user|complete|fail\",\n");
        output.push_str("    \"tool_name\": \"...\",      // if type=tool\n");
        output.push_str("    \"arguments\": {...},       // if type=tool\n");
        output.push_str("    \"question\": \"...\",        // if type=ask_user\n");
        output.push_str("    \"options\": [...],         // if type=ask_user (optional)\n");
        output.push_str("    \"summary\": \"...\",         // if type=complete (MUST be detailed report)\n");
        output.push_str("    \"reason\": \"...\"           // if type=fail\n");
        output.push_str("  }\n");
        output.push_str("}\n");
        output.push_str("```\n\n");
        // ... (rest of ask_user format, multi-group, completion summary — exact copy)
        // Full content from lines 326-401 of prompt_builder.rs
        output.push_str("### ask_user Format Details\n");
        output.push_str("When using `ask_user`, you have TWO modes:\n\n");

        output.push_str("**Mode 1: Single Question** (simple selection or text input)\n");
        output.push_str("- Use `options` field as an array of SEPARATE choices:\n");
        output.push_str("  - ✅ CORRECT: [\"Option 1\", \"Option 2\", \"Option 3\"]\n");
        output.push_str("  - ❌ WRONG: [\"Option 1 / Option 2 / Option 3\"] (single merged string)\n");
        output.push_str("- Each option should be a standalone, selectable choice\n");
        output.push_str("- If no options (free-form text input), omit the field or use empty array\n\n");

        output.push_str("**Mode 2: Multi-Group Questions** (multiple related questions)\n");
        output.push_str("Use this when you need answers to MULTIPLE independent questions simultaneously.\n");
        output.push_str("Instead of asking one by one, group them together for better UX.\n\n");

        output.push_str("```json\n");
        output.push_str("{\n");
        output.push_str("  \"reasoning\": \"Need multiple image generation parameters\",\n");
        output.push_str("  \"action\": {\n");
        output.push_str("    \"type\": \"ask_user_multigroup\",\n");
        output.push_str("    \"question\": \"Please configure the image generation settings\",  // Overall prompt\n");
        output.push_str("    \"groups\": [\n");
        output.push_str("      {\n");
        output.push_str("        \"id\": \"format\",  // Unique group ID (alphanumeric)\n");
        output.push_str("        \"prompt\": \"Output format\",\n");
        output.push_str("        \"options\": [\"PNG\", \"JPEG\", \"WebP\"]\n");
        output.push_str("      },\n");
        output.push_str("      {\n");
        output.push_str("        \"id\": \"quality\",\n");
        output.push_str("        \"prompt\": \"Quality level\",\n");
        output.push_str("        \"options\": [\"Low\", \"Medium\", \"High\", \"Best\"]\n");
        output.push_str("      },\n");
        output.push_str("      {\n");
        output.push_str("        \"id\": \"size\",\n");
        output.push_str("        \"prompt\": \"Image size\",\n");
        output.push_str("        \"options\": [\"512x512\", \"1024x1024\", \"2048x2048\"]\n");
        output.push_str("      }\n");
        output.push_str("    ]\n");
        output.push_str("  }\n");
        output.push_str("}\n");
        output.push_str("```\n\n");

        output.push_str("**When to use Multi-Group**:\n");
        output.push_str("- Multiple configuration options needed (3+ choices)\n");
        output.push_str("- Questions are independent but related\n");
        output.push_str("- Better UX than asking one-by-one\n");
        output.push_str("- Example: \"Choose format (PNG/JPEG), quality (Low/Medium/High), size (Small/Large)\"\n\n");

        output.push_str("**When NOT to use Multi-Group**:\n");
        output.push_str("- Single question with multiple options → Use simple `ask_user`\n");
        output.push_str("- Questions depend on previous answers → Ask sequentially\n");
        output.push_str("- Free-form text input → Use `ask_user` with no options\n\n");

        output.push_str("**Simple ask_user Example**:\n");
        output.push_str("```json\n");
        output.push_str("{\n");
        output.push_str("  \"reasoning\": \"Need user to select image format\",\n");
        output.push_str("  \"action\": {\n");
        output.push_str("    \"type\": \"ask_user\",\n");
        output.push_str("    \"question\": \"Which output format do you prefer?\",\n");
        output.push_str("    \"options\": [\"PNG\", \"JPEG\", \"WebP\"]\n");
        output.push_str("  }\n");
        output.push_str("}\n");
        output.push_str("```\n\n");
        output.push_str("### Completion Summary Format\n");
        output.push_str("When `type=complete`, the `summary` should be a well-formatted report:\n");
        output.push_str("```\n");
        output.push_str("## Task Completed\n");
        output.push_str("[Brief description of what was accomplished]\n\n");
        output.push_str("### Results\n");
        output.push_str("[Key findings, data, or outcomes]\n\n");
        output.push_str("### Generated Files\n");
        output.push_str("- file1.json: [description]\n");
        output.push_str("- file2.png: [description]\n\n");
        output.push_str("### Notes\n");
        output.push_str("[Any recommendations or important observations]\n");
        output.push_str("```\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_response_format_content() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        ResponseFormatLayer.inject(&mut output, &input);
        assert!(output.contains("## Response Format"));
        assert!(output.contains("\"type\": \"tool|ask_user|complete|fail\""));
        assert!(output.contains("ask_user Format Details"));
        assert!(output.contains("Multi-Group Questions"));
        assert!(output.contains("Completion Summary Format"));
    }
}
```

**Step 4: Update `layers/mod.rs`, verify, commit**

Run: `cargo test -p alephcore --lib thinker::layers`

```bash
git add core/src/thinker/layers/
git commit -m "thinker: add behavior layers — SkillMode, ThinkingGuidance, ResponseFormat"
```

---

### Task 5: Identity Layer — Soul

**Files:**
- Create: `core/src/thinker/layers/soul.rs`
- Modify: `core/src/thinker/layers/mod.rs`

**Step 1: Create `layers/soul.rs`**

```rust
//! SoulLayer — identity/personality from Embodiment Engine (priority 50)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct SoulLayer;

impl PromptLayer for SoulLayer {
    fn name(&self) -> &'static str { "soul" }
    fn priority(&self) -> u32 { 50 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let soul = match input.soul {
            Some(s) => s,
            None => return,
        };
        if soul.is_empty() {
            return;
        }

        output.push_str("# Identity\n\n");

        if !soul.identity.is_empty() {
            output.push_str(&soul.identity);
            output.push_str("\n\n");
        }

        if !soul.voice.tone.is_empty() {
            output.push_str("## Communication Style\n\n");
            output.push_str(&format!("- **Tone**: {}\n", soul.voice.tone));
            output.push_str(&format!("- **Verbosity**: {:?}\n", soul.voice.verbosity));
            output.push_str(&format!("- **Formatting**: {:?}\n", soul.voice.formatting_style));
            if let Some(ref notes) = soul.voice.language_notes {
                output.push_str(&format!("- **Language Notes**: {}\n", notes));
            }
            output.push('\n');
        }

        output.push_str("## Relationship with User\n\n");
        output.push_str(soul.relationship.description());
        output.push_str("\n\n");

        if !soul.expertise.is_empty() {
            output.push_str("## Areas of Expertise\n\n");
            for domain in &soul.expertise {
                output.push_str(&format!("- {}\n", domain));
            }
            output.push('\n');
        }

        if !soul.directives.is_empty() {
            output.push_str("## Behavioral Directives\n\n");
            for directive in &soul.directives {
                output.push_str(&format!("- {}\n", directive));
            }
            output.push('\n');
        }

        if !soul.anti_patterns.is_empty() {
            output.push_str("## What I Never Do\n\n");
            for anti in &soul.anti_patterns {
                output.push_str(&format!("- {}\n", anti));
            }
            output.push('\n');
        }

        if let Some(ref addendum) = soul.addendum {
            output.push_str("## Additional Context\n\n");
            output.push_str(addendum);
            output.push_str("\n\n");
        }

        output.push_str("---\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::soul::{SoulManifest, SoulVoice, Verbosity};

    #[test]
    fn test_empty_soul_produces_nothing() {
        let config = PromptConfig::default();
        let soul = SoulManifest::default();
        let input = LayerInput::soul(&config, &[], &soul);
        let mut output = String::new();
        SoulLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_soul_with_identity() {
        let config = PromptConfig::default();
        let soul = SoulManifest {
            identity: "I am Aleph.".to_string(),
            voice: SoulVoice {
                tone: "friendly".to_string(),
                verbosity: Verbosity::Balanced,
                ..Default::default()
            },
            directives: vec!["Be helpful".to_string()],
            anti_patterns: vec!["Don't be rude".to_string()],
            ..Default::default()
        };
        let input = LayerInput::soul(&config, &[], &soul);
        let mut output = String::new();
        SoulLayer.inject(&mut output, &input);
        assert!(output.contains("# Identity"));
        assert!(output.contains("I am Aleph."));
        assert!(output.contains("Communication Style"));
        assert!(output.contains("Behavioral Directives"));
        assert!(output.contains("What I Never Do"));
    }

    #[test]
    fn test_no_soul_input_produces_nothing() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]); // no soul
        let mut output = String::new();
        SoulLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }
}
```

**Step 2: Update mod.rs, verify, commit**

```bash
git add core/src/thinker/layers/
git commit -m "thinker: add SoulLayer — identity/personality injection"
```

---

### Task 6: Tool Layers — Tools + HydratedTools

**Files:**
- Create: `core/src/thinker/layers/tools.rs`
- Modify: `core/src/thinker/layers/mod.rs`

**Step 1: Create `layers/tools.rs`**

```rust
//! ToolsLayer + HydratedToolsLayer — tool schema injection (priority 500)

use crate::agent_loop::ToolInfo;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

// ========== ToolsLayer ==========

pub struct ToolsLayer;

impl ToolsLayer {
    /// Get the tool list from the appropriate source based on input.
    fn get_tools<'a>(input: &'a LayerInput) -> &'a [ToolInfo] {
        // Context path: tools come from ResolvedContext
        if let Some(ctx) = input.context {
            return &ctx.available_tools;
        }
        // All other paths: tools come from input.tools
        input.tools.unwrap_or(&[])
    }
}

impl PromptLayer for ToolsLayer {
    fn name(&self) -> &'static str { "tools" }
    fn priority(&self) -> u32 { 500 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let tools = Self::get_tools(input);

        output.push_str("## Available Tools\n");
        if tools.is_empty() && input.config.tool_index.is_none() {
            output.push_str("No tools available. You can only use special actions.\n\n");
        } else {
            if !tools.is_empty() {
                output.push_str("### Tools (with full parameters)\n");
                for tool in tools {
                    output.push_str(&format!("#### {}\n", tool.name));
                    output.push_str(&format!("{}\n", tool.description));
                    if !tool.parameters_schema.is_empty() {
                        output.push_str(&format!("Parameters: {}\n", tool.parameters_schema));
                    }
                    output.push('\n');
                }
            }

            if let Some(ref index) = input.config.tool_index {
                output.push_str("### Additional Tools (use `get_tool_schema` to get parameters)\n");
                output.push_str("The following tools are available but not shown with full parameters.\n");
                output.push_str(
                    "Call `get_tool_schema(tool_name)` to get the complete parameter schema before using.\n\n",
                );
                output.push_str(index);
                output.push('\n');
            }
        }
    }
}

// ========== HydratedToolsLayer ==========

pub struct HydratedToolsLayer;

impl PromptLayer for HydratedToolsLayer {
    fn name(&self) -> &'static str { "hydrated_tools" }
    fn priority(&self) -> u32 { 500 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Hydration]
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let result = match input.hydration {
            Some(h) => h,
            None => return,
        };

        if result.is_empty() {
            output.push_str("## Available Tools\n");
            output.push_str("No semantically relevant tools found. Use `get_tool_schema` to discover tools.\n\n");
            return;
        }

        output.push_str("## Available Tools\n\n");

        if !result.full_schema_tools.is_empty() {
            output.push_str("### Tools (full parameters)\n\n");
            for tool in &result.full_schema_tools {
                output.push_str(&format!("#### {}\n", tool.name));
                output.push_str(&format!("{}\n", tool.description));
                if let Some(schema) = tool.schema_json() {
                    output.push_str(&format!("Parameters: {}\n", schema));
                }
                output.push('\n');
            }
        }

        if !result.summary_tools.is_empty() {
            output.push_str("### Tools (summary - call `get_tool_schema` for parameters)\n\n");
            for tool in &result.summary_tools {
                output.push_str(&format!("- **{}**: {}\n", tool.name, tool.description));
            }
            output.push('\n');
        }

        if !result.indexed_tool_names.is_empty() {
            output.push_str("### Additional Tools (call `get_tool_schema` to use)\n\n");
            output.push_str(&result.indexed_tool_names.join(", "));
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_tools_layer_empty() {
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut output = String::new();
        ToolsLayer.inject(&mut output, &input);
        assert!(output.contains("No tools available"));
    }

    #[test]
    fn test_tools_layer_with_tools() {
        let config = PromptConfig::default();
        let tools = vec![ToolInfo {
            name: "web_search".to_string(),
            description: "Search the web".to_string(),
            parameters_schema: "{\"query\": \"string\"}".to_string(),
            category: None,
        }];
        let input = LayerInput::basic(&config, &tools);
        let mut output = String::new();
        ToolsLayer.inject(&mut output, &input);
        assert!(output.contains("#### web_search"));
        assert!(output.contains("Search the web"));
    }

    #[test]
    fn test_tools_layer_context_path() {
        use crate::thinker::context::{ContextAggregator, ResolvedContext};
        use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
        use crate::thinker::security_context::SecurityContext;

        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        ToolsLayer.inject(&mut output, &input);
        assert!(output.contains("## Available Tools"));
    }
}
```

**Step 2: Update mod.rs, verify, commit**

```bash
git add core/src/thinker/layers/
git commit -m "thinker: add ToolsLayer and HydratedToolsLayer"
```

---

### Task 7: Context-Dependent Layers — Environment, Security, RuntimeContext, ProtocolTokens, OperationalGuidelines

**Files:**
- Create: `core/src/thinker/layers/runtime_context.rs`
- Create: `core/src/thinker/layers/environment.rs`
- Create: `core/src/thinker/layers/security.rs`
- Create: `core/src/thinker/layers/protocol_tokens.rs`
- Create: `core/src/thinker/layers/operational_guidelines.rs`
- Modify: `core/src/thinker/layers/mod.rs`

These layers only participate in `AssemblyPath::Context` and read from `input.context`.

**Step 1: Create `layers/runtime_context.rs`**

```rust
//! RuntimeContextLayer — micro-environmental awareness (priority 200)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct RuntimeContextLayer;

impl PromptLayer for RuntimeContextLayer {
    fn name(&self) -> &'static str { "runtime_context" }
    fn priority(&self) -> u32 { 200 }
    fn paths(&self) -> &'static [AssemblyPath] { &[AssemblyPath::Context] }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        if let Some(ref runtime_ctx) = ctx.runtime_context {
            output.push_str(&runtime_ctx.to_prompt_section());
            output.push_str("\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    #[test]
    fn test_skipped_when_no_runtime_context() {
        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        RuntimeContextLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }

    #[test]
    fn test_injected_when_present() {
        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let mut ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        ctx.runtime_context = Some(crate::thinker::runtime_context::RuntimeContext {
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            shell: "zsh".to_string(),
            working_dir: std::path::PathBuf::from("/workspace"),
            repo_root: None,
            current_model: "claude-opus-4-6".to_string(),
            hostname: "test".to_string(),
        });
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        RuntimeContextLayer.inject(&mut output, &input);
        assert!(output.contains("## Runtime Environment"));
        assert!(output.contains("os=macos"));
    }
}
```

**Step 2: Create `layers/environment.rs`**

```rust
//! EnvironmentLayer — channel paradigm, capabilities, constraints (priority 300)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct EnvironmentLayer;

impl PromptLayer for EnvironmentLayer {
    fn name(&self) -> &'static str { "environment" }
    fn priority(&self) -> u32 { 300 }
    fn paths(&self) -> &'static [AssemblyPath] { &[AssemblyPath::Context] }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        let contract = &ctx.environment_contract;

        output.push_str("## Environment Contract\n\n");
        output.push_str(&format!("**Paradigm**: {}\n\n", contract.paradigm.description()));

        if !contract.active_capabilities.is_empty() {
            output.push_str("**Active Capabilities**:\n");
            for cap in &contract.active_capabilities {
                let (name, hint) = cap.prompt_hint();
                output.push_str(&format!("- `{}`: {}\n", name, hint));
            }
            output.push('\n');
        }

        let mut constraint_notes = Vec::new();
        if let Some(max_chars) = contract.constraints.max_output_chars {
            constraint_notes.push(format!("Max output: {} characters", max_chars));
        }
        if contract.constraints.prefer_compact {
            constraint_notes.push("Prefer concise responses".to_string());
        }
        if contract.constraints.supports_streaming {
            constraint_notes.push("Streaming enabled".to_string());
        }

        if !constraint_notes.is_empty() {
            output.push_str("**Constraints**:\n");
            for note in constraint_notes {
                output.push_str(&format!("- {}\n", note));
            }
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    #[test]
    fn test_environment_layer_content() {
        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        EnvironmentLayer.inject(&mut output, &input);
        assert!(output.contains("## Environment Contract"));
        assert!(output.contains("**Paradigm**"));
    }
}
```

**Step 3: Create `layers/security.rs`**

```rust
//! SecurityLayer — blocked/approval-required tools (priority 600)

use crate::thinker::context::DisableReason;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct SecurityLayer;

impl PromptLayer for SecurityLayer {
    fn name(&self) -> &'static str { "security" }
    fn priority(&self) -> u32 { 600 }
    fn paths(&self) -> &'static [AssemblyPath] { &[AssemblyPath::Context] }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };

        let disabled_tools = &ctx.disabled_tools;
        let security_notes = &ctx.environment_contract.security_notes;

        if security_notes.is_empty() && disabled_tools.is_empty() {
            return;
        }

        output.push_str("## Security & Constraints\n\n");

        for note in security_notes {
            output.push_str(&format!("- {}\n", note));
        }
        if !security_notes.is_empty() {
            output.push('\n');
        }

        let blocked_by_policy: Vec<_> = disabled_tools
            .iter()
            .filter(|d| matches!(d.reason, DisableReason::BlockedByPolicy { .. }))
            .collect();

        if !blocked_by_policy.is_empty() {
            output.push_str("**Disabled by Policy**:\n");
            for tool in blocked_by_policy {
                if let DisableReason::BlockedByPolicy { ref reason } = tool.reason {
                    output.push_str(&format!("- `{}` — {}\n", tool.name, reason));
                }
            }
            output.push('\n');
        }

        let requires_approval: Vec<_> = disabled_tools
            .iter()
            .filter(|d| matches!(d.reason, DisableReason::RequiresApproval { .. }))
            .collect();

        if !requires_approval.is_empty() {
            output.push_str("**Requires User Approval**:\n");
            for tool in requires_approval {
                if let DisableReason::RequiresApproval { prompt: ref approval_prompt } = tool.reason {
                    output.push_str(&format!(
                        "- `{}` — available, but each invocation requires user confirmation ({})\n",
                        tool.name, approval_prompt
                    ));
                }
            }
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    #[test]
    fn test_skipped_when_no_constraints() {
        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        SecurityLayer.inject(&mut output, &input);
        // Permissive security with no tools = nothing disabled
        assert!(output.is_empty() || !output.contains("Disabled by Policy"));
    }
}
```

**Step 4: Create `layers/protocol_tokens.rs`**

```rust
//! ProtocolTokensLayer — structured LLM response tokens for background mode (priority 700)

use crate::thinker::interaction::Capability;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct ProtocolTokensLayer;

impl PromptLayer for ProtocolTokensLayer {
    fn name(&self) -> &'static str { "protocol_tokens" }
    fn priority(&self) -> u32 { 700 }
    fn paths(&self) -> &'static [AssemblyPath] { &[AssemblyPath::Context] }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        if !ctx.environment_contract.active_capabilities.contains(&Capability::SilentReply) {
            return;
        }
        output.push_str(&crate::thinker::protocol_tokens::ProtocolToken::to_prompt_section());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    #[test]
    fn test_injected_in_background_mode() {
        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::Background);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        ProtocolTokensLayer.inject(&mut output, &input);
        assert!(output.contains("ALEPH_HEARTBEAT_OK"));
    }

    #[test]
    fn test_skipped_in_webrich_mode() {
        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::WebRich);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        ProtocolTokensLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }
}
```

**Step 5: Create `layers/operational_guidelines.rs`**

```rust
//! OperationalGuidelinesLayer — system health monitoring for Background/CLI (priority 800)

use crate::thinker::interaction::InteractionParadigm;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

pub struct OperationalGuidelinesLayer;

impl PromptLayer for OperationalGuidelinesLayer {
    fn name(&self) -> &'static str { "operational_guidelines" }
    fn priority(&self) -> u32 { 800 }
    fn paths(&self) -> &'static [AssemblyPath] { &[AssemblyPath::Context] }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };

        match ctx.environment_contract.paradigm {
            InteractionParadigm::Background | InteractionParadigm::CLI => {}
            _ => return,
        }

        output.push_str("## System Operational Awareness\n\n");
        output.push_str("You are aware of your own runtime environment and can monitor it proactively.\n\n");

        output.push_str("### Diagnostic Capabilities (read-only, always allowed)\n");
        output.push_str("- Check disk space: `df -h`\n");
        output.push_str("- Check memory usage: `vm_stat` / `free -h`\n");
        output.push_str("- Check running Aleph processes: `ps aux | grep aleph`\n");
        output.push_str("- Check configuration validity: read config files and validate structure\n");
        output.push_str("- Check Desktop Bridge status: query UDS socket availability\n");
        output.push_str("- Check LanceDB health: verify database file accessibility\n\n");

        output.push_str("### When You Detect Issues\n");
        output.push_str("If you notice configuration conflicts, database issues, disconnected bridges,\n");
        output.push_str("abnormal resource usage, or runtime capability degradation:\n\n");
        output.push_str("**Action**: Report to the user with:\n");
        output.push_str("1. What you observed (specific evidence)\n");
        output.push_str("2. Potential impact\n");
        output.push_str("3. Suggested remediation steps\n");
        output.push_str("4. Do NOT execute remediation without explicit user approval\n\n");

        output.push_str("### What You Must NEVER Do Autonomously\n");
        output.push_str("- Restart Aleph services\n");
        output.push_str("- Modify configuration files\n");
        output.push_str("- Delete or compact databases\n");
        output.push_str("- Kill processes\n");
        output.push_str("- Change system settings\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::context::ContextAggregator;
    use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
    use crate::thinker::security_context::SecurityContext;

    #[test]
    fn test_injected_in_background() {
        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::Background);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        OperationalGuidelinesLayer.inject(&mut output, &input);
        assert!(output.contains("System Operational Awareness"));
        assert!(output.contains("Diagnostic Capabilities"));
    }

    #[test]
    fn test_skipped_in_messaging() {
        let config = PromptConfig::default();
        let interaction = InteractionManifest::new(InteractionParadigm::Messaging);
        let security = SecurityContext::permissive();
        let ctx = ContextAggregator::resolve(&interaction, &security, &[]);
        let input = LayerInput::context(&config, &ctx);
        let mut output = String::new();
        OperationalGuidelinesLayer.inject(&mut output, &input);
        assert!(output.is_empty());
    }
}
```

**Step 6: Update `layers/mod.rs` with all 5 new modules, verify, commit**

Run: `cargo test -p alephcore --lib thinker::layers`

Expected: All layer tests PASS (total ~30+ tests across all layer files).

```bash
git add core/src/thinker/layers/
git commit -m "thinker: add context-dependent layers — RuntimeContext, Environment, Security, ProtocolTokens, OperationalGuidelines"
```

---

### Task 8: Wire Pipeline — Register All Layers + default_layers()

**Files:**
- Modify: `core/src/thinker/prompt_pipeline.rs`
- Modify: `core/src/thinker/layers/mod.rs`

**Step 1: Finalize `layers/mod.rs` with all 20 layers re-exported**

```rust
//! Prompt layers — each file implements one PromptLayer.

pub mod role;
pub mod guidelines;
pub mod special_actions;
pub mod citation_standards;
pub mod generation_models;
pub mod runtime_capabilities;
pub mod custom_instructions;
pub mod language;
pub mod skill_instructions;
pub mod skill_mode;
pub mod thinking_guidance;
pub mod response_format;
pub mod soul;
pub mod tools;
pub mod runtime_context;
pub mod environment;
pub mod security;
pub mod protocol_tokens;
pub mod operational_guidelines;

pub use role::RoleLayer;
pub use guidelines::GuidelinesLayer;
pub use special_actions::SpecialActionsLayer;
pub use citation_standards::CitationStandardsLayer;
pub use generation_models::GenerationModelsLayer;
pub use runtime_capabilities::RuntimeCapabilitiesLayer;
pub use custom_instructions::CustomInstructionsLayer;
pub use language::LanguageLayer;
pub use skill_instructions::SkillInstructionsLayer;
pub use skill_mode::SkillModeLayer;
pub use thinking_guidance::ThinkingGuidanceLayer;
pub use response_format::ResponseFormatLayer;
pub use soul::SoulLayer;
pub use tools::{ToolsLayer, HydratedToolsLayer};
pub use runtime_context::RuntimeContextLayer;
pub use environment::EnvironmentLayer;
pub use security::SecurityLayer;
pub use protocol_tokens::ProtocolTokensLayer;
pub use operational_guidelines::OperationalGuidelinesLayer;
```

**Step 2: Add `default_layers()` to `prompt_pipeline.rs`**

```rust
use super::layers::*;

impl PromptPipeline {
    /// Register all 20 default layers, sorted by priority.
    pub fn default_layers() -> Self {
        Self::new(vec![
            Box::new(SoulLayer),                    // 50
            Box::new(RoleLayer),                    // 100
            Box::new(RuntimeContextLayer),          // 200
            Box::new(EnvironmentLayer),             // 300
            Box::new(RuntimeCapabilitiesLayer),     // 400
            Box::new(ToolsLayer),                   // 500
            Box::new(HydratedToolsLayer),           // 500
            Box::new(SecurityLayer),                // 600
            Box::new(ProtocolTokensLayer),          // 700
            Box::new(OperationalGuidelinesLayer),   // 800
            Box::new(CitationStandardsLayer),       // 900
            Box::new(GenerationModelsLayer),        // 1000
            Box::new(SkillInstructionsLayer),       // 1050
            Box::new(SpecialActionsLayer),          // 1100
            Box::new(ResponseFormatLayer),          // 1200
            Box::new(GuidelinesLayer),              // 1300
            Box::new(ThinkingGuidanceLayer),        // 1350
            Box::new(SkillModeLayer),               // 1400
            Box::new(CustomInstructionsLayer),      // 1500
            Box::new(LanguageLayer),                // 1600
        ])
    }
}
```

**Step 3: Add pipeline-level test**

```rust
#[test]
fn test_default_layers_count() {
    let pipeline = PromptPipeline::default_layers();
    assert_eq!(pipeline.layer_count(), 20);
}

#[test]
fn test_default_layers_sorted() {
    let pipeline = PromptPipeline::default_layers();
    // Access internal layers for testing
    let priorities: Vec<u32> = pipeline.layers.iter().map(|l| l.priority()).collect();
    assert!(priorities.windows(2).all(|w| w[0] <= w[1]), "Layers must be sorted by priority");
}
```

**Step 4: Verify and commit**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline`

```bash
git add core/src/thinker/prompt_pipeline.rs core/src/thinker/layers/mod.rs
git commit -m "thinker: register all 20 layers in default_layers() pipeline"
```

---

### Task 9: Wire Pipeline into PromptBuilder — Replace Build Methods

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs`

This is the critical step. Replace each build method's internal implementation to delegate to Pipeline.

**Step 1: Add Pipeline field to PromptBuilder and replace `build_system_prompt()`**

In `prompt_builder.rs`, change:

```rust
use super::prompt_pipeline::PromptPipeline;
use super::prompt_layer::{AssemblyPath, LayerInput};

pub struct PromptBuilder {
    config: PromptConfig,
    pipeline: PromptPipeline,
}

impl PromptBuilder {
    pub fn new(config: PromptConfig) -> Self {
        let pipeline = PromptPipeline::default_layers();
        Self { config, pipeline }
    }

    pub fn build_system_prompt(&self, tools: &[ToolInfo]) -> String {
        let input = LayerInput::basic(&self.config, tools);
        self.pipeline.execute(AssemblyPath::Basic, &input)
    }
```

**Step 2: Verify Basic path**

Run: `cargo test -p alephcore --lib thinker::prompt_builder`

Expected: All existing tests for `build_system_prompt` still PASS. Key tests:
- `test_thinking_guidance_disabled_by_default`
- `test_thinking_guidance_enabled`

**Step 3: Replace `build_system_prompt_cached()`**

```rust
    pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
        let header = Self::build_static_header();
        let input = LayerInput::basic(&self.config, tools);
        let dynamic = self.pipeline.execute(AssemblyPath::Cached, &input);
        vec![
            SystemPromptPart { content: header, cache: true },
            SystemPromptPart { content: dynamic, cache: false },
        ]
    }
```

Keep `build_static_header()` as a private method (unchanged).

**Step 4: Replace `build_system_prompt_with_hydration()`**

```rust
    pub fn build_system_prompt_with_hydration(&self, hydration: &HydrationResult) -> String {
        let input = LayerInput::hydration(&self.config, hydration);
        self.pipeline.execute(AssemblyPath::Hydration, &input)
    }
```

**Step 5: Replace `build_system_prompt_with_soul()`**

```rust
    pub fn build_system_prompt_with_soul(&self, tools: &[ToolInfo], soul: &SoulManifest) -> String {
        let input = LayerInput::soul(&self.config, tools, soul);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }
```

**Step 6: Verify Soul path tests**

Run: `cargo test -p alephcore --lib thinker::prompt_builder`

Key tests:
- `test_build_system_prompt_with_soul` — Identity before Role
- `test_soul_section_with_expertise`
- `test_thinking_guidance_with_soul`

**Step 7: Replace `build_system_prompt_with_context()`**

```rust
    pub fn build_system_prompt_with_context(&self, ctx: &ResolvedContext) -> String {
        let input = LayerInput::context(&self.config, ctx);
        self.pipeline.execute(AssemblyPath::Context, &input)
    }
```

**Step 8: Verify all Context path tests**

Run: `cargo test -p alephcore --lib thinker::prompt_builder`

Key tests:
- `test_build_system_prompt_with_context_includes_runtime_context`
- `test_full_prompt_with_all_enhancements_background_mode` — ordering verification
- `test_interactive_prompt_minimal_token_overhead` — WebRich exclusion checks

**Step 9: Verify full test suite**

Run: `cargo test -p alephcore`

Expected: ALL tests PASS.

**Step 10: Commit**

```bash
git add core/src/thinker/prompt_builder.rs
git commit -m "thinker: wire PromptPipeline into all 5 build methods"
```

---

### Task 10: Cleanup — Delete Migrated Methods, Slim Down prompt_builder.rs

**Files:**
- Modify: `core/src/thinker/prompt_builder.rs`

**Step 1: Delete all migrated `append_*` methods**

Remove these methods from `prompt_builder.rs` (they are now in `layers/`):
- `append_runtime_capabilities()`
- `append_runtime_context_section()`
- `append_tools()`
- `append_hydrated_tools()`
- `append_generation_models()`
- `append_special_actions()`
- `append_response_format()`
- `append_guidelines()`
- `append_thinking_guidance()`
- `append_skill_mode()`
- `append_skill_instructions()`
- `append_custom_instructions()`
- `append_language_setting()`
- `append_soul_section()`
- `append_environment_contract()`
- `append_security_constraints()`
- `append_silent_behavior()`
- `append_protocol_tokens()`
- `append_operational_guidelines()`
- `append_citation_standards()`
- `build_dynamic_content()`

Keep:
- `SystemPromptPart`, `PromptConfig`, `PromptBuilder` structs
- 5 public `build_*` methods (now thin delegates)
- `build_static_header()` (private, used by cached path)
- `build_messages()`, `build_observation()`
- `Message`, `MessageRole` types and impls
- `truncate_str()`, `format_attachment()` helpers

**Step 2: Delete redundant unit tests**

Remove tests from `prompt_builder.rs` that are now covered by Layer unit tests:
- `test_append_soul_section_empty` → `layers/soul.rs`
- `test_append_soul_section_basic` → `layers/soul.rs`
- `test_soul_section_with_expertise` → `layers/soul.rs`
- `test_append_runtime_context_section` → `layers/runtime_context.rs`
- `test_append_protocol_tokens_with_silent_reply` → `layers/protocol_tokens.rs`
- `test_append_protocol_tokens_without_silent_reply` → `layers/protocol_tokens.rs`
- `test_append_operational_guidelines_background` → `layers/operational_guidelines.rs`
- `test_append_operational_guidelines_cli` → `layers/operational_guidelines.rs`
- `test_append_operational_guidelines_messaging_skipped` → `layers/operational_guidelines.rs`
- `test_append_citation_standards` → `layers/citation_standards.rs`

Keep integration tests (they test the public API via Pipeline):
- `test_build_system_prompt_with_soul`
- `test_thinking_guidance_disabled_by_default`
- `test_thinking_guidance_enabled`
- `test_thinking_guidance_with_soul`
- `test_build_system_prompt_with_context_includes_runtime_context`
- `test_build_system_prompt_with_context_no_runtime_context`
- `test_full_prompt_with_all_enhancements_background_mode`
- `test_interactive_prompt_minimal_token_overhead`

**Step 3: Verify**

Run: `cargo test -p alephcore`

Expected: ALL tests PASS. No test coverage regression.

Run: `cargo clippy -p alephcore -- -W clippy::all`

Expected: No warnings.

**Step 4: Verify line count**

Run: `wc -l core/src/thinker/prompt_builder.rs`

Expected: ~350 lines (down from 1724).

**Step 5: Commit**

```bash
git add core/src/thinker/prompt_builder.rs
git commit -m "thinker: cleanup — delete migrated append methods, slim prompt_builder to ~350 lines"
```

---

### Final Verification

Run the complete test suite:

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore
```

Expected: All tests pass. The refactoring is purely structural — no behavior changes.
