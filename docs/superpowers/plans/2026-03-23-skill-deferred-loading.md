# Skill Deferred Loading Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable deferred skill loading — LLM sees name+description index, calls `skill_read` tool for full content on-demand.

**Architecture:** Add `DEFERRED_LOADING_GUIDANCE` constant, inject it into both prompt paths (thinker + agent_loop), register `ReadSkillTool` in `BuiltinToolRegistry`, and remove dead v1 code.

**Tech Stack:** Rust, alephcore crate

**Spec:** `docs/superpowers/specs/2026-03-23-skill-deferred-loading-design.md`

---

### Task 1: Add DEFERRED_LOADING_GUIDANCE constant

**Files:**
- Modify: `src/skill/prompt.rs:1-45`
- Test: `src/skill/prompt.rs` (existing test module)

- [ ] **Step 1: Add the constant**

In `src/skill/prompt.rs`, add after the `use` statement (line 3):

```rust
/// Deferred loading guidance appended after skill index in system prompts.
/// Tells the LLM to call `skill_read` before executing a skill.
pub const DEFERRED_LOADING_GUIDANCE: &str =
    "To use a skill, first call the `skill_read` tool with the skill name \
     to load its full instructions, then follow those instructions. \
     Use `skill_list` to discover available skills if needed.";
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles successfully

- [ ] **Step 3: Commit**

```bash
git add src/skill/prompt.rs
git commit -m "skill: add DEFERRED_LOADING_GUIDANCE constant"
```

---

### Task 2: Add guidance to thinker SkillInstructionsLayer

**Files:**
- Modify: `src/thinker/layers/skill_instructions.rs:1-81`
- Test: `src/thinker/layers/skill_instructions.rs` (existing test module, lines 83-283)

- [ ] **Step 1: Write the failing test**

In the existing test module in `skill_instructions.rs`, add:

```rust
#[test]
fn deferred_loading_guidance_present() {
    use crate::skill::prompt::DEFERRED_LOADING_GUIDANCE;

    let layer = SkillInstructionsLayer;
    let skills = vec![make_skill("SomeSkill", PromptScope::System)];
    let config = PromptConfig {
        eligible_skills: Some(skills),
        ..Default::default()
    };
    let tools: Vec<ToolInfo> = vec![];
    let input = LayerInput::basic(&config, &tools);
    let mut out = String::new();
    layer.inject(&mut out, &input);

    assert!(out.contains(DEFERRED_LOADING_GUIDANCE));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib skill_instructions::tests::deferred_loading_guidance_present`
Expected: FAIL — output does not contain the guidance text

- [ ] **Step 3: Add guidance to inject()**

In `skill_instructions.rs`, modify lines 75-79. Note: the third `push_str` changes from `\n\n` to `\n` (single newline) so the guidance text flows naturally after the header. Change from:

```rust
        output.push_str("## Available Skills\n\n");
        output.push_str("You can invoke skills using the `skill` tool. ");
        output.push_str("Skills provide specialized instructions for specific tasks.\n\n");
        output.push_str(&xml);
        output.push_str("\n\n");
```

To:

```rust
        output.push_str("## Available Skills\n\n");
        output.push_str("You can invoke skills using the `skill` tool. ");
        output.push_str("Skills provide specialized instructions for specific tasks.\n");
        output.push_str(crate::skill::prompt::DEFERRED_LOADING_GUIDANCE);
        output.push_str("\n\n");
        output.push_str(&xml);
        output.push_str("\n\n");
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib skill_instructions::tests`
Expected: all tests PASS (including existing tests)

- [ ] **Step 5: Commit**

```bash
git add src/thinker/layers/skill_instructions.rs
git commit -m "thinker: add deferred loading guidance to SkillInstructionsLayer"
```

---

### Task 3: Add guidance to agent_loop PromptBuilder

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs:198-220`
- Test: `src/agent_loop/prompt_builder.rs` (existing test module)

- [ ] **Step 1: Write the failing test**

In the existing test module in `prompt_builder.rs`, add:

```rust
#[test]
fn test_build_with_deferred_loading_guidance() {
    use crate::domain::skill::{PromptScope, SkillContent, SkillManifest, SkillSource};
    use crate::skill::prompt::DEFERRED_LOADING_GUIDANCE;

    let mut skill = SkillManifest::new(
        "test-skill", "Test Skill", "A test skill",
        SkillContent::new("content"), SkillSource::Bundled,
    );
    skill.set_scope(PromptScope::System);

    let builder = PromptBuilder::new()
        .with_eligible_skills(vec![skill]);

    let prompt = builder.build(&[], None);

    assert!(prompt.contains(DEFERRED_LOADING_GUIDANCE));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::prompt_builder::tests::test_build_with_deferred_loading_guidance`
Expected: FAIL

- [ ] **Step 3: Add guidance to build()**

In `prompt_builder.rs`, modify lines 214-218. Change from:

```rust
                let xml = build_skills_prompt_xml(&filtered);
                sections.push(format!(
                    "# Available Skills\n\nYou can invoke skills using the `skill` tool. \
                     Skills provide specialized instructions for specific tasks.\n\n{}",
                    xml
                ));
```

To:

```rust
                let xml = build_skills_prompt_xml(&filtered);
                sections.push(format!(
                    "# Available Skills\n\nYou can invoke skills using the `skill` tool. \
                     Skills provide specialized instructions for specific tasks.\n\
                     {}\n\n{}",
                    crate::skill::prompt::DEFERRED_LOADING_GUIDANCE,
                    xml
                ));
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agent_loop::prompt_builder::tests`
Expected: all tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "agent_loop: add deferred loading guidance to PromptBuilder"
```

---

### Task 4: Register ReadSkillTool in BuiltinToolRegistry

**Files:**
- Modify: `src/executor/builtin_registry/registry.rs:27-133` (add field)
- Modify: `src/executor/builtin_registry/builder.rs:12-442` (create instance, register, add to struct)
- Modify: `src/executor/builtin_registry/registry.rs:270-575` (add execute_tool match arm)

- [ ] **Step 1: Add field to BuiltinToolRegistry struct**

In `registry.rs`, add after the `list_skills_tool` field (line 43):

```rust
    /// Read skill tool instance (deferred loading — LLM calls this to load full skill instructions)
    pub(crate) read_skill_tool: crate::builtin_tools::skill_reader::ReadSkillTool,
```

- [ ] **Step 2: Add import and instantiation in builder.rs**

In `builder.rs`, add to the import line 19 (change from):

```rust
use crate::builtin_tools::skill_reader::ListSkillsTool as SkillListTool;
```

To:

```rust
use crate::builtin_tools::skill_reader::{ListSkillsTool as SkillListTool, ReadSkillTool as SkillReadTool};
```

In the `with_config()` method, after line 53 (`let list_skills_tool = SkillListTool::default();`), add:

```rust
        let read_skill_tool = SkillReadTool::default();
```

- [ ] **Step 3: Register tool metadata in register_core_tools()**

In `builder.rs`, after the `skill_list` registration (line 477-478), add:

```rust
        reg(tools, "skill_read", SkillReadTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::skill_reader::ReadSkillArgs)).unwrap_or_default());
