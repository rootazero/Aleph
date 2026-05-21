# Prompt Modular Assembly Design

**Date**: 2026-04-01
**Status**: Approved
**Scope**: PromptBuilder refactor + behavior content modules + cache boundary + session context

---

## Problem

Aleph's `PromptBuilder` has several structural issues that limit agent behavior quality:

1. **Monolithic `BASE_BEHAVIOR`** — 80+ lines of hardcoded behavior rules in a single `const` string. Not modular, not maintainable, not cache-friendly.
2. **`memory_context` never injected** — `build()` is always called with `None`, making the Memory section dead code.
3. **No cache boundary** — Static and dynamic prompt parts are not separated, breaking prompt cache economics on every session-specific change.
4. **No session-specific guidance** — No dynamic rules based on current tool availability or session state.
5. **Missing behavior modules** — No task philosophy, risk action rules, output style guidance, or tool usage grammar — behavior patterns that Claude Code uses to prevent agent drift.
6. **No environment info** — LLM has no awareness of OS, shell, working directory, or git branch.

## Approach

**Approach B: Modular section functions + cache boundary** — Keep `PromptBuilder` struct, split `BASE_BEHAVIOR` into independent `.md` files loaded via `include_str!`, add static/dynamic boundary marker, introduce `SessionContext` for environment injection.

### Why this approach

- Reuses the proven `model_behaviors/` pattern (`include_str!` + `.md` files)
- No new trait abstractions — `PromptBuilder` keeps its builder API
- Content separated from code — `.md` files can be reviewed and iterated independently
- Fixes multiple issues in one pass: cache boundary + memory signature + environment + behavior content

### Alternatives considered

- **A) Section Registry with trait** — Over-engineered for current needs (only one prompt assembly path). Natural evolution target if subagent-specific prompts are needed later.
- **C) Just expand BASE_BEHAVIOR** — Accumulates tech debt. No cache fix, no environment injection, no modularity.

---

## Design

### File Structure

```
src/agent_loop/
├── prompt_builder.rs              # Refactored (delete BASE_BEHAVIOR, rewrite build())
├── sections/                      # NEW directory
│   ├── mod.rs                     # include_str! + render functions
│   ├── task_philosophy.md         # Task execution discipline
│   ├── risk_actions.md            # Blast radius / risk action rules
│   ├── tool_grammar.md            # Tool usage syntax and patterns
│   ├── output_style.md            # Output efficiency and style
│   ├── persistence.md             # Memory protocol (migrated from BASE_BEHAVIOR)
│   └── guidance/                  # Conditional session guidance
│       ├── browser.md             # Browser tool usage rules
│       ├── code_exec.md           # Code execution / venv rules
│       └── subagent.md            # Subagent delegation rules
└── model_behaviors/               # Existing (unchanged)
    ├── anthropic.md
    ├── openai.md
    ├── gemini.md
    └── ollama.md
```

### SessionContext

```rust
/// Runtime context injected into dynamic prompt sections.
pub struct SessionContext {
    pub os: String,
    pub shell: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub language: String,
}
```

Lightweight struct, no trait. Built once at loop start in `run_loop.rs`.

### build() Signature Change

```rust
pub fn build(
    &self,
    tools: &[ToolInfo],
    memory_context: Option<&str>,
    session: Option<&SessionContext>,
) -> String
```

### Section Order — Static Before Dynamic

```
┌─────────────────────────────────────┐
│         STATIC (cacheable)          │
│                                     │
│  0. Persona prefix (sub-agent)      │
│  1. Identity (soul/default)         │
│  2. Communication Style (soul)      │
│  3. Directives (soul)               │
│  4. Task Philosophy      ← NEW .md │
│  5. Risk Actions         ← NEW .md │
│  6. Tool Grammar         ← NEW .md │
│  7. Output Style         ← NEW .md │
│  8. Persistence          ← NEW .md │
│  9. Model Behavior   (per-LLM .md) │
├─────── CACHE BOUNDARY ──────────────┤
│         DYNAMIC (per-session)       │
│                                     │
│ 10. Tool Usage Rules (capability)   │
│ 11. Available Tools (list)          │
│ 12. Available Skills (filtered)     │
│ 13. Session Guidance     ← NEW     │
│ 14. Environment Info     ← NEW     │
│ 15. Context from Memory            │
│ 16. Additional Instructions (soul) │
│ 17. Discovered Skills (prefetch)   │
└─────────────────────────────────────┘
```

Cache boundary marker: `<!-- CACHE_BOUNDARY -->` inserted between static and dynamic parts. Provider Bridge can split on this marker for Anthropic API `cache_control` in a future iteration.

### Provider-Aware Behavior Layering

New sections (task_philosophy, risk_actions, tool_grammar, output_style, persistence) are **universal** — they define Aleph's core behavior rules regardless of LLM provider. Provider-specific tuning uses the existing `model_behaviors/` overlay system.

**Layering model**: Universal sections (static) → Model Behavior overlay (static, last) → Dynamic sections

Different providers need different emphasis, not different rules:

| Provider | Characteristic | Overlay Strategy |
|----------|---------------|-----------------|
| Anthropic | RLHF well-aligned, naturally follows instructions | Minimal — almost no overlay needed |
| OpenAI | Tends to explain plans instead of executing | Reinforce tool_grammar ("act, don't describe") and output_style ("no filler") |
| Gemini | Tool call format sensitive | Add tool call format constraints and examples |
| Ollama | Local small models, weak instruction following | Simplified rules + concrete examples |

**Changes to existing `model_behaviors/` files**:

