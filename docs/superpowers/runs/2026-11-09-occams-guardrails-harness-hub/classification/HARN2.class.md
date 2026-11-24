# Occam's Razor Classification Report: HARN2

**Batch ID**: HARN2  
**Target Module**: `src/harness` (Agent Loop - Think & Prompt)  
**Assigned Files**:
- `src/harness/agent/think.rs` (1994 lines total, 1696 budgeted)
- `src/harness/agent/prompt.rs` (1526 lines total, 494 budgeted)
- `src/harness/tests/think.rs` (1105 lines)
- `src/harness/tests/prompt.rs` (454 lines)
- `src/harness/tests/reactive_compaction.rs` (1137 lines)

**Evaluation Date**: 2026-11-09  
**Review Doctrine**: Occam's Razor — remove unnecessary concepts, redundant allocations, dead code, and accidental complexity while strictly preserving contracts, invariants, and R10 redlines.

---

## Findings Matrix

| ID | Location | Taxonomy | Smell Description | Route Tag | Planned Action |
|---|---|---|---|---|---|
| HARN2-01 | `prompt.rs:119-126` | `REDUNDANT-ALLOCATION` / `ACCIDENTAL-COMPLEXITY` | `parse_tool_use_block` called twice per assistant block (once in `reconstruct_assistant_blocks`, then again in `build_prompt_with_transient_tail`), re-allocating JSON strings and input objects. | `FIX-NOW` | Iterate surviving `ContentBlock::ToolCall` in `blocks` to populate `open_tool_use_ids` and `expected_results`. |
| HARN2-02 | `prompt.rs:208-212`, `340-356` | `REDUNDANT-ALLOCATION` | In `SessionEvent::ToolError`, `orphan_result_note` was passed `&serde_json::Value::String(error.clone())`, which `orphan_result_note` cloned again. | `FIX-NOW` | Accept `rendered: &str` in `orphan_result_note`; pass `error.as_str()` directly for tool errors without `Value` allocation. |
| HARN2-03 | `think.rs:102, 407, 1059, 1514` | `REDUNDANT-ALLOCATION` | `response.text_content()` unconditionally clones `self.text` into an owned `String` in `is_empty_response`, `promote_text_tool_calls`, `run_turn_internal`, and `fire_boundary_grace_turn`. | `FIX-NOW` | Use `response.text.as_deref()` without allocation for empty checks, promote scans, and text formatting. |
| HARN2-04 | `think.rs:131-156` | `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY` | Shape #1 duplication: `last_fold_to_seq` and `last_fold_event` duplicate the exact same reverse event scan for `FoldRecorded` and `CompactionPerformed`. | `FIX-NOW` | Delegate `last_fold_to_seq` to `last_fold_event(events).map_or(0, |(_, to_seq)| to_seq)`. |
| HARN2-05 | `think.rs:375-378` | `CONTROL-FLOW` / `ACCIDENTAL-COMPLEXITY` | Redundant `match system_prompt` in `build_request_payload` when `with_system` already accepts `Option<&'a str>`. | `FIX-NOW` | Chain `.with_system(system_prompt)` directly in the fluent builder. |
| HARN2-06 | `think.rs:184-186` | `REDUNDANT-ALLOCATION` | `output.value.to_string()` in `estimate_prompt_tokens_after_seq` allocates a formatted JSON string even when `output.value` is already `Value::String`. | `FIX-NOW` | Match `Value::String(s) => text.push_str(s)` to avoid JSON string allocation for string tool results. |
| HARN2-07 | `think.rs:718-723, 888, 985, 1623-1627` | `CONTROL-FLOW` / `ACCIDENTAL-COMPLEXITY` | Verbose boilerplate for `tools_ref`, `matches!(StopReason)`, and `final_text` Option creation. | `FIX-NOW` | Simplify with `then`, `then_some`, and direct `== StopReason::MaxTokens`. |
| HARN2-08 | `tests/think.rs:249+` | `ACCIDENTAL-COMPLEXITY` | 9 unit tests duplicate the full 25-field `HarnessDeps` struct literal. | `FIX-NOW` | Extract a `default_deps(session, llm)` test fixture helper in `tests/think.rs`. |
| HARN2-09 | `think.rs:1203-1211, 1257-1265` | `ACCIDENTAL-COMPLEXITY` | Identical `pending_tool_use_ids` collection for `close_unexecuted_tool_uses` in Halt and Veto branches. | `FIX-CAREFULLY` | Verify and clean up closure pattern while preserving the exact `&[String]` signature for `act.rs` compatibility. |
| HARN2-10 | `think.rs:1341-1352` | `CONTROL-FLOW` | Duplicate cancellation check in `race_llm_call` branches. | `DEFER` | `tokio::select!` with biased branching and sleep_until timeout is delicate for timing/cancellation invariants; keep intact. |
| HARN2-11 | `prompt.rs:360-415` | `CONTROL-FLOW` | `reconstruct_assistant_blocks` signature and logic for thinking replay. | `DEFER` | Wire-level thinking replay semantics (`ResponsesEncrypted`, `ThinkingOrigin`) are delicate and tested; keep intact. |
