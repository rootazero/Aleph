# Agent Prompt Pipeline & Verification Agent Design

**Date**: 2026-04-01
**Status**: Approved
**Scope**: Agent behavior prompts, SubAgent prompt assembly pipeline, Verify Agent + StopHook

---

## Background

Aleph's built-in agents (explore, coder, researcher) currently use 5-10 line static prompt files that lack behavioral constraints, output format specifications, and role-specific mental models. The SubAgent prompt construction bypasses the Section Registry + Cache Partitioning system (`PromptBuilder`) that the main agent uses, resulting in no cache sharing and no budget enforcement for sub-agents.

Additionally, there is no verification mechanism to prevent agents from claiming completion prematurely — a well-known LLM failure mode.

### Reference

This design draws lessons from Claude Code's Agent system while respecting Aleph's architectural redlines:
- **R8 (LLM Sovereignty)**: Behavioral constraints use natural language guidance in prompts, not deterministic code replacing LLM judgment
- **R10 (Intelligence in Prompt)**: Agent specialization lives in prompt sections, not middleware
- **Dual-layer safety**: Prompt guidance + runtime tool denial (existing `denied_tools`) for defense in depth

---

## Design

### Part 1: AgentDef Extension

**Remove** `system_prompt: String` field from `AgentDef`.
**Add** `prompt_sections: Vec<String>` field — declares which agent-specific prompt sections this agent needs.

```rust
pub struct AgentDef {
    pub id: String,
    pub mode: AgentMode,
    pub prompt_sections: Vec<String>,  // replaces system_prompt
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub max_iterations: Option<u32>,
}
```

`AgentDef::new()` signature changes from `(id, mode, system_prompt)` to `(id, mode)`. New builder method: `.with_prompt_sections(vec![...])`.

### Part 2: Agent-Specific Prompt Sections

New section renderers in `src/agent_loop/prompt_sections/`:

| Section | File | Agent | Stability | Priority | Protected |
|---------|------|-------|-----------|----------|-----------|
| `agent_role` | agent_role.rs | All SubAgents | Dynamic | 55 | yes |
| `explore_constraints` | explore_constraints.rs | explore | Dynamic | 60 | no |
| `coder_guidelines` | coder_guidelines.rs | coder | Dynamic | 60 | no |
| `researcher_protocol` | researcher_protocol.rs | researcher | Dynamic | 60 | no |
| `verify_protocol` | verify_protocol.rs | verify | Dynamic | 60 | no |

**`agent_role`** — shared base for all sub-agents:
- Role: "You are a specialized sub-agent of Aleph"
- Contract: complete the task fully, report concisely, don't leave partial work
- Constraint: stay within your declared tool set

**`explore_constraints`** — read-only exploration specialist:
- Prefer parallel tool calls (glob + grep simultaneously)
- Start broad (directory structure), then narrow (specific files)
- Hard constraints: file modification blocked at runtime, max 20 iterations
- Output: structured findings with file paths and recommended next steps

**`coder_guidelines`** — code writing specialist:
- Follow project coding conventions (read before writing)
- Make minimal, focused changes
- Verify changes compile before reporting completion
- Output: summary of changes made with file paths

**`researcher_protocol`** — information gathering specialist:
- Cross-reference multiple sources
- Distinguish facts from speculation
- Hard constraints: no file modifications, max 15 iterations
- Output: structured research report with sources

**`verify_protocol`** — adversarial verifier:
- Mindset: "try to break it" — assume bugs until proven otherwise
- Mandatory checks: cargo check, cargo test, cargo clippy
- Change-type specific checks (refactoring → API surface, new features → test coverage)
- Adversarial probes after mandatory checks pass
- Output: VERDICT (PASS/FAIL/PARTIAL) with check results and issues found
- Hard rule: never output PASS without actually running checks

### Part 3: SubAgent Prompt Assembly Pipeline

New constructor `PromptBuilder::for_agent(agent: &AgentDef)`:

1. Register shared Stable sections via `with_default_behavior_sections()` (identity, system_rules, directives, tone, tool_usage, etc.)
2. Register `agent_role` section for all SubAgents
3. Resolve and register each section named in `agent.prompt_sections`
4. Caller adds context: `.with_tools()`, `.with_environment()`, etc.
5. `build()` produces `PromptResult` with cache boundary