```

Note: `ReadSkillArgs` already derives `JsonSchema` (schemars) at `skill_reader.rs:29`.

- [ ] **Step 4: Add field to struct initialization**

In `builder.rs`, in the `Self { ... }` block, add after `list_skills_tool,` (line 368):

```rust
            read_skill_tool,
```

- [ ] **Step 5: Update the info log**

In `builder.rs`, change line 171 from:

```rust
        info!("Registered skill.list and read_config_guide tools in BuiltinToolRegistry");
```

To:

```rust
        info!("Registered skill.list, skill.read, and read_config_guide tools in BuiltinToolRegistry");
```

- [ ] **Step 6: Add execute_tool match arm**

In `registry.rs`, after the `"skill_list"` match arm (line 306), add:

```rust
            "skill_read" => Box::pin(async move { self.read_skill_tool.call_json(arguments).await }),
```

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles successfully

- [ ] **Step 8: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: all tests PASS

- [ ] **Step 9: Verify tool is registered**

Run: `cargo test -p alephcore --lib` and manually verify that `BuiltinToolRegistry::with_config()` produces a registry where `tools.get("skill_read")` returns `Some(...)`. If there is an existing test for tool registration, add `skill_read` to it. Otherwise, verify by checking the info log output includes "skill.read".

- [ ] **Step 10: Commit**

```bash
git add src/executor/builtin_registry/registry.rs src/executor/builtin_registry/builder.rs
git commit -m "executor: register ReadSkillTool (skill_read) in BuiltinToolRegistry"
```

---

### Task 5: Clean up dead code in extension/skill_tool.rs

**Files:**
- Modify: `src/extension/skill_tool.rs:246-343` (remove dead functions)
- Modify: `src/extension/skill_tool.rs:443-652` (remove dead tests)

**Important:** All line numbers below reference the original file state. Perform all deletions in a single pass (delete tests first bottom-to-top, then functions bottom-to-top) to avoid line number shifts.

- [ ] **Step 1: Remove dead tests (delete bottom-to-top)**

In the `#[cfg(test)] mod tests` section, delete:
- `test_build_tool_description_empty` (lines 443-447)
- `test_build_tool_description` (lines 449-457)
- `create_skill_with_scope` helper (lines 467-480)
- `test_filter_skills_by_scope_system_always_included` (lines 482-497)
- `test_filter_skills_by_scope_tool_requires_active_tool` (lines 499-519)
- `test_filter_skills_by_scope_tool_without_bound_tool_excluded` (lines 521-531)
- `test_filter_skills_by_scope_standalone_never_included` (lines 533-546)
- `test_filter_skills_by_scope_disabled_never_included` (lines 548-556)
- `test_filter_skills_by_scope_mixed` (lines 558-586)
- `test_build_skill_tool_description_v2_empty` (lines 588-593)
- `test_build_skill_tool_description_v2_filters_by_scope` (lines 595-615)
- `test_build_skill_tool_description_v2_respects_auto_invocable` (lines 617-636)
- `test_build_skill_tool_description_v2_format` (lines 638-652+)

