# Model Behavior System Design

**Date**: 2026-03-28
**Status**: Draft
**Author**: Brainstorming session

## Problem

Different LLMs respond to the same system prompt with vastly different behaviors. Claude is RLHF-tuned toward proactive tool calling and task execution; GPT-5.4 tends to hesitate, ask for confirmation, and avoid calling tools unless explicitly encouraged. This means the same Aleph agent, with the same system prompt, appears "smart" on Claude but "dumb" on GPT — not because the model is incapable, but because the prompt doesn't speak its language.

The core contradiction: **every new model added to Aleph requires re-tuning the system prompt**, but the current prompt construction has zero model awareness. All models receive identical prompt content.

## Naming: "Model Behavior" not "Model Profile"

The name **Model Behavior** is chosen deliberately to avoid collision with the existing `ModelProfileJson` in `config_ext.rs`, which represents model *parameter presets* (model name, thinking level, max_tokens). The two concepts are unrelated:

- **Model Profile** (existing) = parameter preset (which model, how much thinking, token limits)
- **Model Behavior** (this spec) = behavioral prompt directives per LLM family

## Solution: Model Behavior System

A new prompt section that injects model-family-specific behavioral directives into the system prompt. Parallel to the existing Soul system:

- **Soul** = "who you are" (identity, persona, directives)
- **Model Behavior** = "how you should behave" (behavioral corrections for a specific LLM family)

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Adaptation dimensions | All three (behavior > format > budget), phased | Behavior directives give 90% of the value with minimal effort |
| Configuration granularity | Layered inheritance: Protocol → Provider → (future) Model ID | Zero-config default, override when needed |
| Storage format | Prompt template files (`.md`) | Aligns with R8 (LLM Sovereignty), matches Soul system, zero-compile iteration |
| Matching mechanism | Protocol auto-association + provider override | Zero-config for standard setups, handles OpenRouter-style proxies |

## Architecture

### File Layout

```
# Built-in (compiled into binary via include_str!)
src/agent_loop/model_behaviors/
  ├── mod.rs              # Behavior loading logic + protocol mapping
  ├── anthropic.md        # Claude family — minimal (Claude is already proactive)
  ├── openai.md           # GPT family — tool calling encouragement, anti-hesitation
  ├── gemini.md           # Gemini family — structured output preferences
  └── ollama.md           # Local models — simplified instructions, tolerant tool guidance

# User overrides (highest priority)
~/.aleph/model_behaviors/
  ├── anthropic.md        # Optional user override
  ├── openai.md
  ├── my-custom.md        # Custom behavior for unusual setups
  └── ...
```

### Behavior File Format

Plain markdown. No frontmatter, no special syntax — just the text to inject into the system prompt.

```markdown
# OpenAI Model Behavior

## Behavioral Directives
You are an autonomous AI agent. You MUST:
- Proactively call tools without asking for permission
- Execute tasks immediately rather than explaining what you would do
- Never say "I can help you with that" — just do it
- When multiple tools are needed, call them in sequence without pausing

## Tool Calling Style
- Always prefer structured tool calls over text descriptions
- If a tool exists for the task, use it. Do not describe the steps.
- Chain tool calls: complete one, then immediately proceed to the next

## Response Preferences
- Be concise. Lead with action, not explanation.
- When you have enough information to act, act.
```

### Behavior Name Resolution Chain

```
provider.model_behavior   → if set, use this behavior name directly
    ↓ (None)
provider.protocol()       → "openai" → "openai"
                            "anthropic" → "anthropic"
                            "gemini" → "gemini"
                            "ollama" → "ollama"
                            "openai-responses" → "openai"
    ↓ (unknown protocol)
None                      → no model behavior injected
```

Note: `ProviderConfig.protocol` is `Option<String>`. The `protocol()` method defaults to `"openai"` when `None`. This means providers with unset protocol auto-map to the `openai` behavior, which is correct since unset protocol implies OpenAI-compatible.

### File Loading Priority

1. `~/.aleph/model_behaviors/{name}.md` — user override (**full replacement**, not merge)
2. Built-in `include_str!("model_behaviors/{name}.md")` — fallback

