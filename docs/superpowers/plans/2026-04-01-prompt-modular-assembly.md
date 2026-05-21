# Prompt Modular Assembly Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor PromptBuilder from a monolithic BASE_BEHAVIOR constant into modular .md section files with cache boundary, session-specific guidance, environment info injection, and provider-aware behavior layering.

**Architecture:** Split prompt assembly into static sections (compiled-in .md files via `include_str!`) and dynamic sections (rendered at runtime from `SessionContext` and tool registry), separated by a cache boundary marker. Provider-specific behavioral tuning stays in the existing `model_behaviors/` overlay system with expanded content.

**Tech Stack:** Rust, `include_str!` macro, Markdown content files

---

## File Structure

```
src/agent_loop/
├── prompt_builder.rs              # MODIFY — delete BASE_BEHAVIOR/DEFAULT_IDENTITY, new build() signature
├── mod.rs                         # MODIFY — add `pub mod sections;`, export SessionContext
├── sections/                      # CREATE — new module
│   ├── mod.rs                     # include_str! + SessionContext + render functions
│   ├── task_philosophy.md         # NEW — task execution discipline
│   ├── risk_actions.md            # NEW — blast radius rules
│   ├── tool_grammar.md            # NEW — tool usage patterns
│   ├── output_style.md            # NEW — output efficiency
│   ├── persistence.md             # NEW — memory protocol (from BASE_BEHAVIOR)
│   └── guidance/                  # NEW — conditional session guidance
│       ├── browser.md             # NEW — browser tool rules
│       ├── code_exec.md           # NEW — code execution rules
│       └── subagent.md            # NEW — subagent delegation rules
├── model_behaviors/               # MODIFY content only
│   ├── anthropic.md               # unchanged (minimal)
│   ├── openai.md                  # MODIFY — expand with reinforcements
│   ├── gemini.md                  # MODIFY — expand with tool format constraints
│   └── ollama.md                  # MODIFY — expand with simplified rules
└── loop_core.rs                   # MODIFY — pass SessionContext to build()

src/gateway/execution_engine/
└── run_loop.rs                    # MODIFY — construct SessionContext
```

---

### Task 1: Create sections/ module with static .md content files

**Files:**
- Create: `src/agent_loop/sections/mod.rs`
- Create: `src/agent_loop/sections/task_philosophy.md`
- Create: `src/agent_loop/sections/risk_actions.md`
- Create: `src/agent_loop/sections/tool_grammar.md`
- Create: `src/agent_loop/sections/output_style.md`
- Create: `src/agent_loop/sections/persistence.md`

- [ ] **Step 1: Create `task_philosophy.md`**

```markdown
## Task Execution Philosophy

- ALWAYS use available tools to gather information and take actions. Do NOT answer from memory or guess when a tool can provide the answer.
- When the user asks you to do something and a matching tool exists, call it immediately rather than describing what you would do.
- Continue working until the user's request is fully resolved. Chain multiple tool calls if needed.
- Read existing code before modifying it. Understand context before suggesting changes.
- Do not add features, refactoring, or "improvements" beyond what was asked. A bug fix does not need surrounding code cleaned up. A simple feature does not need extra configurability.
- Do not add error handling, fallbacks, or validation for scenarios that cannot happen. Trust internal code and framework guarantees. Only validate at system boundaries (user input, external APIs).
- Do not create helpers, utilities, or abstractions for one-time operations. Three similar lines of code is better than a premature abstraction.
- When an approach fails, diagnose why before switching tactics. Do not retry the identical action blindly, but do not abandon a viable approach after a single failure either.
- If you discover a security vulnerability in code you are editing, fix it immediately.
- Delete unused code completely. Do not comment it out — git is the time machine.
- Report results honestly. Never claim tests passed without running them.
```

- [ ] **Step 2: Create `risk_actions.md`**

```markdown
## Actions with Care

Consider the reversibility and blast radius of every action before executing it.

Actions that require user confirmation before proceeding:
- **Destructive operations**: deleting files, dropping database tables, killing processes, overwriting uncommitted changes
- **Hard-to-reverse operations**: force-push, amending published commits, removing or downgrading packages
- **Actions visible to others**: pushing code, creating/closing/commenting on PRs or issues, sending messages to external services
- **Modifying shared state**: changing shared infrastructure, permissions, or CI/CD pipelines

When encountering unexpected state (unfamiliar files, branches, or configuration), investigate before deleting or overwriting — it may represent in-progress work.

Do not use destructive actions as shortcuts to bypass obstacles. Resolve merge conflicts rather than discarding changes. If a lock file exists, investigate what process holds it rather than deleting it.
```