- `anthropic.md` — Stays minimal (current single-line comment is correct)
- `openai.md` — Expand with reinforcements referencing universal sections:
  ```markdown
  ## Execution Reinforcement
  
  The tool grammar rules above are critical for you. Specifically:
  - NEVER describe what you plan to do. Call the tool immediately.
  - NEVER list steps without executing them. Execute step 1 now.
  - NEVER ask "Would you like me to proceed?" — proceed.
  - Prefer a 3-line response with a tool call over a 20-line explanation.
  ```
- `gemini.md` — Expand with tool call format constraints
- `ollama.md` — Expand with simplified rule summaries and concrete tool call examples

This avoids the combinatorial explosion of per-provider section variants (5 sections × 4 providers = 20 files) while allowing targeted behavioral tuning where each provider needs it most.

### Section Content Design

#### task_philosophy.md

Core rules preventing agent behavior drift:
- Read existing code before modifying
- Do not add features beyond what was asked
- Do not add error handling for impossible scenarios
- Do not create abstractions for one-time operations
- Diagnose failures before switching tactics
- Fix security vulnerabilities immediately
- Delete unused code — git is the time machine
- Report results honestly

#### risk_actions.md

Blast radius awareness:
- Destructive operations require confirmation
- Hard-to-reverse operations require confirmation
- Actions visible to others require confirmation
- Investigate unexpected state before overwriting

#### tool_grammar.md

Tool usage patterns:
- Call tools immediately when they match the request
- Execute independent tool calls in parallel
- Prefer action over preparation
- Chain multiple tool calls until request is resolved
- Keep user informed in natural language before each tool call

#### output_style.md

Output efficiency:
- Lead with answer, not reasoning
- Skip filler words and preamble
- One sentence over three when possible
- Focus on decisions, milestones, and blockers
- Use media_send for inline media delivery

#### persistence.md

Migrated verbatim from current `BASE_BEHAVIOR` Memory Protocol section:
- When to save memory (corrections, environment facts)
- When to search sessions (past references, cross-session context)
- When to extract skills (complex tasks, non-obvious solutions)

### Session Guidance — Dynamic Generation

```rust
pub fn render_session_guidance(tools: &[ToolInfo]) -> Option<String>
```

Checks tool names in the current registry and conditionally includes guidance:
- `browser_open` / `browser_snapshot` present → include `guidance/browser.md`
- `code_exec` / `bash` present → include `guidance/code_exec.md`
- `subagent` present → include `guidance/subagent.md`

### Environment Info Injection

```rust
pub fn render_environment(ctx: &SessionContext) -> String
```

Renders OS, shell, working directory, git branch, and language preference.

### memory_context — Signature Fix Only

`build()` signature accepts `Option<&str>` for memory context. `run_loop.rs` passes `None` with a `// TODO: inject memory context from MemoryReflectionService` comment. Actual memory query integration is a separate follow-up task.

---

## Changes

| File | Type | Description |
|------|------|-------------|
| `src/agent_loop/sections/mod.rs` | New | `include_str!` all .md files, `render_session_guidance()`, `render_environment()`, `SessionContext` |
| `src/agent_loop/sections/task_philosophy.md` | New | Task execution philosophy |
| `src/agent_loop/sections/risk_actions.md` | New | Risk action rules |
| `src/agent_loop/sections/tool_grammar.md` | New | Tool usage grammar |
| `src/agent_loop/sections/output_style.md` | New | Output style |
| `src/agent_loop/sections/persistence.md` | New | Memory protocol (from BASE_BEHAVIOR) |
| `src/agent_loop/sections/guidance/browser.md` | New | Browser tool guidance |
| `src/agent_loop/sections/guidance/code_exec.md` | New | Code execution guidance |
| `src/agent_loop/sections/guidance/subagent.md` | New | Subagent delegation guidance |
| `src/agent_loop/prompt_builder.rs` | Refactor | Delete `BASE_BEHAVIOR`, add `SessionContext`, rewrite `build()` with static/dynamic split |
| `src/agent_loop/mod.rs` | Modify | Add `pub mod sections;`, export `SessionContext` |
| `src/agent_loop/model_behaviors/openai.md` | Modify | Expand with tool_grammar and output_style reinforcements |
| `src/agent_loop/model_behaviors/gemini.md` | Modify | Expand with tool call format constraints |
| `src/agent_loop/model_behaviors/ollama.md` | Modify | Expand with simplified rules and concrete examples |
| `src/gateway/execution_engine/run_loop.rs` | Modify | Build `SessionContext`, pass to `build()` |

## Deletions

- `BASE_BEHAVIOR` constant (~80 lines) — fully decomposed into `.md` section files
- `DEFAULT_IDENTITY` constant — moved to `sections/mod.rs`

## Testing

Existing tests (13) adapted to new `build()` signature (third param `None`).

New tests:
- `test_build_includes_task_philosophy` — section present in output
- `test_build_includes_risk_actions` — section present in output
- `test_cache_boundary_present` — `CACHE_BOUNDARY` marker in output
- `test_static_before_dynamic` — static sections positioned before dynamic
- `test_session_guidance_conditional` — browser guidance only when browser tools present
- `test_environment_rendering` — SessionContext rendered correctly
- `test_build_backward_compat_none_session` — `None` session produces valid prompt

## Out of Scope

- Provider Bridge cache_control integration (future: split on `CACHE_BOUNDARY` marker)
- Actual memory_context query from MemoryReflectionService (separate task)
- Subagent-specific prompt templates (future iteration — Approach A evolution)
- Section runtime enable/disable toggles (YAGNI)
- Guidance hot-reload from user directory (system rules should not be user-overridable)