Keep all other tests (permission tests, invoke_skill test).

- [ ] **Step 2: Remove dead functions (delete bottom-to-top)**

In `skill_tool.rs`, delete:
- `build_skill_tool_description_v2()` (lines 302-343)
- `filter_skills_by_scope()` (lines 271-300)
- `build_skill_tool_description()` (lines 246-269)

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles (may show warnings about unused `create_skill_with_scope` if not fully removed)

- [ ] **Step 4: Run remaining tests**

Run: `cargo test -p alephcore --lib extension::skill_tool::tests`
Expected: remaining tests PASS (permission tests, invoke_skill test)

- [ ] **Step 5: Commit**

```bash
git add src/extension/skill_tool.rs
git commit -m "extension: remove dead skill_tool_description and scope filtering functions"
```

---

### Task 6: Clean up dead code in extension/mod.rs and skill_ops.rs

**Files:**
- Modify: `src/extension/mod.rs:64` (remove pub use export)
- Modify: `src/extension/mod.rs:335-360` (remove dead function)
- Modify: `src/extension/skill_ops.rs:119-124` (remove dead method)

- [ ] **Step 1: Remove pub use export**

In `extension/mod.rs` line 64, change from:

```rust
pub use skill_tool::{build_skill_tool_description, check_skill_permission, request_skill_permission_async};
```

To:

```rust
pub use skill_tool::{check_skill_permission, request_skill_permission_async};
```

- [ ] **Step 2: Remove build_skill_instructions()**

In `extension/mod.rs`, delete the `build_skill_instructions()` function and its doc comment (lines 339-360). Keep the section separator comment at lines 335-338 (`// === Utility Functions ===`).

- [ ] **Step 3: Remove get_skill_tool_description()**

In `extension/skill_ops.rs`, delete lines 119-124:

```rust
    /// Get skill tool description for LLM
    pub async fn get_skill_tool_description(&self) -> String {
        self.ensure_loaded().await.ok();
        let skills = self.get_auto_invocable_skills().await;
        super::skill_tool::build_skill_tool_description(&skills)
    }
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles successfully. If there are downstream users of `build_skill_tool_description` outside the crate, the compiler will tell us.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: all tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/extension/mod.rs src/extension/skill_ops.rs
git commit -m "extension: remove dead build_skill_instructions and get_skill_tool_description"
```

---

### Task 7: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: all tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: no new warnings

- [ ] **Step 3: Verify the complete change set**

Run: `git log --oneline -6`
Expected: 6 commits from this plan, all on main branch