- [ ] **Step 3: Create `tool_grammar.md`**

```markdown
## Tool Usage Grammar

When a tool directly matches the user's request, call it IMMEDIATELY. Do not explain what you plan to do — execute.

**Parallel execution:**
- When multiple tool calls have no dependencies between them, execute them all in parallel.
- When calls depend on previous results, execute them sequentially.

**Efficiency:**
- Prefer action over preparation. A failed attempt with a clear error message is more useful than exhausting the token budget on exploration.
- Continue working until the request is fully resolved. Chain multiple tool calls if needed.

**Persistence:**
- When a tool call fails, analyze the error carefully.
- Retry with corrected parameters or a different approach.
- If that fails, try a completely different strategy to achieve the same goal.
- NEVER give up after just 1-2 attempts. Only stop if you have genuinely exhausted all possible approaches AND explained what you tried.

**Keep the user informed:**
- Before each tool call, briefly state what you are about to do in natural, conversational language.
- Do NOT expose raw tool names, parameters, or JSON.
- Good: "Let me check your calendar..." or "I'll search for that file now."
- Bad: "Calling calendar_search with params {...}"
```

- [ ] **Step 4: Create `output_style.md`**

```markdown
## Output Style

- Lead with the answer or action, not the reasoning. Skip filler words, preamble, and unnecessary transitions.
- Do not restate what the user said — just do it.
- If you can say it in one sentence, do not use three. Prefer short, direct sentences over long explanations.
- Focus text output on:
  - Decisions that need the user's input
  - Status updates at natural milestones
  - Errors or blockers that change the plan
- Provide concise summaries of actions taken and results obtained.
- When delivering media (images, audio, video), use the media_send tool so content appears inline. Do not just paste URLs.
```

- [ ] **Step 5: Create `persistence.md`**

This is a verbatim migration from the current `BASE_BEHAVIOR` Memory Protocol section.

```markdown
## Memory Protocol

### When to Save Memory
- User corrections and preferences → highest priority, prevents repeating mistakes.
- Environment facts (OS, tools, project conventions) → reduces future context gathering.
- Do NOT save: task progress, session outcomes, completed-work logs, or temporary TODO state.

### When to Search Sessions
- User references something from a past conversation.
- You suspect relevant cross-session context exists.
- Before asking user to repeat information they may have already told you.
- Use the session_search tool — sessions have verbatim transcripts.

### When to Extract Skills
- After completing a complex task (5+ tool calls).
- After fixing a tricky error with a non-obvious solution.
- After discovering a reusable workflow or pattern.
- Save via memory as a Lesson-type fact with clear, reusable steps.
```

- [ ] **Step 6: Create `sections/mod.rs` with include_str! and SessionContext**

