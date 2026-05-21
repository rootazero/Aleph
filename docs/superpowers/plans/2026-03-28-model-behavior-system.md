# Model Behavior System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inject LLM-family-specific behavioral directives into system prompts so each model (Claude, GPT, Gemini, Ollama) receives prompt tuning optimized for its behavioral tendencies.

**Architecture:** A `model_behaviors` module under `agent_loop/` provides `load_model_behavior(name)` and `protocol_to_behavior(protocol)`. Built-in `.md` files are compiled via `include_str!`, with user overrides at `~/.aleph/model_behaviors/`. The `PromptBuilder` gains a `model_behavior` field injected between Directives and Tool Rules. The `AiProvider` trait gains a `protocol()` method so `run_loop.rs` can resolve the behavior name.

**Tech Stack:** Rust, `include_str!`, `dirs` crate (already a dependency), existing PromptBuilder pattern.

**Spec:** `docs/superpowers/specs/2026-03-28-model-profile-system-design.md`

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| CREATE | `src/agent_loop/model_behaviors/mod.rs` | `load_model_behavior()`, `protocol_to_behavior()`, built-in `include_str!` constants |
| CREATE | `src/agent_loop/model_behaviors/anthropic.md` | Claude behavior (near-empty) |
| CREATE | `src/agent_loop/model_behaviors/openai.md` | GPT behavior (proactive tool calling) |
| CREATE | `src/agent_loop/model_behaviors/gemini.md` | Gemini behavior (structured output) |
| CREATE | `src/agent_loop/model_behaviors/ollama.md` | Local model behavior (simplified tool guidance) |
| MODIFY | `src/agent_loop/mod.rs` | Add `pub mod model_behaviors;` |
| MODIFY | `src/agent_loop/prompt_builder.rs` | Add `model_behavior` field, builder method, section in `build()` |
| MODIFY | `src/providers/mod.rs` | Add `fn protocol()` to `AiProvider` trait |
| MODIFY | `src/providers/http_provider.rs` | Override `protocol()` to return adapter name |
| MODIFY | `src/providers/ollama.rs` | Override `protocol()` to return `"ollama"` |
| MODIFY | `src/config/types/provider.rs` | Add `model_behavior: Option<String>` to `ProviderConfig` |
| MODIFY | `src/gateway/execution_engine/run_loop.rs` | Resolve behavior, pass to PromptBuilder |

---

### Task 1: Create built-in behavior `.md` files

**Files:**
- Create: `src/agent_loop/model_behaviors/anthropic.md`
- Create: `src/agent_loop/model_behaviors/openai.md`
- Create: `src/agent_loop/model_behaviors/gemini.md`
- Create: `src/agent_loop/model_behaviors/ollama.md`

- [ ] **Step 1: Create the directory**

```bash
mkdir -p src/agent_loop/model_behaviors
```

- [ ] **Step 2: Create `anthropic.md`**

```markdown
<!-- Minimal — Claude's RLHF alignment already favors proactive execution -->
```

- [ ] **Step 3: Create `openai.md`**

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

- [ ] **Step 4: Create `gemini.md`**

```markdown
## Execution Directives

You are an autonomous agent. Act decisively:
- Call tools immediately when the task is clear. Do not narrate your plan first.
- Chain tool calls — finish one, start the next.
- Prefer structured tool responses over free-text descriptions.
- When generating structured data, use the exact schema provided.
```

- [ ] **Step 5: Create `ollama.md`**

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

- [ ] **Step 6: Commit**

```bash
git add src/agent_loop/model_behaviors/
git commit -m "model_behaviors: add built-in behavior files for 4 LLM families"
```

---

### Task 2: Create the `model_behaviors` module with loading logic and tests

**Files:**
- Create: `src/agent_loop/model_behaviors/mod.rs`
- Modify: `src/agent_loop/mod.rs` (line 9, add module declaration)

- [ ] **Step 1: Write tests first in `mod.rs`**

Create `src/agent_loop/model_behaviors/mod.rs` with the tests at the bottom:

```rust
//! Model behavior loading — per-LLM-family behavioral directives.
//!
//! Built-in `.md` files are compiled into the binary. User overrides
//! at `~/.aleph/model_behaviors/{name}.md` take full precedence.

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
    builtin_behavior(name)
}

/// Get built-in behavior content (no user override check).
fn builtin_behavior(name: &str) -> Option<String> {
    match name {
        "anthropic" => Some(BUILTIN_ANTHROPIC.to_string()),
        "openai" => Some(BUILTIN_OPENAI.to_string()),
        "gemini" => Some(BUILTIN_GEMINI.to_string()),
        "ollama" => Some(BUILTIN_OLLAMA.to_string()),
        _ => None,
    }
}

/// Map protocol name to default behavior name.
pub fn protocol_to_behavior(protocol: &str) -> Option<&'static str> {
    match protocol {
        "anthropic" => Some("anthropic"),
        "openai" => Some("openai"),
        "openai-responses" => Some("openai"),
        "gemini" => Some("gemini"),
        "ollama" => Some("ollama"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_anthropic_loads() {
        let content = builtin_behavior("anthropic").unwrap();
        // anthropic.md is near-empty (just an HTML comment)
        assert!(!content.is_empty());
    }

    #[test]
    fn test_builtin_openai_loads() {
        let content = builtin_behavior("openai").unwrap();
        assert!(content.contains("Execution Directives"));
        assert!(content.contains("ALWAYS call tools proactively"));
    }

    #[test]
    fn test_builtin_gemini_loads() {
        let content = builtin_behavior("gemini").unwrap();
        assert!(content.contains("Execution Directives"));
    }

    #[test]
    fn test_builtin_ollama_loads() {
        let content = builtin_behavior("ollama").unwrap();
        assert!(content.contains("Tool Usage Guide"));
    }

    #[test]
    fn test_builtin_unknown_returns_none() {
        assert!(builtin_behavior("unknown-model").is_none());
    }

    #[test]
    fn test_load_falls_back_to_builtin() {
        // No user override exists, so should return built-in
        let content = load_model_behavior("openai").unwrap();
        assert!(content.contains("Execution Directives"));
    }

    #[test]
    fn test_load_unknown_returns_none() {
        assert!(load_model_behavior("nonexistent").is_none());
    }

    #[test]
    fn test_protocol_to_behavior_openai() {
        assert_eq!(protocol_to_behavior("openai"), Some("openai"));
    }

    #[test]
    fn test_protocol_to_behavior_openai_responses() {
        assert_eq!(protocol_to_behavior("openai-responses"), Some("openai"));
    }

    #[test]
    fn test_protocol_to_behavior_anthropic() {
        assert_eq!(protocol_to_behavior("anthropic"), Some("anthropic"));
    }

    #[test]
    fn test_protocol_to_behavior_gemini() {
        assert_eq!(protocol_to_behavior("gemini"), Some("gemini"));
    }

    #[test]
    fn test_protocol_to_behavior_ollama() {
        assert_eq!(protocol_to_behavior("ollama"), Some("ollama"));
    }

    #[test]
    fn test_protocol_to_behavior_unknown() {
        assert_eq!(protocol_to_behavior("some-custom-protocol"), None);
    }
}
```

- [ ] **Step 2: Register the module in `agent_loop/mod.rs`**

Add after line 8 (`mod prompt_builder;`):

```rust
pub mod model_behaviors;
```

- [ ] **Step 3: Run tests to verify they pass**

```bash
cargo test -p alephcore --lib model_behaviors
```