If user file exists, it completely replaces the built-in behavior. This keeps the mental model simple: one file = one behavior, no merge confusion.

Note: `dirs::home_dir()` may return `None` on unusual systems. In that case, the user override path is silently skipped and the built-in fallback is used.

## Implementation

### Integration Point: `agent_loop::PromptBuilder`

**Critical**: The actual execution path uses `agent_loop::PromptBuilder` (in `src/agent_loop/prompt_builder.rs`), NOT the `thinker::PromptPipeline`. The `run_loop.rs` imports `PromptBuilder` from `agent_loop` and calls its `build()` method, which assembles sections by simple concatenation.

The `thinker::PromptPipeline` with its 28 layers is a separate system. This spec targets `agent_loop::PromptBuilder` as the primary integration point.

### 1. Model Behavior Loading Module

```rust
// src/agent_loop/model_behaviors/mod.rs

const BUILTIN_ANTHROPIC: &str = include_str!("anthropic.md");
const BUILTIN_OPENAI: &str = include_str!("openai.md");
const BUILTIN_GEMINI: &str = include_str!("gemini.md");
const BUILTIN_OLLAMA: &str = include_str!("ollama.md");

/// Load model behavior content by name.
///
/// Checks user override at `~/.aleph/model_behaviors/{name}.md` first,
/// falls back to built-in content.
pub fn load_model_behavior(name: &str) -> Option<String> {
    // 1. Check user override (silently skip if home_dir unavailable)
    if let Some(home) = dirs::home_dir() {
        let user_path = home.join(".aleph/model_behaviors").join(format!("{name}.md"));
        if let Ok(content) = std::fs::read_to_string(&user_path) {
            return Some(content);
        }
    }

    // 2. Fallback to built-in
    match name {
        "anthropic" => Some(BUILTIN_ANTHROPIC.to_string()),
        "openai" => Some(BUILTIN_OPENAI.to_string()),
        "gemini" => Some(BUILTIN_GEMINI.to_string()),
        "ollama" => Some(BUILTIN_OLLAMA.to_string()),
        _ => None,  // Unknown behavior — no injection
    }
}

/// Map protocol name to default behavior name.
pub fn protocol_to_behavior(protocol: &str) -> Option<&'static str> {
    match protocol {
        "anthropic" => Some("anthropic"),
        "openai" => Some("openai"),
        "openai-responses" => Some("openai"),  // Same behavioral family
        "gemini" => Some("gemini"),
        "ollama" => Some("ollama"),
        _ => None,
    }
}
```

### 2. PromptBuilder Extension

Add a `model_behavior` field to `agent_loop::PromptBuilder`:

```rust
// src/agent_loop/prompt_builder.rs

pub struct PromptBuilder {
    persona_prefix: Option<String>,
    soul_identity: Option<String>,
    soul_tone: Option<String>,
    soul_directives: Vec<String>,
    capability_rules: Option<String>,
    custom_instructions: Option<String>,
    eligible_skills: Option<Vec<SkillManifest>>,
    model_behavior: Option<String>,  // NEW
}

impl PromptBuilder {
    /// Set model behavior content (loaded from model_behaviors/).
    pub fn with_model_behavior(mut self, content: &str) -> Self {
        self.model_behavior = Some(content.to_string());
        self
    }
}
```

### 3. Injection in `build()` Method

Insert model behavior as a new section in the `build()` method, between **Directives (section 3)** and **Tool Usage Rules (section 4)**:

```rust
pub fn build(&self, tools: &[ToolInfo], memory_context: Option<&str>) -> String {
    let mut sections: Vec<String> = Vec::new();

    // 0. Persona prefix
    // 1. Identity
    // 2. Communication Style
    // 3. Directives

    // 3.5 Model Behavior (NEW) — after identity, before tool rules
    if let Some(behavior) = &self.model_behavior {
        sections.push(format!("# Model Behavior\n\n{}", behavior));
    }

    // 4. Tool Usage Rules
    // 5. Available Tools
    // 6. Available Skills
    // 7. Context from Memory
    // 8. Additional Instructions
    // 9. Core Behavior (BASE_BEHAVIOR)
    // ...
}
```