```rust
//! Prompt sections — modular content for PromptBuilder.
//!
//! Static sections are compiled into the binary via `include_str!`.
//! Dynamic sections are rendered at runtime from `SessionContext`.

use std::collections::HashSet;

// =============================================================================
// Static section content (compiled-in .md files)
// =============================================================================

pub const TASK_PHILOSOPHY: &str = include_str!("task_philosophy.md");
pub const RISK_ACTIONS: &str = include_str!("risk_actions.md");
pub const TOOL_GRAMMAR: &str = include_str!("tool_grammar.md");
pub const OUTPUT_STYLE: &str = include_str!("output_style.md");
pub const PERSISTENCE: &str = include_str!("persistence.md");

pub const DEFAULT_IDENTITY: &str = "You are a helpful personal AI assistant.";

// =============================================================================
// Conditional guidance content
// =============================================================================

const BROWSER_GUIDANCE: &str = include_str!("guidance/browser.md");
const CODE_EXEC_GUIDANCE: &str = include_str!("guidance/code_exec.md");
const SUBAGENT_GUIDANCE: &str = include_str!("guidance/subagent.md");

// =============================================================================
// SessionContext
// =============================================================================

/// Runtime context injected into dynamic prompt sections.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    /// Operating system name (e.g. "macos", "linux")
    pub os: String,
    /// User's shell (e.g. "/bin/zsh")
    pub shell: String,
    /// Current working directory
    pub cwd: String,
    /// Current git branch, if in a git repository
    pub git_branch: Option<String>,
    /// User's preferred language (e.g. "zh-CN", "en")
    pub language: String,
}

// =============================================================================
// Dynamic section renderers
// =============================================================================

use super::prompt_builder::ToolInfo;

/// Render environment info section from SessionContext.
pub fn render_environment(ctx: &SessionContext) -> String {
    let mut lines = vec![
        format!("- OS: {}", ctx.os),
        format!("- Shell: {}", ctx.shell),
        format!("- Working Directory: {}", ctx.cwd),
    ];
    if let Some(ref branch) = ctx.git_branch {
        lines.push(format!("- Git Branch: {}", branch));
    }
    if !ctx.language.is_empty() {
        lines.push(format!("- Language: {}", ctx.language));
    }
    lines.join("\n")
}

/// Render session-specific guidance based on available tools.
///
/// Returns `None` if no tool-specific guidance applies.
pub fn render_session_guidance(tools: &[ToolInfo]) -> Option<String> {
    let tool_names: HashSet<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let mut parts: Vec<&str> = Vec::new();

    if tool_names.contains("browser_open") || tool_names.contains("browser_snapshot") {
        parts.push(BROWSER_GUIDANCE);
    }
    if tool_names.contains("code_exec") || tool_names.contains("bash") {
        parts.push(CODE_EXEC_GUIDANCE);
    }
    if tool_names.contains("subagent") {
        parts.push(SUBAGENT_GUIDANCE);
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_sections_are_non_empty() {
        assert!(!TASK_PHILOSOPHY.is_empty());
        assert!(!RISK_ACTIONS.is_empty());
        assert!(!TOOL_GRAMMAR.is_empty());
        assert!(!OUTPUT_STYLE.is_empty());
        assert!(!PERSISTENCE.is_empty());
    }

    #[test]
    fn render_environment_basic() {
        let ctx = SessionContext {
            os: "macos".into(),
            shell: "/bin/zsh".into(),
            cwd: "/home/user/project".into(),
            git_branch: Some("main".into()),
            language: "zh-CN".into(),
        };
        let result = render_environment(&ctx);
        assert!(result.contains("macos"));
        assert!(result.contains("/bin/zsh"));
        assert!(result.contains("main"));
        assert!(result.contains("zh-CN"));
    }

    #[test]
    fn render_environment_no_git() {
        let ctx = SessionContext {
            os: "linux".into(),
            shell: "/bin/bash".into(),
            cwd: "/tmp".into(),
            git_branch: None,
            language: String::new(),
        };
        let result = render_environment(&ctx);
        assert!(!result.contains("Git Branch"));
        assert!(!result.contains("Language"));
    }

    #[test]
    fn session_guidance_empty_when_no_matching_tools() {
        let tools = vec![ToolInfo {
            name: "memory_store".into(),
            description: "Store memory".into(),
            parameters_schema: None,
        }];
        assert!(render_session_guidance(&tools).is_none());
    }

    #[test]
    fn session_guidance_includes_browser_when_present() {
        let tools = vec![ToolInfo {
            name: "browser_open".into(),
            description: "Open browser".into(),
            parameters_schema: None,
        }];
        let result = render_session_guidance(&tools).unwrap();
        assert!(result.contains("browser") || result.contains("Browser"));
    }

    #[test]
    fn session_guidance_includes_subagent_when_present() {
        let tools = vec![ToolInfo {
            name: "subagent".into(),
            description: "Run subagent".into(),
            parameters_schema: None,
        }];
        let result = render_session_guidance(&tools).unwrap();
        assert!(result.contains("subagent") || result.contains("Subagent") || result.contains("sub-agent"));
    }
}
```

- [ ] **Step 7: Register module in `agent_loop/mod.rs`**

Add `pub mod sections;` after line 9 (`pub mod factory;`), and add export:

```rust
pub mod sections;
```

```rust
pub use sections::SessionContext;
```

