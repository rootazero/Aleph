# Agent Prompt Pipeline & Verification Agent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace static agent prompts with Section Registry-based prompt assembly, add specialized behavioral prompts for all built-in agents, and introduce a Verification Agent with StopHook integration.

**Architecture:** Extend the existing PromptBuilder Section Registry to support per-agent section composition. Each AgentDef declares which prompt sections it needs; PromptBuilder::for_agent() assembles shared Stable sections + agent-specific Dynamic sections. A new Verify Agent runs as both a callable sub-agent and an automatic StopHook.

**Tech Stack:** Rust, tokio (async), serde, async-trait

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/agents/types.rs` | Remove `system_prompt`, add `prompt_sections` to AgentDef |
| Modify | `src/agents/registry.rs` | Update `builtin_agents()` with new API, add verify agent |
| Create | `src/agent_loop/prompt_sections/agent_role.rs` | Shared sub-agent base role section |
| Create | `src/agent_loop/prompt_sections/explore_constraints.rs` | Explore agent behavioral constraints |
| Create | `src/agent_loop/prompt_sections/coder_guidelines.rs` | Coder agent behavioral guidelines |
| Create | `src/agent_loop/prompt_sections/researcher_protocol.rs` | Researcher agent behavioral protocol |
| Create | `src/agent_loop/prompt_sections/verify_protocol.rs` | Verify agent adversarial protocol |
| Modify | `src/agent_loop/prompt_sections/mod.rs` | Add module declarations + resolve() function |
| Modify | `src/agent_loop/prompt_builder.rs` | Add `for_agent()` constructor + `with_prompt_sections()` |
| Modify | `src/agent_loop/stop_hooks.rs` | Extract StopHookHandler trait, rename StopHook → ShellStopHook |
| Create | `src/agent_loop/verify_stop_hook.rs` | VerifyStopHook implementation |
| Modify | `src/agent_loop/mod.rs` | Add verify_stop_hook module, update re-exports |
| Modify | `src/agent_loop/loop_core.rs` | Update stop_hooks type, wire for_agent() |
| Delete | `src/agents/prompts/main.md` | Replaced by section system |
| Delete | `src/agents/prompts/explore.md` | Replaced by explore_constraints section |
| Delete | `src/agents/prompts/coder.md` | Replaced by coder_guidelines section |
| Delete | `src/agents/prompts/researcher.md` | Replaced by researcher_protocol section |

---

## Task 1: Refactor AgentDef — Remove system_prompt, Add prompt_sections

**Files:**
- Modify: `src/agents/types.rs`

- [ ] **Step 1: Write failing test for new prompt_sections field**

Add this test at the end of the existing `mod tests` block in `src/agents/types.rs`:

```rust
#[test]
fn test_agent_def_prompt_sections() {
    let agent = AgentDef::new("test", AgentMode::SubAgent)
        .with_prompt_sections(vec!["explore_constraints".into()])
        .with_allowed_tools(vec!["glob".into()]);

    assert_eq!(agent.id, "test");
    assert_eq!(agent.prompt_sections, vec!["explore_constraints".to_string()]);
    assert!(agent.allowed_tools.contains(&"glob".to_string()));
}

#[test]
fn test_agent_def_default_prompt_sections_empty() {
    let agent = AgentDef::new("test", AgentMode::Primary);
    assert!(agent.prompt_sections.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agents::types::tests::test_agent_def_prompt_sections`
Expected: FAIL — `AgentDef::new` takes 3 args, not 2; no `prompt_sections` field; no `with_prompt_sections` method.

- [ ] **Step 3: Modify AgentDef struct and impl**

Replace the entire `AgentDef` struct and its `impl` block in `src/agents/types.rs`:

```rust
/// Definition of an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    /// Unique identifier (e.g., "explore", "coder", "researcher")
    pub id: String,
    /// Agent mode
    pub mode: AgentMode,
    /// Prompt section names this agent requires (resolved by PromptBuilder)
    pub prompt_sections: Vec<String>,
    /// Tools this agent is allowed to use ("*" for all)
    pub allowed_tools: Vec<String>,
    /// Tools this agent is denied from using
    pub denied_tools: Vec<String>,
    /// Maximum iterations (overrides default loop limit)
    pub max_iterations: Option<u32>,
}

impl AgentDef {
    /// Create a new agent definition
    pub fn new(id: impl Into<String>, mode: AgentMode) -> Self {
        Self {
            id: id.into(),
            mode,
            prompt_sections: vec![],
            allowed_tools: vec!["*".into()],
            denied_tools: vec![],
            max_iterations: None,
        }
    }

    /// Set prompt sections for this agent
    pub fn with_prompt_sections(mut self, sections: Vec<String>) -> Self {
        self.prompt_sections = sections;
        self
    }

    /// Set allowed tools
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = tools;
        self
    }

    /// Set denied tools
    pub fn with_denied_tools(mut self, tools: Vec<String>) -> Self {
        self.denied_tools = tools;
        self
    }

    /// Set max iterations
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Check if a tool is allowed for this agent
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        if self.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }
        if self.allowed_tools.iter().any(|t| t == "*") {
            return true;
        }
        self.allowed_tools.iter().any(|t| t == tool_name)
    }
}
```

- [ ] **Step 4: Update existing tests in types.rs**

Replace the existing `test_agent_def_new` test:

```rust
#[test]
fn test_agent_def_new() {
    let agent = AgentDef::new("test", AgentMode::SubAgent);
    assert_eq!(agent.id, "test");
    assert_eq!(agent.mode, AgentMode::SubAgent);
    assert!(agent.prompt_sections.is_empty());
    assert_eq!(agent.allowed_tools, vec!["*"]);
    assert!(agent.denied_tools.is_empty());
    assert!(agent.max_iterations.is_none());
}
```

Update all other tests that call `AgentDef::new` with 3 args — remove the third `system_prompt` argument. The tests `test_is_tool_allowed_wildcard`, `test_is_tool_allowed_specific`, `test_is_tool_denied`, `test_denied_overrides_allowed`, `test_with_max_iterations` all need the third arg removed:

```rust
// Change all occurrences of:
AgentDef::new("test", AgentMode::SubAgent, "")
AgentDef::new("test", AgentMode::SubAgent, "Test prompt")
AgentDef::new("test", AgentMode::Primary, "")
// To:
AgentDef::new("test", AgentMode::SubAgent)
AgentDef::new("test", AgentMode::Primary)
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agents::types::tests`
Expected: ALL PASS (8 tests)

- [ ] **Step 6: Fix all compilation errors across the codebase**

Search for all remaining references to the old 3-arg `AgentDef::new` and `agent.system_prompt`:

Run: `cargo check -p alephcore 2>&1 | head -60`

Fix each error. Key locations:
- `src/agents/registry.rs` — `builtin_agents()` calls `AgentDef::new` with 3 args + `include_str!` (fix in Task 2)
- Any code reading `agent_def.system_prompt` — replace with PromptBuilder path (fix in Task 6)

For now, make it compile by updating `registry.rs` to use 2-arg `new()` with temporary empty prompt_sections (proper values added in Task 2).

- [ ] **Step 7: Run full check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 8: Commit**

```bash
git add src/agents/types.rs
git commit -m "refactor(agents): remove system_prompt from AgentDef, add prompt_sections"
```

---

## Task 2: Update builtin_agents() Registry with New API

**Files:**
- Modify: `src/agents/registry.rs`

- [ ] **Step 1: Write failing test for verify agent**

Add to the existing `mod tests` block in `src/agents/registry.rs`:

```rust
#[test]
fn test_verify_agent_config() {
    let registry = AgentRegistry::with_builtins();
    let verify = registry.get("verify").unwrap();

    assert_eq!(verify.mode, AgentMode::SubAgent);
    assert!(verify.is_tool_allowed("bash"));
    assert!(verify.is_tool_allowed("glob"));
    assert_eq!(verify.max_iterations, Some(25));
    assert!(verify.prompt_sections.contains(&"verify_protocol".to_string()));
}