This placement means: "know who you are" → "know how to behave on this LLM" → "know the tool rules".

### 4. ProviderConfig Extension

```rust
// src/config/types/provider.rs

pub struct ProviderConfig {
    // ... existing fields ...
    pub model_behavior: Option<String>,  // NEW: override protocol default behavior name
}
```

### 5. Wiring in run_loop.rs

```rust
// src/gateway/execution_engine/run_loop.rs

use crate::agent_loop::model_behaviors::{load_model_behavior, protocol_to_behavior};

// Resolve behavior name from provider config
let behavior_name = provider_config.model_behavior.as_deref()
    .or_else(|| protocol_to_behavior(provider_config.protocol()));

// Load behavior content
let behavior_content = behavior_name.and_then(load_model_behavior);

// Build prompt with behavior
let mut prompt_builder = PromptBuilder::from_soul(&resolved_soul);
if let Some(content) = &behavior_content {
    prompt_builder = prompt_builder.with_model_behavior(content);
}
```

### 6. Optional: Thinker PromptPipeline Integration

If the `thinker::PromptPipeline` path is also used (e.g., for future features), a `ModelBehaviorLayer` can be added separately. The actual `PromptLayer` trait API:

```rust
// src/thinker/layers/model_behavior.rs (optional, future)

pub struct ModelBehaviorLayer;

impl PromptLayer for ModelBehaviorLayer {
    fn name(&self) -> &'static str { "model_behavior" }
    fn priority(&self) -> u32 { 150 }  // After Profile (75) and Role (100)
    fn paths(&self) -> &'static [AssemblyPath] { &[AssemblyPath::Soul, AssemblyPath::Context] }
    fn stability(&self) -> LayerStability { LayerStability::Stable }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full | PromptMode::Compact)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(name) = input.model_behavior_name {
            if let Some(content) = load_model_behavior(name) {
                output.push_str(&content);
                output.push('\n');
            }
        }
    }
}
```

Note: This is deferred to when/if the PromptPipeline becomes the primary prompt assembly path for agent loops. The `agent_loop::PromptBuilder` integration is the v1 deliverable. When implementing this layer, `LayerInput` must be extended with a `model_behavior_name: Option<&'a str>` field (it does not exist today).

## Data Flow

```
ProviderConfig
  ├── protocol: Some("openai")  // Option<String>, defaults to "openai" via protocol()
  ├── model_behavior: None (or Some("anthropic") for OpenRouter)
  │
  ▼
protocol_to_behavior("openai") → "openai"
  │
  ▼
load_model_behavior("openai")
  ├── Check ~/.aleph/model_behaviors/openai.md → not found
  └── Fallback → include_str!("openai.md") → Some(content)
  │
  ▼
PromptBuilder::from_soul(&soul)
  .with_model_behavior(&content)
  .build(tools, memory)
  │
  ▼
Final system prompt:
  [Persona] + [Identity] + [Style] + [Directives]
  + [>>> Model Behavior <<<]                        ← NEW
  + [Tool Rules] + [Tools] + [Skills] + [Memory]
  + [Instructions] + [Core Behavior]
```

## Built-in Behavior Content

### `anthropic.md` (Claude)

```markdown
<!-- Minimal — Claude's RLHF alignment already favors proactive execution -->
```

Nearly empty. Claude doesn't need behavioral correction. This file exists so the system has a consistent behavior for every protocol, and users can override it if they want to _constrain_ Claude's proactivity.

### `openai.md` (GPT)

```markdown
## Execution Directives

You are an autonomous agent with full tool access. Your primary mode is EXECUTION, not conversation.

Rules:
- ALWAYS call tools proactively. Never ask "would you like me to..." — just do it.
- When you have enough context to act, act immediately. Do not explain what you plan to do.
- Chain multiple tool calls in sequence. Complete one, then proceed to the next without pausing.
- If a task requires information, use tools to get it. Do not ask the user to provide what you can look up.
- Prefer action over explanation. A 3-line response with a tool call beats a 20-line explanation.

Anti-patterns to avoid:
- "I can help you with that! Let me..." → Just call the tool.
- "Would you like me to proceed?" → Proceed.
- "Here's what I would do: 1. ... 2. ... 3. ..." → Do step 1 now.
- Listing steps without executing them → Execute step 1, then step 2, then step 3.
```