- [ ] **Step 8: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -30`

Expected: SUCCESS (sections module compiles, no consumers yet)

- [ ] **Step 9: Run section tests**

Run: `cargo test -p alephcore sections::tests -- --nocapture 2>&1 | tail -20`

Expected: All 5 tests pass

- [ ] **Step 10: Commit**

```bash
git add src/agent_loop/sections/
git add src/agent_loop/mod.rs
git commit -m "feat(prompt): add modular sections module with static .md content and SessionContext"
```

---

### Task 2: Create guidance/ conditional .md files

**Files:**
- Create: `src/agent_loop/sections/guidance/browser.md`
- Create: `src/agent_loop/sections/guidance/code_exec.md`
- Create: `src/agent_loop/sections/guidance/subagent.md`

- [ ] **Step 1: Create `guidance/browser.md`**

```markdown
### Browser Tools

- ALWAYS use browser_open/browser_snapshot/browser_click to open URLs and interact with web pages. Do NOT use desktop tools to launch a browser application.
- The browser runs in headless mode by default (fast, no visible window). Only use profile="user" when the user explicitly asks to open a real/visible browser.
- If a browser tool fails, wait briefly and retry — browser operations are inherently flaky and retrying usually works.
- Prefer targeted CSS selectors (click, fill) over full-page snapshots. Use evaluate_script with specific queries rather than dumping entire page content.
```

- [ ] **Step 2: Create `guidance/code_exec.md`**

```markdown
### Code Execution

- NEVER use the system Python directly. Use the shared virtual environment at `~/.aleph/.venv/` for all global tools, packages, and quick scripts: `source ~/.aleph/.venv/bin/activate && uv pip install <packages>`.
- If the venv does not exist, create it first: `uv venv ~/.aleph/.venv`.
- For standalone Python projects, create `.venv` inside the project directory under the workspace.
```

- [ ] **Step 3: Create `guidance/subagent.md`**

```markdown
### Subagent Delegation

When dispatching a subagent, write the prompt like a briefing for a smart colleague who just walked into the room — they have no context from this conversation.

- Explain what you are trying to accomplish and why.
- Describe what you have already learned or ruled out.
- Give enough context about the surrounding problem that the subagent can make judgment calls rather than just following a narrow instruction.
- If you need a short response, say so.
- NEVER delegate understanding. Do not write "based on your findings, fix the bug." Include file paths, line numbers, and what specifically to change.
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -20`

Expected: SUCCESS

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/sections/guidance/
git commit -m "feat(prompt): add conditional session guidance for browser, code_exec, and subagent"
```

---

### Task 3: Refactor PromptBuilder — delete BASE_BEHAVIOR, rewrite build()

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs`

- [ ] **Step 1: Write the failing test for new build() signature**

Add this test at the end of the `#[cfg(test)] mod tests` block in `prompt_builder.rs`:

```rust
#[test]
fn test_build_with_session_context() {
    use crate::agent_loop::sections::SessionContext;

    let ctx = SessionContext {
        os: "macos".into(),
        shell: "/bin/zsh".into(),
        cwd: "/home/user/project".into(),
        git_branch: Some("main".into()),
        language: "zh-CN".into(),
    };

    let prompt = PromptBuilder::new().build(&[], None, Some(&ctx));

    assert!(prompt.contains("# Environment"));
    assert!(prompt.contains("macos"));
    assert!(prompt.contains("main"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore test_build_with_session_context 2>&1 | tail -10`

Expected: FAIL — `build()` only takes 2 args

- [ ] **Step 3: Rewrite prompt_builder.rs**

Replace the entire `build()` method and delete `BASE_BEHAVIOR` and `DEFAULT_IDENTITY` constants. The full new implementation:

```rust
use super::sections::{self, SessionContext};

const SECTION_SEPARATOR: &str = "\n\n---\n\n";
const CACHE_BOUNDARY: &str = "\n\n<!-- CACHE_BOUNDARY -->\n\n";

// Delete: const DEFAULT_IDENTITY (moved to sections::DEFAULT_IDENTITY)
// Delete: const BASE_BEHAVIOR (decomposed into sections/*.md)
```

New `build()` method:

```rust
pub fn build(
    &self,
    tools: &[ToolInfo],
    memory_context: Option<&str>,
    session: Option<&SessionContext>,
) -> String {
    let mut static_sections: Vec<String> = Vec::new();
    let mut dynamic_sections: Vec<String> = Vec::new();

    // === STATIC SECTIONS (cacheable) ===

    // 0. Persona prefix — highest priority, overrides default identity
    if let Some(persona) = &self.persona_prefix {
        static_sections.push(format!("# Persona\n\n{}", persona));
    }

    // 1. Identity
    let identity = self
        .soul_identity
        .as_deref()
        .unwrap_or(sections::DEFAULT_IDENTITY);
    static_sections.push(format!("# Identity\n\n{}", identity));

    // 2. Communication Style
    if let Some(tone) = &self.soul_tone {
        static_sections.push(format!("# Communication Style\n\n{}", tone));
    }

    // 3. Directives
    if !self.soul_directives.is_empty() {
        let bullets: String = self
            .soul_directives
            .iter()
            .map(|d| format!("- {}", d))
            .collect::<Vec<_>>()
            .join("\n");
        static_sections.push(format!("# Directives\n\n{}", bullets));
    }

    // 4. Task Philosophy
    static_sections.push(format!("# Task Execution\n\n{}", sections::TASK_PHILOSOPHY));

    // 5. Risk Actions
    static_sections.push(format!("# Risk Awareness\n\n{}", sections::RISK_ACTIONS));

    // 6. Tool Grammar
    static_sections.push(format!("# Tool Usage\n\n{}", sections::TOOL_GRAMMAR));

    // 7. Output Style
    static_sections.push(format!("# Output\n\n{}", sections::OUTPUT_STYLE));

    // 8. Persistence
    static_sections.push(format!("# Persistence\n\n{}", sections::PERSISTENCE));

    // 9. Model Behavior — LLM-family-specific overlay
    if let Some(behavior) = &self.model_behavior {
        static_sections.push(format!("# Model Behavior\n\n{}", behavior));
    }

    // === DYNAMIC SECTIONS (per-session) ===

    // 10. Tool Usage Rules (capability rules from agent config)
    if let Some(rules) = &self.capability_rules {
        dynamic_sections.push(format!("# Tool Usage Rules\n\n{}", rules));
    }

    // 11. Available Tools
    if !tools.is_empty() {
        let tool_list: String = tools
            .iter()
            .map(|t| format!("- **{}**: {}", t.name, t.description))
            .collect::<Vec<_>>()
            .join("\n");
        dynamic_sections.push(format!("# Available Tools\n\n{}", tool_list));
    }

    // 12. Available Skills (scope-filtered from SkillSystem v2)
    if let Some(ref skills) = self.eligible_skills {
        let active_tool_names: Vec<&str> =
            tools.iter().map(|t| t.name.as_str()).collect();
        let filtered: Vec<&SkillManifest> = skills
            .iter()
            .filter(|s| match *s.scope() {
                PromptScope::System => true,
                PromptScope::Tool => s
                    .bound_tool()
                    .is_some_and(|bound| active_tool_names.contains(&bound)),
                PromptScope::Standalone | PromptScope::Disabled => false,
            })
            .collect();

        if !filtered.is_empty() {
            let xml = build_skills_prompt_xml(&filtered);
            dynamic_sections.push(format!(
                "# Available Skills\n\nYou can invoke skills using the `skill` tool. \
                 Skills provide specialized instructions for specific tasks.\n\
                 {}\n\n{}",
                crate::skill::prompt::DEFERRED_LOADING_GUIDANCE,
                xml
            ));
        }
    }

    // 13. Session Guidance (conditional on available tools)
    if let Some(guidance) = sections::render_session_guidance(tools) {
        dynamic_sections.push(format!("# Session Guidance\n\n{}", guidance));
    }

    // 14. Environment Info
    if let Some(ctx) = session {
        dynamic_sections.push(format!(
            "# Environment\n\n{}",
            sections::render_environment(ctx)
        ));
    }

    // 15. Context from Memory
    if let Some(ctx) = memory_context {
        dynamic_sections.push(format!("# Context from Memory\n\n{}", ctx));
    }

    // 16. Additional Instructions
    if let Some(instructions) = &self.custom_instructions {
        dynamic_sections.push(format!(
            "# Additional Instructions\n\n{}",
            instructions
        ));
    }

    // 17. Discovered Skills (from async prefetch)
    let skill_section = self
        .skill_info_section
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(ref section) = skill_section {
        dynamic_sections.push(section.clone());
    }

    // Assemble with cache boundary
    let static_part = static_sections.join(SECTION_SEPARATOR);
    if dynamic_sections.is_empty() {
        static_part
    } else {
        let dynamic_part = dynamic_sections.join(SECTION_SEPARATOR);
        format!("{}{}{}", static_part, CACHE_BOUNDARY, dynamic_part)
    }
}
```