Section name resolution via `prompt_sections::resolve(name: &str) -> Option<PromptSection>` — a simple match-based registry.

**Cache benefit**: Stable zone (identity, system_rules, tone, tools, memory_protocol) is identical across main agent and all sub-agents, enabling Provider-side prompt cache reuse.

### Part 4: Verify Agent as Built-in + StopHook

**Built-in Agent registration**:
```rust
AgentDef::new("verify", AgentMode::SubAgent)
    .with_prompt_sections(vec!["verify_protocol".into()])
    .with_allowed_tools(vec!["*".into()])
    .with_max_iterations(25)
```

**StopHook trait abstraction**:

Refactor `StopHook` to a trait `StopHookHandler`:
```rust
#[async_trait]
pub trait StopHookHandler: Send + Sync {
    fn name(&self) -> &str;
    async fn evaluate(&self, ctx: &StopHookContext) -> StopHookVerdict;
}
```

Existing shell-command `StopHook` becomes `ShellStopHook` implementing the trait.
New `VerifyStopHook` also implements the trait.

`AgentLoop.stop_hooks` type changes from `Vec<Arc<StopHook>>` to `Vec<Arc<dyn StopHookHandler>>`.

**VerifyStopHook behavior**:

| Condition | Action |
|-----------|--------|
| `iterations < 3` | Skip (trivial task) |
| Agent not in trigger list | Skip (only triggers for main, coder) |
| Verify agent itself | Skip (prevent infinite recursion) |
| VERDICT: PASS | `StopHookVerdict::Allow` |
| VERDICT: FAIL | `StopHookVerdict::Block { reason }` — agent must fix issues |
| VERDICT: PARTIAL | `StopHookVerdict::Allow` — report attached for user reference |

### Part 5: Cleanup

**Delete**:
- `src/agents/prompts/main.md`
- `src/agents/prompts/explore.md`
- `src/agents/prompts/coder.md`
- `src/agents/prompts/researcher.md`

**Retain**:
- `src/agents/prompts/team_*.md` (out of scope)

**Not changed**:
- Swarm system (coordinator, bus, aggregator)
- SubAgent trait / Dispatcher / ExecutionCoordinator (execution pipeline unchanged)
- Gateway / SessionManager (routing layer unchanged)
- BuiltinToolRegistry (tool registration unchanged)

---

## File Change Summary

| Action | File | Est. Lines |
|--------|------|-----------|
| Modify | `src/agents/types.rs` | ~30 |
| Modify | `src/agents/registry.rs` | ~50 |
| Create | `src/agent_loop/prompt_sections/agent_role.rs` | ~40 |
| Create | `src/agent_loop/prompt_sections/explore_constraints.rs` | ~50 |
| Create | `src/agent_loop/prompt_sections/coder_guidelines.rs` | ~50 |
| Create | `src/agent_loop/prompt_sections/researcher_protocol.rs` | ~50 |
| Create | `src/agent_loop/prompt_sections/verify_protocol.rs` | ~80 |
| Modify | `src/agent_loop/prompt_sections/mod.rs` | ~15 |
| Modify | `src/agent_loop/prompt_builder.rs` | ~40 |
| Modify | `src/agent_loop/stop_hooks.rs` | ~60 |
| Create | `src/agent_loop/verify_stop_hook.rs` | ~120 |
| Modify | `src/agent_loop/loop_core.rs` | ~30 |
| Delete | `src/agents/prompts/{main,explore,coder,researcher}.md` | -30 |
| **Total** | | **~650 new/modified, ~30 deleted** |

---

## Implementation Order

1. **AgentDef refactor** — add `prompt_sections`, remove `system_prompt`, update `AgentDef::new()` signature
2. **Section renderers** — create 5 new section files with prompt content
3. **Section resolver** — add `resolve()` to `prompt_sections/mod.rs`
4. **PromptBuilder::for_agent()** — new constructor integrating agent sections
5. **Registry update** — rewrite `builtin_agents()` with new API, add verify agent
6. **Loop integration** — wire `for_agent()` into `loop_core.rs` SubAgent path
7. **StopHook refactor** — extract trait, rename to ShellStopHook, create VerifyStopHook
8. **Cleanup** — delete old prompt files, update all tests
9. **Verification** — cargo check, cargo test, cargo clippy