### `gemini.md` (Gemini)

```markdown
## Execution Directives

You are an autonomous agent. Act decisively:
- Call tools immediately when the task is clear. Do not narrate your plan first.
- Chain tool calls — finish one, start the next.
- Prefer structured tool responses over free-text descriptions.
- When generating structured data, use the exact schema provided.
```

### `ollama.md` (Local Models)

```markdown
## Tool Usage Guide

You have access to tools. When a task matches an available tool, you MUST use it.

How to use tools:
- Read the tool descriptions carefully
- Call tools with the required parameters
- Wait for the result before proceeding
- If a tool call fails, try again with corrected parameters

Important:
- Always prefer tool calls over text responses when a matching tool exists
- Execute tasks step by step — one tool call at a time
- Be concise in your text responses
```

## Scope & Non-Goals

### In Scope
- Model behavior loading module in `src/agent_loop/model_behaviors/`
- Integration into `agent_loop::PromptBuilder` as new section
- Built-in behavior files for anthropic/openai/gemini/ollama
- User override via `~/.aleph/model_behaviors/{name}.md`
- `ProviderConfig.model_behavior` optional override field
- Protocol → behavior auto-mapping

### Out of Scope (Future)
- `thinker::PromptPipeline` `ModelBehaviorLayer` (deferred until pipeline is primary path)
- Model ID level inheritance (e.g., `gpt-5.4` overriding `openai` defaults)
- Format/structure adaptation (XML vs markdown preference per model)
- Token budget tuning per model (current TokenBudget system is sufficient)
- A/B testing or automatic behavior optimization
- Behavior hot-reload without restart (nice-to-have, not required for v1)
- Profile selection change on provider failover (v1: selection happens once at prompt build time)

## Testing Strategy

1. **Unit tests**: `load_model_behavior()` returns correct content (user override > built-in > None)
2. **Unit tests**: `protocol_to_behavior()` mapping correctness
3. **Unit tests**: `PromptBuilder.build()` includes model behavior section when set
4. **Unit tests**: `PromptBuilder.build()` omits model behavior section when not set
5. **E2E validation**: Run same agent task on Claude and GPT with behaviors, verify GPT proactively calls tools

## Files to Create/Modify

| Action | File | Description |
|--------|------|-------------|
| CREATE | `src/agent_loop/model_behaviors/mod.rs` | Behavior loading logic + protocol mapping |
| CREATE | `src/agent_loop/model_behaviors/anthropic.md` | Built-in Claude behavior |
| CREATE | `src/agent_loop/model_behaviors/openai.md` | Built-in GPT behavior |
| CREATE | `src/agent_loop/model_behaviors/gemini.md` | Built-in Gemini behavior |
| CREATE | `src/agent_loop/model_behaviors/ollama.md` | Built-in local model behavior |
| MODIFY | `src/agent_loop/mod.rs` | Add `pub mod model_behaviors;` |
| MODIFY | `src/agent_loop/prompt_builder.rs` | Add `model_behavior` field + builder method + section in `build()` |
| MODIFY | `src/config/types/provider.rs` | Add `model_behavior: Option<String>` to ProviderConfig |
| MODIFY | `src/gateway/execution_engine/run_loop.rs` | Resolve behavior name, load content, pass to PromptBuilder |

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Behavior content makes model worse | Medium | Low | anthropic.md starts near-empty; iterate based on testing |
| Token budget overflow from long behaviors | Low | Low | Behaviors are short (~200 tokens); well within budget |
| User confusion about override behavior | Low | Low | Full replacement, not merge — simple mental model |
| Breaking existing Claude behavior | Very Low | High | anthropic.md is near-empty, no behavioral change |
| Naming confusion with ModelProfileJson | None | N/A | Resolved by using "Model Behavior" naming |