- [ ] **Step 4: Update all existing test call sites**

Every `.build(&[], None)` becomes `.build(&[], None, None)`.
Every `.build(&[], Some("..."))` becomes `.build(&[], Some("..."), None)`.
Every `.build(&tools, None)` becomes `.build(&tools, None, None)`.

There are 16 test call sites to update. Use find-and-replace:
- `.build(&[], None)` → `.build(&[], None, None)` (13 occurrences)
- `.build(&[], Some(` → stays the same but add `, None)` at end (1 occurrence)
- `.build(&tools, None)` → `.build(&tools, None, None)` (2 occurrences)

Also update test assertions:
- `test_build_empty_is_valid`: change `assert!(prompt.contains("assistant"))` to `assert!(prompt.contains("# Identity"))` (DEFAULT_IDENTITY text changed location)
- `test_build_includes_orchestration_prompt`: **delete this test** — `CODE TASK ORCHESTRATION` no longer exists in BASE_BEHAVIOR
- `base_behavior_contains_memory_protocol`: **rewrite** to check sections:
  ```rust
  #[test]
  fn sections_contain_memory_protocol() {
      use crate::agent_loop::sections;
      assert!(sections::PERSISTENCE.contains("Memory Protocol"));
      assert!(sections::PERSISTENCE.contains("When to Save Memory"));
      assert!(sections::PERSISTENCE.contains("When to Search Sessions"));
      assert!(sections::PERSISTENCE.contains("When to Extract Skills"));
  }
  ```

- [ ] **Step 5: Run test to verify new test passes**

Run: `cargo test -p alephcore test_build_with_session_context 2>&1 | tail -10`

Expected: PASS

- [ ] **Step 6: Run all prompt_builder tests**

Run: `cargo test -p alephcore prompt_builder 2>&1 | tail -20`

Expected: All tests pass

- [ ] **Step 7: Compile check full crate**

Run: `cargo check -p alephcore 2>&1 | head -30`

Expected: SUCCESS (loop_core.rs will fail — fixed in Task 4)

- [ ] **Step 8: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "refactor(prompt): decompose BASE_BEHAVIOR into modular sections with cache boundary"
```

---

### Task 4: Update call sites — loop_core.rs and run_loop.rs

**Files:**
- Modify: `src/agent_loop/loop_core.rs` (2 call sites, lines 572 and 1242)
- Modify: `src/gateway/execution_engine/run_loop.rs` (SessionContext construction)

- [ ] **Step 1: Update loop_core.rs call sites**

At line 572, change:
```rust
let mut system_prompt = self.prompt_builder.build(&tool_infos, None);
```
to:
```rust
let mut system_prompt = self.prompt_builder.build(&tool_infos, None, None);
```

At line 1242, change:
```rust
system_prompt = self.prompt_builder.build(&tool_infos, None);
```
to:
```rust
system_prompt = self.prompt_builder.build(&tool_infos, None, None);
```

Note: `loop_core.rs` passes `None` for session context because it doesn't own the `SessionContext` — the context is baked into the prompt by the outer `run_loop.rs` caller. This is intentional: `AgentLoop` is a generic loop, `SessionContext` is injected at the gateway level.

Wait — actually, `AgentLoop` calls `build()` internally during the loop. So we need `AgentLoop` to hold the `SessionContext` and pass it through. Let me adjust:

Add a field to `AgentLoop`:

```rust
/// Optional session context for environment info injection.
session_context: Option<sections::SessionContext>,
```

Add a builder method:

```rust
pub fn with_session_context(mut self, ctx: sections::SessionContext) -> Self {
    self.session_context = Some(ctx);
    self
}
```

Update both `build()` call sites in `loop_core.rs`:

```rust
// line 572
let mut system_prompt = self.prompt_builder.build(&tool_infos, None, self.session_context.as_ref());

