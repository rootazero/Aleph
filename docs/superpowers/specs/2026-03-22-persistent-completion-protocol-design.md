# Persistent Completion Protocol

**Date**: 2026-03-22
**Status**: Approved
**Scope**: `src/agent_loop/loop_core.rs`, `src/agent_loop/prompt_builder.rs`

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

**Behavior change**: The old nudge required `consecutive_errors > 0` to fire and reset `consecutive_errors = 0` when triggered. The new protocol fires unconditionally when `tool_calls_made > 0` and the tag is absent — the `consecutive_errors > 0` precondition is removed. The `consecutive_errors` counter is no longer reset by the completion nudge; it continues to accumulate independently and triggers the circuit breaker at 10 regardless of nudge state.

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

    // Complex task: check for completion tag in CURRENT response
    // (must use response.text, not final_text, to avoid stale values
    // when LLM returns EndTurn with no text after a nudge)
    let has_completion_tag = response
        .text
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

**Existing tests to update** — all tests that exercise tool calls + EndTurn must add `<task-complete/>` to their final mock response text to avoid triggering the new nudge path:
- `test_tool_call_then_response` — add tag to `"Done echoing."` response
- `test_multi_turn_tool_chain` — add tag to final response
- `test_safety_guard_blocks_tool` — add tag to final response
- `test_persistence_nudge_on_premature_stop` — rewrite to test new completion protocol
- `test_persistence_nudge_fires_only_once` — rewrite to test 3-nudge stages
- Any other test where `tool_calls_made > 0` and final response lacks the tag

**New tests**:
- LLM stops with `<task-complete/>` in response → loop ends immediately
- LLM stops without tag after tool use → nudge injected up to 3 times, stage escalation
- LLM stops without tag and no tool use → loop ends naturally (no nudge)
- `<task-complete/>` in intermediate response (with tool calls) → ignored
- LLM returns EndTurn with no text after nudge → `response.text` is None, no false positive from stale `final_text`

## Boundary Conditions

1. **consecutive_errors unchanged** — safety circuit breaker, orthogonal to completion protocol
2. **LoopRunResult unchanged** — no new fields; `hit_limit` already expresses abnormal termination
3. **Tag in intermediate responses** — ignored; only checked in EndTurn + no tool calls branch
4. **Tag visible to user** — not stripped; `<completion-check>` is useful for user review. Can add stripping later (YAGNI)
5. **Subagents** — protocol applies automatically since all agents share `AgentLoop`
6. **Provider compatibility** — nudge messages are injected as `user` role (matching existing pattern). Up to 3 sequential `[SYSTEM]` user messages may appear in history. This follows the existing architecture choice and works with all supported providers

## R8 Compliance

The code performs zero semantic judgment. It checks:
- `tool_calls_made > 0` — a counter increment, mechanical fact
- `text.contains("<task-complete/>")` — string matching, not reasoning

All quality evaluation (is the task truly done? are requirements met?) lives entirely in the system prompt, executed by the LLM.

## Estimated Impact

~30-40 lines of production code + test updates. No new files, no new dependencies, no API changes.