#[test]
fn test_builtin_agents_have_prompt_sections() {
    let registry = AgentRegistry::with_builtins();

    let explore = registry.get("explore").unwrap();
    assert!(explore.prompt_sections.contains(&"explore_constraints".to_string()));

    let coder = registry.get("coder").unwrap();
    assert!(coder.prompt_sections.contains(&"coder_guidelines".to_string()));

    let researcher = registry.get("researcher").unwrap();
    assert!(researcher.prompt_sections.contains(&"researcher_protocol".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agents::registry::tests::test_verify_agent_config`
Expected: FAIL — no "verify" agent registered; prompt_sections empty.

- [ ] **Step 3: Rewrite builtin_agents() function**

Replace the entire `builtin_agents()` function in `src/agents/registry.rs`:

```rust
/// Returns the built-in agent definitions
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![
        // Main agent — full access, no agent-specific sections
        AgentDef::new("main", AgentMode::Primary),
        // Explore agent — read-only tools
        AgentDef::new("explore", AgentMode::SubAgent)
            .with_prompt_sections(vec!["explore_constraints".into()])
            .with_allowed_tools(vec![
                "glob".into(),
                "grep".into(),
                "read_file".into(),
                "web_fetch".into(),
                "search".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
            .with_max_iterations(20),
        // Coder agent — file operations
        AgentDef::new("coder", AgentMode::SubAgent)
            .with_prompt_sections(vec!["coder_guidelines".into()])
            .with_allowed_tools(vec![
                "read_file".into(),
                "write_file".into(),
                "edit_file".into(),
                "glob".into(),
                "grep".into(),
                "bash".into(),
            ])
            .with_max_iterations(30),
        // Researcher agent — search and web
        AgentDef::new("researcher", AgentMode::SubAgent)
            .with_prompt_sections(vec!["researcher_protocol".into()])
            .with_allowed_tools(vec![
                "search".into(),
                "web_fetch".into(),
                "read_file".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
            .with_max_iterations(15),
        // Verify agent — adversarial verification
        AgentDef::new("verify", AgentMode::SubAgent)
            .with_prompt_sections(vec!["verify_protocol".into()])
            .with_allowed_tools(vec!["*".into()])
            .with_max_iterations(25),
    ]
}
```

- [ ] **Step 4: Update existing tests**

Update `test_builtin_agents_count`:

```rust
#[test]
fn test_builtin_agents_count() {
    let agents = builtin_agents();
    assert_eq!(agents.len(), 5);
}
```

Update `test_registry_register_and_get` — remove third arg:

```rust
#[test]
fn test_registry_register_and_get() {
    let registry = AgentRegistry::new();
    let agent = AgentDef::new("test", AgentMode::SubAgent);
    registry.register(agent);
    let retrieved = registry.get("test").unwrap();
    assert_eq!(retrieved.id, "test");
}
```

Update all other tests in this file that use 3-arg `AgentDef::new` to use 2-arg version. Remove all assertions on `system_prompt`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agents::registry::tests`
Expected: ALL PASS

- [ ] **Step 6: Commit**

```bash
git add src/agents/registry.rs
git commit -m "refactor(agents): update builtin_agents with prompt_sections, add verify agent"
```

---

## Task 3: Create Agent-Specific Prompt Section Renderers

**Files:**
- Create: `src/agent_loop/prompt_sections/agent_role.rs`
- Create: `src/agent_loop/prompt_sections/explore_constraints.rs`
- Create: `src/agent_loop/prompt_sections/coder_guidelines.rs`
- Create: `src/agent_loop/prompt_sections/researcher_protocol.rs`
- Create: `src/agent_loop/prompt_sections/verify_protocol.rs`
- Modify: `src/agent_loop/prompt_sections/mod.rs`

- [ ] **Step 1: Create agent_role.rs**

Create `src/agent_loop/prompt_sections/agent_role.rs`:

```rust
//! Shared base role section for all sub-agents.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

/// Render the shared sub-agent role section.
///
/// `agent_id` is injected so the LLM knows which specialist it is.
pub fn render(agent_id: &str) -> PromptSection {
    let content = format!(
        r#"# Sub-Agent Role

You are **{agent_id}**, a specialized sub-agent of Aleph.

## Contract
- Complete the delegated task fully — do not leave partial work.
- Stay within your declared tool set. Do not attempt to use tools you are not given.
- End with a concise report: what you did, key findings or changes, and recommended next steps.
- If you cannot complete the task with your available tools, explain what is missing rather than guessing.

## Communication
- Be direct and factual. No filler, no apologies.
- Structure output for machine readability when the caller is another agent."#
    );

    PromptSection {
        name: "agent_role".into(),
        stability: Stability::Dynamic,
        priority: 55,
        protected: true,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_contains_agent_id() {
        let section = render("explore");
        assert!(section.content.contains("**explore**"));
        assert_eq!(section.name, "agent_role");
        assert_eq!(section.stability, Stability::Dynamic);
        assert_eq!(section.priority, 55);
        assert!(section.protected);
    }
}
```

- [ ] **Step 2: Create explore_constraints.rs**

Create `src/agent_loop/prompt_sections/explore_constraints.rs`:

```rust
//! Behavioral constraints for the Explore agent.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render() -> PromptSection {
    PromptSection {
        name: "explore_constraints".into(),
        stability: Stability::Dynamic,
        priority: 60,
        protected: false,
        content: r#"# Explore Agent Constraints

## Role
You are a read-only exploration specialist. Your sole purpose is gathering
information — you NEVER modify, create, or delete anything.

## Behavioral Rules
- Prefer parallel tool calls for speed (glob + grep simultaneously).
- Start broad (directory structure), then narrow (specific files).
- When searching code, try multiple patterns before reporting "not found".
- Read only the parts of files you need — use offset/limit for large files.

## Hard Constraints (enforced by system)
- File modification tools are blocked at runtime.
- Bash is not available.
- Maximum 20 iterations — be efficient.

## Output Format
End with a structured summary:
- **Findings**: what you found, with file paths and line numbers.
- **Relevance**: how each finding relates to the request.
- **Next steps**: recommended actions for the caller."#
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_has_correct_metadata() {
        let section = render();
        assert_eq!(section.name, "explore_constraints");
        assert_eq!(section.priority, 60);
        assert!(section.content.contains("read-only exploration specialist"));
    }
}
```

- [ ] **Step 3: Create coder_guidelines.rs**

Create `src/agent_loop/prompt_sections/coder_guidelines.rs`:

```rust
//! Behavioral guidelines for the Coder agent.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render() -> PromptSection {
    PromptSection {
        name: "coder_guidelines".into(),
        stability: Stability::Dynamic,
        priority: 60,
        protected: false,
        content: r#"# Coder Agent Guidelines

## Role
You are a code writing specialist. You read, write, and edit code with precision.

## Behavioral Rules
- Read existing code before modifying — understand context and conventions first.
- Make minimal, focused changes. Do not refactor unrelated code.
- One concern per edit. If a file needs multiple changes, make them in separate edits.
- Verify changes compile: run `cargo check` after significant edits.
- Follow the project's existing patterns, naming, and style.

## Hard Constraints
- Maximum 30 iterations — plan your work efficiently.
- Do not introduce new dependencies without explicit approval.

## Output Format
End with a summary:
- **Changes made**: list each file modified with a one-line description.
- **Compilation**: whether `cargo check` passes.
- **Notes**: anything the caller should review or test."#
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_has_correct_metadata() {
        let section = render();
        assert_eq!(section.name, "coder_guidelines");
        assert_eq!(section.priority, 60);
        assert!(section.content.contains("code writing specialist"));
    }
}
```

- [ ] **Step 4: Create researcher_protocol.rs**

Create `src/agent_loop/prompt_sections/researcher_protocol.rs`:

```rust
//! Behavioral protocol for the Researcher agent.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render() -> PromptSection {
    PromptSection {
        name: "researcher_protocol".into(),
        stability: Stability::Dynamic,
        priority: 60,
        protected: false,
        content: r#"# Researcher Agent Protocol

## Role
You are an information gathering specialist. You search, fetch, and synthesize
information from multiple sources.

## Behavioral Rules
- Cross-reference multiple sources before making claims.
- Distinguish facts from inference — label speculation clearly.
- Cite sources: include URLs, file paths, or document names.
- Prefer primary sources (official docs, source code) over secondary.
- When web results are ambiguous, try different search queries.

## Hard Constraints (enforced by system)
- File modification tools are blocked at runtime.
- Bash is not available.
- Maximum 15 iterations — prioritize high-value sources.

## Output Format
End with a structured research report:
- **Summary**: 2-3 sentence answer to the research question.
- **Findings**: detailed evidence organized by topic.
- **Sources**: list of all sources consulted.
- **Confidence**: high / medium / low, with reasoning."#
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_has_correct_metadata() {
        let section = render();
        assert_eq!(section.name, "researcher_protocol");
        assert_eq!(section.priority, 60);
        assert!(section.content.contains("information gathering specialist"));
    }
}
```

- [ ] **Step 5: Create verify_protocol.rs**

Create `src/agent_loop/prompt_sections/verify_protocol.rs`:

```rust
//! Adversarial verification protocol for the Verify agent.

use crate::agent_loop::prompt_builder::{PromptSection, Stability};

pub fn render() -> PromptSection {
    PromptSection {
        name: "verify_protocol".into(),
        stability: Stability::Dynamic,
        priority: 60,
        protected: false,
        content: r#"# Verification Agent Protocol

## Mindset
You are an adversarial verifier. Your job is to TRY TO BREAK IT, not to confirm
it works. Assume the implementation has bugs until proven otherwise.

## Mandatory Checks
For every verification request, you MUST run all applicable checks:
1. **Build check**: `cargo check` — compilation must pass.
2. **Test suite**: `cargo test` — all tests must pass.
3. **Lint check**: `cargo clippy` — no errors.

Do NOT skip a check. If a check cannot run, the verdict is PARTIAL.

## Change-Type Specific Checks
- **Code changes**: read the diff, verify logic correctness, check edge cases.
- **Refactoring**: verify public API surface is unchanged.
- **New features**: verify test coverage exists for new code paths.
- **Bug fixes**: verify the specific bug scenario is covered by a test.

## Adversarial Probes
After mandatory checks pass, actively look for:
- Edge cases the tests don't cover.
- Error handling gaps (unwrap, expect in non-test code).
- Assumptions that could break under different inputs.
- Off-by-one errors, empty collection handling, None/null paths.

## Output Format
Always end with a verdict block exactly in this format:

```
VERDICT: PASS | FAIL | PARTIAL
REASON: <one-line summary>
CHECKS:
- [x] build: <result>
- [x] tests: <N passed, M failed>
- [x] lint: <result>
ISSUES:
- <issue 1>
- <issue 2>
```

## Hard Rules
- NEVER output PASS without actually running the mandatory checks.
- NEVER skip a mandatory check — if it can't run, verdict is PARTIAL.
- Report what you OBSERVED, not what you expected.
- Maximum 25 iterations."#
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_has_correct_metadata() {
        let section = render();
        assert_eq!(section.name, "verify_protocol");
        assert_eq!(section.priority, 60);
        assert!(section.content.contains("adversarial verifier"));
        assert!(section.content.contains("VERDICT:"));
    }
}
```

- [ ] **Step 6: Update mod.rs — add module declarations and resolve function**

Add the following to `src/agent_loop/prompt_sections/mod.rs`:

```rust
//! Section renderers for PromptBuilder.
//!
//! Each submodule exports a `render()` function that returns a `PromptSection`.

pub mod identity;
pub mod tone;
pub mod directives;
pub mod model_behavior;
pub mod system_rules;
pub mod doing_tasks;
pub mod actions;
pub mod tool_usage;
pub mod tone_and_style;
pub mod output_efficiency;
pub mod tools;
pub mod skills;
pub mod memory_protocol;
pub mod custom_instructions;
pub mod environment;
pub mod session_guidance;
pub mod memory;
pub mod discovered_skills;

// Agent-specific sections
pub mod agent_role;
pub mod explore_constraints;
pub mod coder_guidelines;
pub mod researcher_protocol;
pub mod verify_protocol;

use super::prompt_builder::PromptSection;

/// Resolve an agent-specific section by name.
///
/// Returns `None` for unknown names. `agent_role` is handled separately
/// by `PromptBuilder::for_agent()` since it requires the agent ID.
pub fn resolve(name: &str) -> Option<PromptSection> {
    match name {
        "explore_constraints" => Some(explore_constraints::render()),
        "coder_guidelines" => Some(coder_guidelines::render()),
        "researcher_protocol" => Some(researcher_protocol::render()),
        "verify_protocol" => Some(verify_protocol::render()),
        _ => None,
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn resolve_known_sections() {
        assert!(resolve("explore_constraints").is_some());
        assert!(resolve("coder_guidelines").is_some());
        assert!(resolve("researcher_protocol").is_some());
        assert!(resolve("verify_protocol").is_some());
    }

    #[test]
    fn resolve_unknown_returns_none() {
        assert!(resolve("nonexistent").is_none());
        assert!(resolve("agent_role").is_none()); // handled separately
    }
}
```

- [ ] **Step 7: Run all section tests**

Run: `cargo test -p alephcore --lib agent_loop::prompt_sections`
Expected: ALL PASS

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/prompt_sections/
git commit -m "feat(prompt): add agent-specific section renderers and resolve function"
```

---

## Task 4: Add PromptBuilder::for_agent() Constructor

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs`

- [ ] **Step 1: Write failing test**

Add to the `mod tests` block in `src/agent_loop/prompt_builder.rs`:

```rust
#[test]
fn for_agent_includes_shared_and_specific_sections() {
    use crate::agents::types::{AgentDef, AgentMode};

    let agent = AgentDef::new("explore", AgentMode::SubAgent)
        .with_prompt_sections(vec!["explore_constraints".into()]);

    let builder = PromptBuilder::for_agent(&agent);
    let result = builder.build();

    // Should contain shared behavioral sections
    assert!(result.prompt.contains("# System"), "missing system_rules");

    // Should contain agent_role section
    assert!(result.prompt.contains("**explore**"), "missing agent_role with agent id");

    // Should contain explore-specific section
    assert!(
        result.prompt.contains("read-only exploration specialist"),
        "missing explore_constraints"
    );
}

#[test]
fn for_agent_primary_has_no_agent_role() {
    use crate::agents::types::{AgentDef, AgentMode};

    let agent = AgentDef::new("main", AgentMode::Primary);
    let builder = PromptBuilder::for_agent(&agent);
    let result = builder.build();

    // Primary agent should NOT have agent_role section
    assert!(!result.prompt.contains("Sub-Agent Role"), "primary should not have agent_role");

    // Should still have shared behavioral sections
    assert!(result.prompt.contains("# System"), "missing system_rules");
}

#[test]
fn for_agent_unknown_sections_are_skipped() {
    use crate::agents::types::{AgentDef, AgentMode};

    let agent = AgentDef::new("custom", AgentMode::SubAgent)
        .with_prompt_sections(vec!["nonexistent_section".into()]);

    let builder = PromptBuilder::for_agent(&agent);
    let result = builder.build();

    // Should still work — just missing the unknown section
    assert!(result.prompt.contains("# System"), "missing system_rules");
    assert!(result.prompt.contains("Sub-Agent Role"), "missing agent_role");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::prompt_builder::tests::for_agent_includes_shared_and_specific_sections`
Expected: FAIL — no `for_agent` method.

- [ ] **Step 3: Implement for_agent()**

Add this impl block to `src/agent_loop/prompt_builder.rs`, inside the existing `impl PromptBuilder` convenience constructors section (after `with_default_identity`):

```rust
    /// Build a prompt for a specific agent.
    ///
    /// Registers shared Stable behavioral sections, then adds the `agent_role`
    /// section for sub-agents and resolves any agent-specific sections declared
    /// in `agent.prompt_sections`.
    pub fn for_agent(agent: &crate::agents::types::AgentDef) -> Self {
        let mut builder = Self::new().with_default_behavior_sections();

        // Sub-agents get the shared agent_role section
        if agent.mode == crate::agents::types::AgentMode::SubAgent {
            builder.register(
                super::prompt_sections::agent_role::render(&agent.id),
            );
        }

        // Resolve and register agent-specific sections
        for section_name in &agent.prompt_sections {
            if let Some(section) = super::prompt_sections::resolve(section_name) {
                builder.register(section);
            }
        }

        builder
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agent_loop::prompt_builder::tests`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "feat(prompt): add PromptBuilder::for_agent() for sub-agent prompt assembly"
```

---

## Task 5: Refactor StopHook into Trait + ShellStopHook

**Files:**
- Modify: `src/agent_loop/stop_hooks.rs`
- Modify: `src/agent_loop/mod.rs`
- Modify: `src/agent_loop/loop_core.rs`

- [ ] **Step 1: Write failing test for trait-based dispatch**

Add to the `mod tests` block in `src/agent_loop/stop_hooks.rs`:

```rust
#[tokio::test]
async fn test_trait_based_hook_allow() {
    struct AlwaysAllow;
    #[async_trait::async_trait]
    impl StopHookHandler for AlwaysAllow {
        fn name(&self) -> &str { "always_allow" }
        async fn evaluate(&self, _ctx: &StopHookContext, _cancel: &CancellationToken) -> StopHookVerdict {
            StopHookVerdict::Allow
        }
    }

    let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(AlwaysAllow)];
    let ctx = StopHookContext {
        final_text: None,
        iterations: 5,
        tool_calls_made: 3,
        stop_reason: "end_turn".into(),
    };
    let cancel = CancellationToken::new();
    let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
    assert!(result.blocking_reason().is_none());
}

#[tokio::test]
async fn test_trait_based_hook_block() {
    struct AlwaysBlock;
    #[async_trait::async_trait]
    impl StopHookHandler for AlwaysBlock {
        fn name(&self) -> &str { "always_block" }
        async fn evaluate(&self, _ctx: &StopHookContext, _cancel: &CancellationToken) -> StopHookVerdict {
            StopHookVerdict::Block { reason: "blocked by trait".into() }
        }
    }

    let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(AlwaysBlock)];
    let ctx = StopHookContext {
        final_text: None,
        iterations: 1,
        tool_calls_made: 0,
        stop_reason: "end_turn".into(),
    };
    let cancel = CancellationToken::new();
    let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
    assert_eq!(result.blocking_reason(), Some("blocked by trait"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::stop_hooks::tests::test_trait_based_hook_allow`
Expected: FAIL — no `StopHookHandler` trait.

- [ ] **Step 3: Add StopHookHandler trait and refactor**

Replace the contents of `src/agent_loop/stop_hooks.rs`. Key changes:
1. Add `StopHookHandler` trait
2. Rename `StopHook` → `ShellStopHook`
3. Implement `StopHookHandler` for `ShellStopHook`
4. Change `execute_stop_hooks` to accept `&[Box<dyn StopHookHandler>]`

```rust
//! Stop hooks — pluggable handlers executed before the agent loop stops.
//!
//! Handlers implement `StopHookHandler`. The built-in `ShellStopHook` runs
//! external commands; other implementations (e.g. VerifyStopHook) run in-process.

use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

// ── Trait ──────────────────────────────────────────────────────────────

/// A handler that decides whether the agent loop is allowed to stop.
#[async_trait::async_trait]
pub trait StopHookHandler: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Evaluate whether to allow or block the stop.
    async fn evaluate(
        &self,
        ctx: &StopHookContext,
        cancel: &CancellationToken,
    ) -> StopHookVerdict;
}

// ── Context & Verdict ──────────────────────────────────────────────────

/// Context passed to stop hooks.
#[derive(Serialize, Clone)]
pub struct StopHookContext {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub stop_reason: String,
}

/// Result of a single hook execution.
#[derive(Debug)]
pub enum StopHookVerdict {
    Allow,
    Block { reason: String },
    Error { hook_name: String, message: String },
}

/// Aggregated result of all stop hooks.
#[derive(Debug)]
pub struct StopHookAggregateResult {
    pub verdicts: Vec<StopHookVerdict>,
}

impl StopHookAggregateResult {
    /// Returns the first blocking reason, if any.
    pub fn blocking_reason(&self) -> Option<&str> {
        self.verdicts.iter().find_map(|v| match v {
            StopHookVerdict::Block { reason } => Some(reason.as_str()),
            _ => None,
        })
    }

    /// Returns all error messages.
    pub fn errors(&self) -> Vec<(&str, &str)> {
        self.verdicts
            .iter()
            .filter_map(|v| match v {
                StopHookVerdict::Error { hook_name, message } => {
                    Some((hook_name.as_str(), message.as_str()))
                }
                _ => None,
            })
            .collect()
    }
}

// ── ShellStopHook ──────────────────────────────────────────────────────

/// A stop hook that runs an external shell command.
///
/// Exit codes: 0 = allow, 2 = block (stdout = reason), other = error.
pub struct ShellStopHook {
    hook_name: String,
    pub command: String,
    pub timeout: Duration,
}

impl ShellStopHook {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            hook_name: name.into(),
            command: command.into(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait::async_trait]
impl StopHookHandler for ShellStopHook {
    fn name(&self) -> &str {
        &self.hook_name
    }

    async fn evaluate(
        &self,
        ctx: &StopHookContext,
        cancel: &CancellationToken,
    ) -> StopHookVerdict {
        execute_shell_hook(self, ctx, cancel).await
    }
}

// ── Execution ──────────────────────────────────────────────────────────

/// Execute all stop hooks in parallel.
pub async fn execute_stop_hooks(
    hooks: &[Box<dyn StopHookHandler>],
    context: &StopHookContext,
    cancel: &CancellationToken,
) -> StopHookAggregateResult {
    use futures::future::join_all;

    let futures: Vec<_> = hooks
        .iter()
        .map(|hook| hook.evaluate(context, cancel))
        .collect();

    let verdicts = join_all(futures).await;
    StopHookAggregateResult { verdicts }
}

async fn execute_shell_hook(
    hook: &ShellStopHook,
    ctx: &StopHookContext,
    cancel: &CancellationToken,
) -> StopHookVerdict {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::process::Command;

    let context_json =
        serde_json::to_string(ctx).unwrap_or_else(|_| "{}".to_string());

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&hook.command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return StopHookVerdict::Error {
                hook_name: hook.hook_name.clone(),
                message: format!("failed to spawn: {e}"),
            };
        }
    };

    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();

    let result = tokio::select! {
        r = async {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(context_json.as_bytes()).await;
                drop(stdin);
            }

            let (stdout_buf, stderr_buf) = tokio::join!(
                async {
                    let mut buf = Vec::new();
                    if let Some(ref mut h) = stdout_handle {
                        let _ = h.read_to_end(&mut buf).await;
                    }
                    buf
                },
                async {
                    let mut buf = Vec::new();
                    if let Some(ref mut h) = stderr_handle {
                        let _ = h.read_to_end(&mut buf).await;
                    }
                    buf
                }
            );

            match child.wait().await {
                Ok(status) => {
                    let code = status.code().unwrap_or(-1);
                    match code {
                        0 => StopHookVerdict::Allow,
                        2 => {
                            let reason = String::from_utf8_lossy(&stdout_buf)
                                .trim()
                                .to_string();
                            StopHookVerdict::Block {
                                reason: if reason.is_empty() {
                                    format!("hook '{}' blocked stop", hook.hook_name)
                                } else {
                                    reason
                                },
                            }
                        }
                        _ => StopHookVerdict::Error {
                            hook_name: hook.hook_name.clone(),
                            message: format!(
                                "exit code {code}: {}",
                                String::from_utf8_lossy(&stderr_buf).trim()
                            ),
                        },
                    }
                }
                Err(e) => StopHookVerdict::Error {
                    hook_name: hook.hook_name.clone(),
                    message: format!("wait failed: {e}"),
                },
            }
        } => r,
        _ = tokio::time::sleep(hook.timeout) => {
            let _ = child.kill().await;
            StopHookVerdict::Error {
                hook_name: hook.hook_name.clone(),
                message: "timed out".to_string(),
            }
        }
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            StopHookVerdict::Error {
                hook_name: hook.hook_name.clone(),
                message: "cancelled".to_string(),
            }
        }
    };

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_hook(name: &str, cmd: &str) -> Box<dyn StopHookHandler> {
        Box::new(ShellStopHook::new(name, cmd))
    }

    fn test_ctx() -> StopHookContext {
        StopHookContext {
            final_text: None,
            iterations: 1,
            tool_calls_made: 0,
            stop_reason: "end_turn".into(),
        }
    }

    #[tokio::test]
    async fn test_hook_allow() {
        let hooks = vec![shell_hook("allow", "exit 0")];
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &test_ctx(), &cancel).await;
        assert!(result.blocking_reason().is_none());
    }

    #[tokio::test]
    async fn test_hook_block() {
        let hooks = vec![shell_hook("blocker", "echo 'tests not passing' && exit 2")];
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &test_ctx(), &cancel).await;
        assert_eq!(result.blocking_reason(), Some("tests not passing"));
    }

    #[tokio::test]
    async fn test_hook_error_non_blocking() {
        let hooks = vec![shell_hook("broken", "exit 1")];
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &test_ctx(), &cancel).await;
        assert!(result.blocking_reason().is_none());
        assert_eq!(result.errors().len(), 1);
    }

    #[tokio::test]
    async fn test_hook_timeout() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(
            ShellStopHook::new("slow", "sleep 60").with_timeout(Duration::from_millis(100)),
        )];
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &test_ctx(), &cancel).await;
        assert!(result.blocking_reason().is_none());
        let errors = result.errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.contains("timed out"));
    }

    #[tokio::test]
    async fn test_hook_receives_context_json() {
        let hooks = vec![shell_hook(
            "ctx_checker",
            r#"input=$(cat); echo "$input" | grep -q "end_turn" && echo "found end_turn" && exit 2 || exit 0"#,
        )];
        let ctx = StopHookContext {
            final_text: Some("done".into()),
            iterations: 5,
            tool_calls_made: 3,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert_eq!(result.blocking_reason(), Some("found end_turn"));
    }

    #[tokio::test]
    async fn test_multiple_hooks_first_block_wins() {
        let hooks = vec![
            shell_hook("allow1", "exit 0"),
            shell_hook("blocker", "echo 'blocked' && exit 2"),
            shell_hook("allow2", "exit 0"),
        ];
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &test_ctx(), &cancel).await;
        assert_eq!(result.blocking_reason(), Some("blocked"));
    }

    #[tokio::test]
    async fn test_hook_cancel_kills_child() {
        let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(
            ShellStopHook::new("long_running", "sleep 60")
                .with_timeout(Duration::from_secs(30)),
        )];
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let start = std::time::Instant::now();
        let result = execute_stop_hooks(&hooks, &test_ctx(), &cancel).await;
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_secs(5), "Cancel should be fast, took {:?}", elapsed);
        let errors = result.errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.contains("cancelled"));
    }

    #[test]
    fn test_aggregate_no_errors() {
        let result = StopHookAggregateResult {
            verdicts: vec![StopHookVerdict::Allow, StopHookVerdict::Allow],
        };
        assert!(result.blocking_reason().is_none());
        assert!(result.errors().is_empty());
    }

    #[tokio::test]
    async fn test_trait_based_hook_allow() {
        struct AlwaysAllow;
        #[async_trait::async_trait]
        impl StopHookHandler for AlwaysAllow {
            fn name(&self) -> &str { "always_allow" }
            async fn evaluate(&self, _ctx: &StopHookContext, _cancel: &CancellationToken) -> StopHookVerdict {
                StopHookVerdict::Allow
            }
        }

        let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(AlwaysAllow)];
        let ctx = test_ctx();
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &ctx, &cancel).await;
        assert!(result.blocking_reason().is_none());
    }

    #[tokio::test]
    async fn test_trait_based_hook_block() {
        struct AlwaysBlock;
        #[async_trait::async_trait]
        impl StopHookHandler for AlwaysBlock {
            fn name(&self) -> &str { "always_block" }
            async fn evaluate(&self, _ctx: &StopHookContext, _cancel: &CancellationToken) -> StopHookVerdict {
                StopHookVerdict::Block { reason: "blocked by trait".into() }
            }
        }

        let hooks: Vec<Box<dyn StopHookHandler>> = vec![Box::new(AlwaysBlock)];
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&hooks, &test_ctx(), &cancel).await;
        assert_eq!(result.blocking_reason(), Some("blocked by trait"));
    }
}
```

- [ ] **Step 4: Update mod.rs re-exports**

In `src/agent_loop/mod.rs`, change the stop_hooks re-export line:

```rust
// Old:
pub use stop_hooks::{StopHook, StopHookAggregateResult, StopHookContext, StopHookVerdict};