// line 1242
system_prompt = self.prompt_builder.build(&tool_infos, None, self.session_context.as_ref());
```

- [ ] **Step 2: Construct SessionContext in run_loop.rs**

After the `prompt_builder` is built (around line 319 in run_loop.rs), add:

```rust
// Build session context for environment info injection
let session_ctx = crate::agent_loop::SessionContext {
    os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    shell: std::env::var("SHELL").unwrap_or_default(),
    cwd: agent.workspace().to_string_lossy().to_string(),
    git_branch: detect_git_branch(agent.workspace()).await,
    language: agent.config().language.clone().unwrap_or_default(),
};
```

Then chain it onto the `AgentLoop` builder (around line 362):

```rust
.with_session_context(session_ctx)
```

Add the helper function at the bottom of `run_loop.rs`:

```rust
/// Detect current git branch from workspace path.
async fn detect_git_branch(workspace: &std::path::Path) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(workspace)
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() || branch == "HEAD" {
            None
        } else {
            Some(branch)
        }
    } else {
        None
    }
}
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -30`

Expected: SUCCESS

- [ ] **Step 4: Run all tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`

Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/loop_core.rs
git add src/gateway/execution_engine/run_loop.rs
git commit -m "feat(prompt): inject SessionContext into agent loop for environment info"
```

---

### Task 5: Expand model_behaviors/ provider overlays

**Files:**
- Modify: `src/agent_loop/model_behaviors/openai.md`
- Modify: `src/agent_loop/model_behaviors/gemini.md`
- Modify: `src/agent_loop/model_behaviors/ollama.md`

- [ ] **Step 1: Expand `openai.md`**

Replace existing content with:

```markdown
## Execution Directives

You are an autonomous agent with full tool access. Your primary mode is EXECUTION, not conversation.

**Reinforcement of core rules — these are critical for you:**

- ALWAYS call tools proactively. Never ask "would you like me to..." — just do it.
- When you have enough context to act, act immediately. Do not explain what you plan to do.
- Chain multiple tool calls in sequence. Complete one, then proceed to the next without pausing.
- If a task requires information, use tools to get it. Do not ask the user to provide what you can look up.
- Prefer action over explanation. A 3-line response with a tool call beats a 20-line explanation.

**Anti-patterns to avoid:**

- "I can help you with that! Let me..." → Just call the tool.
- "Would you like me to proceed?" → Proceed.
- "Here's what I would do: 1. ... 2. ... 3. ..." → Do step 1 now.
- Listing steps without executing them → Execute step 1, then step 2, then step 3.
- Adding filler words like "Certainly!", "Of course!", "Great question!" → Skip them entirely.
```

- [ ] **Step 2: Expand `gemini.md`**

Replace existing content with:

```markdown
## Execution Directives

You are an autonomous agent with full tool access. Your primary mode is EXECUTION, not conversation.

**Rules:**

- ALWAYS call tools proactively. Do not describe actions — execute them.
- Chain multiple tool calls in sequence without pausing for confirmation.
- When the user's request maps to a tool, call it immediately.
- Prefer action over explanation.

**Tool call format:**

- Provide tool arguments as valid JSON. Do not include comments or trailing commas in JSON.
- When a tool expects a string argument, pass a plain string — not a JSON object.
- If a tool call fails with a format error, check the argument types and retry.
```

- [ ] **Step 3: Expand `ollama.md`**

Replace existing content with:

```markdown
## Tool Usage Guide

You have access to tools. When a task matches an available tool, you MUST use it.

**How to use tools:**

1. Read the tool descriptions carefully.
2. Call the tool with the required parameters.
3. Wait for the result before proceeding.
4. If a tool call fails, read the error message and try again with corrected parameters.

**Important rules:**

- Always prefer tool calls over text responses when a matching tool exists.
- Execute tasks step by step — one tool call at a time.
- Be concise in your text responses.
- When the user asks to do something and you have a matching tool, call the tool immediately. Do not ask for permission.
- When multiple tools are needed, call them one after another.
- Do not make up information. If you need data, use a tool to get it.
```

- [ ] **Step 4: Run model_behaviors tests**

Run: `cargo test -p alephcore model_behaviors 2>&1 | tail -20`

Expected: All tests pass (tests check for `contains("Execution Directives")` etc.)

- [ ] **Step 5: Commit**

```bash
git add src/agent_loop/model_behaviors/openai.md
git add src/agent_loop/model_behaviors/gemini.md
git add src/agent_loop/model_behaviors/ollama.md
git commit -m "feat(prompt): expand model behavior overlays with provider-specific reinforcements"
```

---

### Task 6: Add new integration tests and verify

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs` (add new tests)

