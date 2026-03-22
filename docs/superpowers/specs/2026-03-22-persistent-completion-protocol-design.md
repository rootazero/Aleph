# Persistent Completion Protocol

**Date**: 2026-03-22
**Status**: Approved
**Scope**: `core/src/agent_loop/loop_core.rs`, `core/src/agent_loop/prompt_builder.rs`

## Problem

Aleph's agent loop stops when the LLM returns `EndTurn` with no tool calls. The only persistence mechanism is a one-shot nudge that fires when the LLM gives up after tool errors. This is insufficient — the LLM can declare itself "done" without verifying that all requirements are met, leading to incomplete task execution.

The previous POE module attempted to solve this with a code-based evaluation pipeline, but violated R8 (LLM Sovereignty) by replacing LLM judgment with deterministic code. It was removed.

## Design

### Inspiration

oh-my-openagent's Sisyphus module uses a "Ralph Loop" pattern: the LLM must output an explicit completion signal (`<promise>COMPLETION_PROMISE</promise>`), otherwise the system auto-injects "continue". All quality judgment lives in the prompt; the code only checks for the presence of a tag.

### Approach: Loop-Layer Completion Tag

Extend the existing EndTurn stop logic in `AgentLoop` with a completion tag requirement for complex tasks (tasks that used tools).

**Activation condition**: `tool_calls_made > 0` — a mechanical fact, not a semantic judgment. Pure Q&A (no tool calls) stops naturally without any completion protocol.

**Completion tag**: `<task-complete/>` — LLM must output this in its final response to confirm the task is done.

**Completion check block**: Before the tag, the LLM outputs a structured self-verification:

```
<completion-check>
- Request: [one-line summary of what user asked]
- Done: [what was accomplished]
- Verified: [how correctness was confirmed]
- Risks: [none / specific concerns]
</completion-check>
<task-complete/>
```

### Nudge Stages

When the LLM stops (EndTurn + no tool calls) without `<task-complete/>` and `tool_calls_made > 0`:

| Nudge # | Stage | Message |
|---------|-------|---------|
| 1-2 | Challenge | "You stopped but have not confirmed task completion. Do NOT apologize or explain. Review your work against the original request: is every requirement met? If not, try a different approach. When fully done, output a `<completion-check>` block and `<task-complete/>`." |
| 3 | Graceful exit | "Final attempt. Summarize: (1) what approaches you tried, (2) what succeeded and what failed, (3) what the user should do next. Then output `<task-complete/>`." |

After 3 nudges without `<task-complete/>`, the loop stops unconditionally.

### Merge with Existing Persistence Nudge

The current `nudge_sent: bool` (fires once when LLM gives up after `consecutive_errors > 0`) is replaced by the unified `completion_nudge_count: usize`. The old nudge is a subset of the new protocol — "LLM stops after errors without completing" is just one case of "LLM stops without completion tag".

The `consecutive_errors` counter and `MAX_CONSECUTIVE_ERRORS = 10` remain unchanged — they are a safety circuit breaker for continuous tool failures, orthogonal to the completion protocol.

## Changes

### loop_core.rs (~20 lines)

Replace:
```rust
let mut nudge_sent = false;
```

With:
```rust
let mut completion_nudge_count: usize = 0;
const MAX_COMPLETION_NUDGES: usize = 3;
```

Replace the EndTurn branch (lines 259-280) with:

```rust
if !response.has_tool_calls() && response.stop_reason == StopReason::EndTurn {
    // Simple Q&A (no tools used) -> stop naturally
    if tool_calls_made == 0 {
        break;
    }

    // Complex task: check for completion tag
    let has_completion_tag = final_text
        .as_ref()
        .map_or(false, |t| t.contains("<task-complete/>"));

    if has_completion_tag {
        break;
    }

    // No completion tag — nudge based on stage
    if completion_nudge_count < MAX_COMPLETION_NUDGES {
        completion_nudge_count += 1;

        let nudge_msg = if completion_nudge_count < MAX_COMPLETION_NUDGES {
            "[SYSTEM] You stopped but have not confirmed task completion. \
             Do NOT apologize or explain. Review your work against the original request: \
             is every requirement met? If not, try a different approach. \
             When fully done, output a <completion-check> block and <task-complete/>."
        } else {
            "[SYSTEM] Final attempt. Summarize: (1) what approaches you tried, \
             (2) what succeeded and what failed, (3) what the user should do next. \
             Then output <task-complete/>."
        };

        messages.push(UnifiedMessage::user(nudge_msg));
        continue;
    }

    break;
}
```

### prompt_builder.rs (~12 lines)

Append to `BASE_BEHAVIOR`:

```
- **TASK COMPLETION PROTOCOL.** When your work involved tool calls, you MUST verify completion before stopping:\n\
  1. Review the user's original request — is EVERY requirement addressed?\n\
  2. Verify your results — did the tools succeed? Are the outputs correct?\n\
  3. Output a completion check block:\n\
     <completion-check>\n\
     - Request: [one-line summary of what user asked]\n\
     - Done: [what you accomplished]\n\
     - Verified: [how you confirmed correctness]\n\
     - Risks: [none / specific concerns]\n\
     </completion-check>\n\
  4. Output <task-complete/> to confirm you are done.\n\
  If you did NOT use any tools (pure conversation), just respond naturally — no completion protocol needed.
```

### Tests

- Update existing `test_persistence_nudge_on_premature_stop` and `test_persistence_nudge_fires_only_once` to use `completion_nudge_count`
- New test: LLM stops with `<task-complete/>` in response → loop ends immediately
- New test: LLM stops without tag after tool use → nudge injected up to 3 times
- New test: LLM stops without tag and no tool use → loop ends naturally (no nudge)
- New test: `<task-complete/>` in intermediate response (with tool calls) → ignored

## Boundary Conditions

1. **consecutive_errors unchanged** — safety circuit breaker, orthogonal to completion protocol
2. **LoopRunResult unchanged** — no new fields; `hit_limit` already expresses abnormal termination
3. **Tag in intermediate responses** — ignored; only checked in EndTurn + no tool calls branch
4. **Tag visible to user** — not stripped; `<completion-check>` is useful for user review. Can add stripping later (YAGNI)
5. **Subagents** — protocol applies automatically since all agents share `AgentLoop`

## R8 Compliance

The code performs zero semantic judgment. It checks:
- `tool_calls_made > 0` — a counter increment, mechanical fact
- `text.contains("<task-complete/>")` — string matching, not reasoning

All quality evaluation (is the task truly done? are requirements met?) lives entirely in the system prompt, executed by the LLM.

## Estimated Impact

~30-40 lines of production code + test updates. No new files, no new dependencies, no API changes.