// New:
pub use stop_hooks::{ShellStopHook, StopHookAggregateResult, StopHookContext, StopHookHandler, StopHookVerdict};
```

- [ ] **Step 5: Update loop_core.rs stop_hooks type**

In `src/agent_loop/loop_core.rs`:

Change the import (line 26):
```rust
// Old:
use super::stop_hooks::{self, StopHook, StopHookContext};
// New:
use super::stop_hooks::{self, StopHookContext, StopHookHandler};
```

Change the field type (line 386):
```rust
// Old:
stop_hooks: Vec<StopHook>,
// New:
stop_hooks: Vec<Box<dyn StopHookHandler>>,
```

Change the `with_stop_hooks` method (line 490):
```rust
// Old:
pub fn with_stop_hooks(mut self, hooks: Vec<StopHook>) -> Self {
// New:
pub fn with_stop_hooks(mut self, hooks: Vec<Box<dyn StopHookHandler>>) -> Self {
```

- [ ] **Step 6: Update integration_probe.rs**

Search for all `StopHook::new` in `src/agent_loop/integration_probe.rs` and replace with `ShellStopHook::new`, wrapping in `Box::new()`:

```rust
// Old:
.with_stop_hooks(vec![
    crate::agent_loop::stop_hooks::StopHook::new("allow_hook", "exit 0"),
])

// New:
.with_stop_hooks(vec![
    Box::new(crate::agent_loop::stop_hooks::ShellStopHook::new("allow_hook", "exit 0")),
])
```

Apply this pattern to all occurrences (approximately 4 locations).

- [ ] **Step 7: Run full compilation check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 8: Run stop_hooks tests**

Run: `cargo test -p alephcore --lib agent_loop::stop_hooks::tests`
Expected: ALL PASS

- [ ] **Step 9: Commit**

```bash
git add src/agent_loop/stop_hooks.rs src/agent_loop/mod.rs src/agent_loop/loop_core.rs src/agent_loop/integration_probe.rs
git commit -m "refactor(stop_hooks): extract StopHookHandler trait, rename StopHook to ShellStopHook"
```

---

## Task 6: Create VerifyStopHook

**Files:**
- Create: `src/agent_loop/verify_stop_hook.rs`
- Modify: `src/agent_loop/mod.rs`

- [ ] **Step 1: Write failing test**

Create `src/agent_loop/verify_stop_hook.rs` with tests first:

```rust
//! VerifyStopHook — triggers the Verify Agent before allowing the loop to stop.

use tokio_util::sync::CancellationToken;

use super::stop_hooks::{StopHookContext, StopHookHandler, StopHookVerdict};

/// Configuration for the verify stop hook.
pub struct VerifyStopHookConfig {
    /// Only trigger for these agent IDs.
    pub trigger_for: Vec<String>,
    /// Skip verification if iterations below this threshold.
    pub min_iterations: usize,
}

impl Default for VerifyStopHookConfig {
    fn default() -> Self {
        Self {
            trigger_for: vec!["main".into(), "coder".into()],
            min_iterations: 3,
        }
    }
}

/// A stop hook that evaluates whether verification should run.
///
/// In this initial implementation, the hook checks conditions and returns
/// Allow (skip) or Block (verification needed). The actual Verify Agent
/// dispatch is wired in at the AgentLoop level when this hook blocks.
pub struct VerifyStopHook {
    config: VerifyStopHookConfig,
    /// The agent ID of the current loop (set at construction).
    current_agent_id: String,
}

impl VerifyStopHook {
    pub fn new(current_agent_id: impl Into<String>, config: VerifyStopHookConfig) -> Self {
        Self {
            config,
            current_agent_id: current_agent_id.into(),
        }
    }

    /// Check if verification should be triggered for the given context.
    fn should_verify(&self, ctx: &StopHookContext) -> bool {
        // Never trigger for the verify agent itself (prevent recursion)
        if self.current_agent_id == "verify" {
            return false;
        }

        // Only trigger for configured agent IDs
        if !self.config.trigger_for.contains(&self.current_agent_id) {
            return false;
        }

        // Skip trivial tasks
        if ctx.iterations < self.config.min_iterations {
            return false;
        }

        true
    }
}

#[async_trait::async_trait]
impl StopHookHandler for VerifyStopHook {
    fn name(&self) -> &str {
        "verify"
    }

    async fn evaluate(
        &self,
        ctx: &StopHookContext,
        _cancel: &CancellationToken,
    ) -> StopHookVerdict {
        if !self.should_verify(ctx) {
            return StopHookVerdict::Allow;
        }

        // Signal that verification is needed.
        // The AgentLoop will dispatch the Verify Agent when it sees this block.
        StopHookVerdict::Block {
            reason: format!(
                "[verify] Verification required for agent '{}' after {} iterations. \
                 Run build checks (cargo check), test suite (cargo test), and lint (cargo clippy). \
                 Report results with VERDICT: PASS/FAIL/PARTIAL.",
                self.current_agent_id, ctx.iterations
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx(iterations: usize) -> StopHookContext {
        StopHookContext {
            final_text: Some("done".into()),
            iterations,
            tool_calls_made: iterations * 2,
            stop_reason: "end_turn".into(),
        }
    }

    #[test]
    fn should_verify_true_for_main_with_enough_iterations() {
        let hook = VerifyStopHook::new("main", VerifyStopHookConfig::default());
        assert!(hook.should_verify(&make_ctx(5)));
    }

    #[test]
    fn should_verify_true_for_coder_with_enough_iterations() {
        let hook = VerifyStopHook::new("coder", VerifyStopHookConfig::default());
        assert!(hook.should_verify(&make_ctx(3)));
    }

    #[test]
    fn should_verify_false_for_verify_agent() {
        let hook = VerifyStopHook::new("verify", VerifyStopHookConfig::default());
        assert!(!hook.should_verify(&make_ctx(10)));
    }

    #[test]
    fn should_verify_false_for_explore_agent() {
        let hook = VerifyStopHook::new("explore", VerifyStopHookConfig::default());
        assert!(!hook.should_verify(&make_ctx(10)));
    }

    #[test]
    fn should_verify_false_for_low_iterations() {
        let hook = VerifyStopHook::new("main", VerifyStopHookConfig::default());
        assert!(!hook.should_verify(&make_ctx(1)));
        assert!(!hook.should_verify(&make_ctx(2)));
    }

    #[tokio::test]
    async fn evaluate_blocks_when_verification_needed() {
        let hook = VerifyStopHook::new("main", VerifyStopHookConfig::default());
        let cancel = CancellationToken::new();
        let verdict = hook.evaluate(&make_ctx(5), &cancel).await;

        match verdict {
            StopHookVerdict::Block { reason } => {
                assert!(reason.contains("[verify]"));
                assert!(reason.contains("cargo check"));
            }
            _ => panic!("Expected Block verdict"),
        }
    }

    #[tokio::test]
    async fn evaluate_allows_when_skipped() {
        let hook = VerifyStopHook::new("explore", VerifyStopHookConfig::default());
        let cancel = CancellationToken::new();
        let verdict = hook.evaluate(&make_ctx(5), &cancel).await;

        match verdict {
            StopHookVerdict::Allow => {}
            _ => panic!("Expected Allow verdict"),
        }
    }

    #[test]
    fn custom_config() {
        let config = VerifyStopHookConfig {
            trigger_for: vec!["custom_agent".into()],
            min_iterations: 10,
        };
        let hook = VerifyStopHook::new("custom_agent", config);
        assert!(!hook.should_verify(&make_ctx(5)));
        assert!(hook.should_verify(&make_ctx(10)));
    }
}
```

- [ ] **Step 2: Add module declaration in mod.rs**

Add to `src/agent_loop/mod.rs`:

```rust
pub mod verify_stop_hook;
```

And add re-export:

```rust
pub use verify_stop_hook::{VerifyStopHook, VerifyStopHookConfig};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib agent_loop::verify_stop_hook::tests`
Expected: ALL PASS

- [ ] **Step 4: Commit**

```bash
git add src/agent_loop/verify_stop_hook.rs src/agent_loop/mod.rs
git commit -m "feat(verify): add VerifyStopHook with conditional verification triggering"
```

---

## Task 7: Delete Old Static Prompt Files

**Files:**
- Delete: `src/agents/prompts/main.md`
- Delete: `src/agents/prompts/explore.md`
- Delete: `src/agents/prompts/coder.md`
- Delete: `src/agents/prompts/researcher.md`

- [ ] **Step 1: Verify no remaining include_str! references**

Run: `grep -r 'include_str!.*prompts/main\|include_str!.*prompts/explore\|include_str!.*prompts/coder\|include_str!.*prompts/researcher' src/`
Expected: No output (all references were removed in Task 2)

- [ ] **Step 2: Delete the files**

```bash
cd /Volumes/TBU/Workspace/Aleph
rm src/agents/prompts/main.md
rm src/agents/prompts/explore.md
rm src/agents/prompts/coder.md
rm src/agents/prompts/researcher.md
```

- [ ] **Step 3: Verify team prompts are preserved**

```bash
ls src/agents/prompts/
```
Expected: `team_leader.md team_worker.md team_explorer.md team_critic.md`

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 5: Commit**

```bash
git add -u src/agents/prompts/
git commit -m "chore(agents): remove static prompt files replaced by section renderers"
```

---

## Task 8: Final Verification

**Files:** None (verification only)

- [ ] **Step 1: Full compilation**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 2: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS

- [ ] **Step 3: Lint check**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
Expected: No warnings

- [ ] **Step 4: Verify section count**

Quick sanity check — count prompt sections:

Run: `ls src/agent_loop/prompt_sections/*.rs | wc -l`
Expected: 24 (18 original + 5 new agent sections + mod.rs)

- [ ] **Step 5: Commit any lint fixes if needed**

```bash
git add -A
git commit -m "chore: fix clippy warnings from agent prompt pipeline refactor"
```