Expected: All 12 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/model_behaviors/mod.rs src/agent_loop/mod.rs
git commit -m "model_behaviors: add loading module with protocol mapping and tests"
```

---

### Task 3: Extend `PromptBuilder` with `model_behavior` field

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of `prompt_builder.rs`:

```rust
    #[test]
    fn test_build_includes_model_behavior() {
        let prompt = PromptBuilder::new()
            .with_model_behavior("You MUST call tools proactively.")
            .build(&[], None);

        assert!(prompt.contains("# Model Behavior"));
        assert!(prompt.contains("You MUST call tools proactively."));
    }

    #[test]
    fn test_build_omits_model_behavior_when_not_set() {
        let prompt = PromptBuilder::new().build(&[], None);
        assert!(!prompt.contains("# Model Behavior"));
    }

    #[test]
    fn test_model_behavior_appears_before_tool_rules() {
        let prompt = PromptBuilder::new()
            .with_model_behavior("Be proactive.")
            .with_capability_rules("Always confirm.")
            .build(&[], None);

        let behavior_pos = prompt.find("# Model Behavior").unwrap();
        let rules_pos = prompt.find("# Tool Usage Rules").unwrap();
        assert!(behavior_pos < rules_pos, "Model Behavior should appear before Tool Usage Rules");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p alephcore --lib prompt_builder::tests::test_build_includes_model_behavior
```

Expected: FAIL — `with_model_behavior` method does not exist.

- [ ] **Step 3: Add `model_behavior` field to the struct**

In `prompt_builder.rs`, add to the `PromptBuilder` struct (after `eligible_skills` field, line 77):

```rust
    model_behavior: Option<String>,
```

- [ ] **Step 4: Initialize field in `new()`**

In the `new()` method, add after `eligible_skills: None,` (line 127):

```rust
            model_behavior: None,
```

- [ ] **Step 5: Add builder method**

After the `with_eligible_skills` method (after line 172):

```rust
    /// Set model behavior content (loaded from model_behaviors/).
    pub fn with_model_behavior(mut self, content: &str) -> Self {
        self.model_behavior = Some(content.to_string());
        self
    }
```

- [ ] **Step 6: Add section to `build()` method**

In the `build()` method, add after the Directives section (section 3, after line 207) and before the Tool Usage Rules section (section 4, line 209):

```rust
        // 3.5 Model Behavior — LLM-family-specific behavioral directives
        if let Some(behavior) = &self.model_behavior {
            sections.push(format!("# Model Behavior\n\n{}", behavior));
        }
```

- [ ] **Step 7: Run all prompt_builder tests**

```bash
cargo test -p alephcore --lib prompt_builder
```

Expected: All tests pass (existing + 3 new).

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "prompt_builder: add model_behavior section between directives and tool rules"
```

---

### Task 4: Add `protocol()` to `AiProvider` trait

**Files:**
- Modify: `src/providers/mod.rs` (AiProvider trait)
- Modify: `src/providers/http_provider.rs` (HttpProvider impl)
- Modify: `src/providers/ollama.rs` (OllamaProvider impl)

- [ ] **Step 1: Add default method to `AiProvider` trait**

In `src/providers/mod.rs`, add to the `AiProvider` trait (after the `supports_thinking` method, around line 222):

```rust
    /// Protocol name for model behavior resolution.
    ///
    /// Returns the protocol identifier (e.g., "openai", "anthropic", "gemini", "ollama")
    /// used to select appropriate model behavior directives.
    fn protocol(&self) -> &str {
        "unknown"
    }
```

- [ ] **Step 2: Override in `HttpProvider`**

In `src/providers/http_provider.rs`, add to the `impl AiProvider for HttpProvider` block (after `fn color()`, around line 220):

```rust
    fn protocol(&self) -> &str {
        self.adapter.name()
    }
```

- [ ] **Step 3: Check adapter `name()` returns protocol names**

Verify by searching: `fn name(&self)` in the protocol adapter implementations. These should return "openai", "anthropic", "gemini", "openai-responses" etc. (This is a read-only verification step.)

```bash
cargo test -p alephcore --lib -- --list 2>&1 | head -5
```

Verify it compiles.

- [ ] **Step 4: Override in `OllamaProvider`**

In `src/providers/ollama.rs`, add to the `impl AiProvider for OllamaProvider` block (after `fn name()` or `fn color()`):

```rust
    fn protocol(&self) -> &str {
        "ollama"
    }
```

- [ ] **Step 5: Compile check**

```bash
cargo check -p alephcore
```

Expected: No errors.

- [ ] **Step 6: Commit**

```bash
git add src/providers/mod.rs src/providers/http_provider.rs src/providers/ollama.rs
git commit -m "providers: add protocol() to AiProvider trait for behavior resolution"
```

---

### Task 5: Add `model_behavior` field to `ProviderConfig`

**Files:**
- Modify: `src/config/types/provider.rs`

- [ ] **Step 1: Add the field**

In `ProviderConfig` struct, add after the `system_prompt_mode` field (after line 99):

```rust
    /// Model behavior override: use a specific behavior file instead of protocol default.
    /// Maps to a file in `~/.aleph/model_behaviors/{name}.md` or a built-in behavior.
    /// Example: Set to "anthropic" on an OpenRouter provider that routes to Claude.
    /// TODO: Wire this field into run_loop.rs behavior resolution (currently uses AiProvider::protocol() only).
    #[serde(default)]
    pub model_behavior: Option<String>,
```

- [ ] **Step 2: Update `test_config()` helper**

In the `test_config()` method, add after `system_prompt_mode: None,` (line 171):

```rust
            model_behavior: None,
```

- [ ] **Step 3: Update the two full-struct test configs**

In `test_protocol_without_provider_type` and `test_protocol_defaults_to_openai`, add `model_behavior: None,` after each `system_prompt_mode: None,` line.

- [ ] **Step 4: Compile check**

```bash
cargo check -p alephcore
```

Expected: No errors. (If other files construct `ProviderConfig` explicitly, they'll need the new field too — `cargo check` will find them.)

- [ ] **Step 5: Fix any remaining compile errors**

If `cargo check` reports missing `model_behavior` field in other files, add `model_behavior: None,` to each. Common locations:
- `src/providers/mod.rs` (test helpers)
- `src/gateway/handlers/config_ext.rs`
- Any integration test files

- [ ] **Step 6: Run existing tests**

```bash
cargo test -p alephcore --lib provider
```

Expected: All existing provider tests still pass.

- [ ] **Step 7: Commit**

```bash
git add src/config/types/provider.rs
git commit -m "config: add model_behavior override field to ProviderConfig"
```

If other files were fixed in step 5:

```bash
git add -u
git commit -m "config: fix model_behavior field in all ProviderConfig constructors"
```

---

### Task 6: Wire behavior loading into `run_loop.rs`

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Add import**

Inside `run_agent_loop`, there is a function-scoped `use` block at lines 42-46. Add a new use statement inside that block, after line 45:

```rust
        use crate::agent_loop::model_behaviors::{load_model_behavior, protocol_to_behavior};
```

- [ ] **Step 2: Resolve and load behavior BEFORE creating the bridge**

**Critical ordering**: `provider` is moved into `AiProviderBridge::new(provider)` at line 69. We must read the protocol **before** that move. Insert this block between line 68 (`let provider = ...`) and line 69 (`let bridge = ...`):

```rust
        // Resolve model behavior from provider protocol
        let behavior_content = {
            let protocol_name = provider.protocol().to_string();
            protocol_to_behavior(&protocol_name).and_then(load_model_behavior)
        };
```

Then line 69 (`let bridge = AiProviderBridge::new(provider);`) follows as before.

Note: We use `provider.protocol()` (the new trait method from Task 4). The `ProviderConfig.model_behavior` override field (Task 5) is not wired here because `ProviderConfig` is not accessible from `run_loop.rs` through the current `ProviderRegistry` trait. A `// TODO:` comment is added in Task 5 to track this. The protocol auto-mapping covers the primary use case (Claude/GPT/Gemini/Ollama).

- [ ] **Step 3: Pass behavior to PromptBuilder AFTER the skills loading block**

The prompt_builder goes through two construction phases:
1. Lines 125-128: Initial creation from Soul
2. Lines 132-147: Skills loading (potentially rebinds `prompt_builder`)

The model behavior must be injected **after both phases** — i.e., after line 147. Insert the following after the entire skills loading block (after line 147, before line 149 `// Safety guard`):

```rust
        // Inject model behavior directives (after soul + skills)
        let prompt_builder = if let Some(ref content) = behavior_content {
            prompt_builder.with_model_behavior(content)
        } else {
            prompt_builder
        };
```

This chains naturally with the existing `let prompt_builder = ...` rebinding pattern used by the skills block.

- [ ] **Step 4: Compile check**

```bash
cargo check -p alephcore
```

Expected: No errors.

- [ ] **Step 5: Run all tests**

```bash
cargo test -p alephcore --lib
```

Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "run_loop: wire model behavior into prompt building"
```

---

### Task 7: Final verification

- [ ] **Step 1: Run full test suite**

```bash
cargo test -p alephcore --lib
```

Expected: All tests pass, no regressions.

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | head -30
```

Expected: No new warnings.

- [ ] **Step 3: Verify the integration manually**

Start the server and check that the system prompt includes the model behavior section:

```bash
RUST_LOG=debug cargo run --bin aleph-server 2>&1 | grep -i "model.behav" | head -5
```

(Optional — depends on log level and whether behavior loading is logged.)

- [ ] **Step 4: Commit any remaining fixes**

If clippy or tests revealed issues, fix and commit.

```bash
git add -u
git commit -m "model_behaviors: final cleanup after verification"
```