- [ ] **Step 1: Add cache boundary test**

```rust
#[test]
fn test_cache_boundary_present() {
    let tools = vec![ToolInfo {
        name: "web_search".into(),
        description: "Search the web".into(),
        parameters_schema: None,
    }];
    let prompt = PromptBuilder::new().build(&tools, None, None);
    assert!(
        prompt.contains("<!-- CACHE_BOUNDARY -->"),
        "Cache boundary marker should be present when dynamic sections exist"
    );
}
```

- [ ] **Step 2: Add static-before-dynamic ordering test**

```rust
#[test]
fn test_static_before_dynamic() {
    let tools = vec![ToolInfo {
        name: "web_search".into(),
        description: "Search the web".into(),
        parameters_schema: None,
    }];
    let prompt = PromptBuilder::new()
        .with_capability_rules("Always confirm.")
        .build(&tools, None, None);

    let boundary_pos = prompt.find("<!-- CACHE_BOUNDARY -->").unwrap();
    let task_pos = prompt.find("# Task Execution").unwrap();
    let tools_pos = prompt.find("# Available Tools").unwrap();

    assert!(task_pos < boundary_pos, "Task Execution (static) should be before cache boundary");
    assert!(tools_pos > boundary_pos, "Available Tools (dynamic) should be after cache boundary");
}
```

- [ ] **Step 3: Add session guidance conditional test**

```rust
#[test]
fn test_session_guidance_only_with_matching_tools() {
    let no_browser = vec![ToolInfo {
        name: "memory_store".into(),
        description: "Store memory".into(),
        parameters_schema: None,
    }];
    let prompt_no = PromptBuilder::new().build(&no_browser, None, None);
    assert!(!prompt_no.contains("# Session Guidance"));

    let with_browser = vec![ToolInfo {
        name: "browser_open".into(),
        description: "Open browser".into(),
        parameters_schema: None,
    }];
    let prompt_yes = PromptBuilder::new().build(&with_browser, None, None);
    assert!(prompt_yes.contains("# Session Guidance"));
    assert!(prompt_yes.contains("headless"));
}
```

- [ ] **Step 4: Add backward compatibility test**

```rust
#[test]
fn test_build_backward_compat_none_session() {
    // Ensure None session still produces a valid prompt with all static sections
    let prompt = PromptBuilder::new().build(&[], None, None);
    assert!(prompt.contains("# Identity"));
    assert!(prompt.contains("# Task Execution"));
    assert!(prompt.contains("# Risk Awareness"));
    assert!(prompt.contains("# Tool Usage"));
    assert!(prompt.contains("# Output"));
    assert!(prompt.contains("# Persistence"));
    assert!(!prompt.contains("# Environment"));
}
```

- [ ] **Step 5: Run all tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`

Expected: All tests pass

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`

Expected: No warnings

- [ ] **Step 7: Commit**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "test(prompt): add integration tests for cache boundary, section ordering, and session guidance"
```

---

### Task 7: Final verification and cleanup

- [ ] **Step 1: Verify BASE_BEHAVIOR is fully deleted**

Run: `grep -r "BASE_BEHAVIOR" src/agent_loop/ 2>&1`

Expected: No matches (constant fully removed)

- [ ] **Step 2: Verify DEFAULT_IDENTITY moved**

Run: `grep -r "DEFAULT_IDENTITY" src/agent_loop/ 2>&1`

Expected: Only in `sections/mod.rs` (definition) and `prompt_builder.rs` (usage via `sections::DEFAULT_IDENTITY`)

- [ ] **Step 3: Full build**

Run: `cargo build -p alephcore 2>&1 | tail -10`

Expected: SUCCESS

- [ ] **Step 4: Full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`

Expected: All tests pass

- [ ] **Step 5: Verify prompt output sanity**

Run a quick sanity check that the assembled prompt looks reasonable:

```bash
cargo test -p alephcore test_build_backward_compat_none_session -- --nocapture 2>&1 | head -50
```

Expected: Test passes, output shows structured prompt with section headers

- [ ] **Step 6: Final commit (if any remaining changes)**

```bash
git status
# If clean, skip. Otherwise:
git add -A
git commit -m "chore(prompt): cleanup after modular assembly refactor"
```
