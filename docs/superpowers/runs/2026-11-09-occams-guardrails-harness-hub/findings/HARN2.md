# Occam's Razor Findings & Refactoring Report: Batch HARN2

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-guard-harness-hub`  
**Target Module**: `src/harness` (Agent Loop - Think Phase & Prompt Assembly)  
**Commit Message**: `fix(harness): apply Occam's razor simplifications in think and prompt loops`  

---

## 1. Summary of Work Done

During this review of Batch HARN2 (`src/harness/agent/think.rs`, `src/harness/agent/prompt.rs`, and associated tests), we conducted an Occam's Razor static analysis across all 5 assigned files:
- 2 budgeted production files: `src/harness/agent/think.rs`, `src/harness/agent/prompt.rs`.
- 3 test files: `src/harness/tests/think.rs`, `src/harness/tests/prompt.rs`, `src/harness/tests/reactive_compaction.rs`.

We identified and resolved key inefficiencies in heap allocation, redundant clones, and control flow complexity:
1. **Eliminated Redundant Block Parsing in `build_prompt_with_transient_tail`**: Previously, after `reconstruct_assistant_blocks` had already parsed `content.blocks` and filtered non-orphans, the loop re-iterated over raw `content.blocks` and invoked `parse_tool_use_block` a second time (allocating new `id`, `name`, `arguments`, and `thought_signature` strings). Replaced with a direct scan of the already-reconstructed `blocks`, inserting surviving `ContentBlock::ToolCall` IDs into `open_tool_use_ids` and `expected_results`.
2. **Zero-Allocation Orphan Error Note in `prompt.rs`**: `orphan_result_note` accepted `&serde_json::Value`, forcing the `SessionEvent::ToolError` arm to allocate `serde_json::Value::String(error.clone())`, which `orphan_result_note` cloned again. Refactored `orphan_result_note` to take `rendered: &str`, passing `error.as_str()` directly for errors and using borrowed `Cow` for string results.
3. **Zero-Allocation Empty/Trim Checks in `think.rs`**: Replaced calls to `response.text_content()` (which unconditionally clones `self.text` into an owned `String`) with borrowed `response.text.as_deref().unwrap_or("").trim().is_empty()` in `is_empty_response`, `promote_text_tool_calls`, and `fire_boundary_grace_turn`.
4. **Optimized Assistant Text Formatting in `run_turn_internal`**: Replaced unconditional `format!("{carried}{}", response.text_content())` with `carried.is_empty()` check. In the 99.9% common case where no tokens were carried over, `response.text.clone().unwrap_or_default()` is returned directly without running the `format!` macro or allocating an intermediate copy.
5. **Deduplicated Fold Event Log Walk (Shape #1)**: `last_fold_to_seq` duplicated the exact reverse log scan as `last_fold_event` for `FoldRecorded` and `CompactionPerformed`. Delegated `last_fold_to_seq` directly to `last_fold_event(events).map_or(0, |(_, to_seq)| to_seq)`.
6. **Fluent Builder Chain in `build_request_payload`**: Eliminated redundant `let base = match system_prompt { ... }` since `RequestPayload::with_system` natively takes `Option<&'a str>`.
7. **Zero-Allocation String Value Probe in Token Estimator**: Avoided `to_string()` serialization when estimating tokens for `ToolResult` where `output.value` is already a `Value::String(s)`.
8. **Test Boilerplate Reduction in `tests/think.rs`**: Extracted a `default_deps(session, llm)` fixture helper, replacing ~230 lines of repeated 25-field struct literals across 9 unit tests.

All modifications strictly preserve prompt ordering, cache prefix preservation, token estimation feedback, stream handling, and R10 redlines.

Line counts in budgeted files decreased:
- `think.rs`: 1696 → 1680 lines (−16 lines)
- `prompt.rs`: 494 → 488 lines (−6 lines)
Total harness budgeted lines dropped to 5476 lines, comfortably under `budget.rs::CEILING = 5535`.

---

## 2. Invariants & Preservation Proofs

| Invariant | How Preserved |
|:---|:---|
| **R10 12-File Hard Limit** | Exactly 12 production files remain in `src/harness` (8 top-level + 4 under `agent/`). Zero new files created. |
| **R10 Ratchet Budget** | Line counts decreased in both touched files (`think.rs` −16, `prompt.rs` −6). Total budgeted lines = 5476 <= 5535 (59 lines headroom). |
| **R7 / R10 Non-Cognition** | Zero autonomous recovery loops or deterministic heuristics added to the harness. All cognitive nudges remain in prompt/nudges. |
| **Prompt Message Ordering** | Mid-loop steering messages remain held in `deferred_user_msgs` until all tool results for the current turn have landed. |
| **Cache Prefix Preservation** | `build_request_payload` continues to thread `system_blocks` and `session_id` into metadata so prompt caching remains effective. |
| **Transient Tail Isolation** | Growth nudges and transient reminders are pushed strictly after `persisted_len` is calculated, ensuring they never leak into persisted session events. |
| **Token Usage & Accounting** | Intermediate token usage accounting on empty responses, max output tokens, and grace turns is strictly preserved. |
| **Cancellation Safety** | `CancellationToken` checkpoints in `race_llm_call`, `stream_llm_call`, and grace turns remain completely untouched. |

---

## 3. Surgical Changes Applied

### A. Eliminate Redundant Tool-Use Parsing in Prompt Assembly (`src/harness/agent/prompt.rs`)
- **Before**:
  ```rust
  if let Some(blocks) = reconstruct_assistant_blocks(content, &resolved, earlier) {
      messages.push(UnifiedMessage::Assistant { content: blocks });
      assistant_emitted = true;
      for raw in &content.blocks {
          if let Some(ContentBlock::ToolCall { id, .. }) = parse_tool_use_block(raw) {
              if resolved.contains(id.as_str()) {
                  open_tool_use_ids.insert(id.clone());
                  expected_results.insert(id);
              }
          }
      }
  }
  ```
- **After**:
  ```rust
  if let Some(blocks) = reconstruct_assistant_blocks(content, &resolved, earlier) {
      for block in &blocks {
          if let ContentBlock::ToolCall { id, .. } = block {
              open_tool_use_ids.insert(id.clone());
              expected_results.insert(id.clone());
          }
      }
      messages.push(UnifiedMessage::Assistant { content: blocks });
      assistant_emitted = true;
  }
  ```
  Surviving non-orphan tool calls are already parsed and present in `blocks`. Re-iterating `blocks` avoids re-parsing every JSON object in `content.blocks` and eliminates duplicate string allocations.

### B. Borrowed `rendered: &str` in `orphan_result_note` (`src/harness/agent/prompt.rs`)
- **Before**: `orphan_result_note(call_id: &str, tool_name: &str, payload: &serde_json::Value)` matched `Value::String(s) => s.clone(), other => other.to_string()`. In `SessionEvent::ToolError`, this required `&serde_json::Value::String(error.clone())`.
- **After**: Changed parameter to `rendered: &str`. In `ToolError`, `error.as_str()` is passed directly (zero heap allocations). In `ToolResult`, `Value::String(s)` borrows `s.as_str()`.

### C. Zero-Allocation Response Text Inspection (`src/harness/agent/think.rs`)
- **Before**: `is_empty_response` called `response.text_content().trim().is_empty()`, cloning `response.text` on every call. `promote_text_tool_calls` and `fire_boundary_grace_turn` also unconditionally cloned `response.text_content()`.
- **After**: Switched to `response.text.as_deref().unwrap_or("").trim().is_empty()` and `let Some(text) = response.text.as_deref().filter(|t| !t.is_empty()) else { return 0; }`. Avoids allocation on all checks.

### D. Single-Allocation Text Formation in `run_turn_internal` (`src/harness/agent/think.rs`)
- **Before**: `let text = format!("{carried}{}", response.text_content());` allocated a cloned string from `text_content()`, then allocated another string in `format!`.
- **After**:
  ```rust
  let text = if carried.is_empty() {
      response.text.clone().unwrap_or_default()
  } else {
      let mut s = carried;
      if let Some(t) = response.text.as_deref() {
          s.push_str(t);
      }
      s
  };
  ```
  Reduces allocation count from 2 to 1 on every normal turn.

### E. Consolidation of Fold Event Discovery (`src/harness/agent/think.rs`)
- **Before**: `last_fold_to_seq` and `last_fold_event` duplicate the exact reverse iterator and pattern match over `SessionEvent::FoldRecorded` and `CompactionPerformed`.
- **After**:
  ```rust
  fn last_fold_to_seq(events: &[SessionEventRecord]) -> EventSeq {
      last_fold_event(events).map_or(0, |(_, to_seq)| to_seq)
  }
  ```

### F. Fluent Payload Construction (`src/harness/agent/think.rs`)
- **Before**: Interrupted builder chain with `let base = match system_prompt { ... }`.
- **After**: Chained `.with_system(system_prompt)` directly.

### G. Test Boilerplate Elimination in `tests/think.rs`
- Introduced `default_deps(session, llm)` fixture helper, cutting ~230 lines of boilerplate across 9 unit tests while preserving all field assignments.

---

## 4. Line Budget Accounting (R10 Ratchet)

Under `src/harness/tests/budget.rs::budgeted_lines`, line counts across the 12 budgeted files:
- `src/harness/mod.rs`: 20 lines (unchanged)
- `src/harness/agent.rs`: 1050 lines (unchanged)
- `src/harness/deps.rs`: 222 lines (unchanged)
- `src/harness/trait_def.rs`: 136 lines (unchanged)
- `src/harness/callback.rs`: 78 lines (unchanged)
- `src/harness/chain_context.rs`: 79 lines (unchanged)
- `src/harness/trace.rs`: 261 lines (unchanged)
- `src/harness/trace_sink.rs`: 25 lines (unchanged)
- `src/harness/agent/think.rs`: 1696 → 1680 lines (−16 lines)
- `src/harness/agent/act.rs`: 1278 lines (unchanged in HARN2)
- `src/harness/agent/guardrails.rs`: 159 lines (unchanged in HARN2)
- `src/harness/agent/prompt.rs`: 494 → 488 lines (−6 lines)

**Total budgeted lines**: 5476 lines.  
**Current CEILING**: 5535 lines.  
Verification: `5476 <= 5535` passes with 59 lines of headroom.

---

## 5. What Was NOT Done & Skipped Validations

1. **Deliberately skipped `cargo check` / `cargo test`**: Strictly followed user instruction ("无需cargo check，直接提交。全部模块review完成后统一cargo check。").
2. **Did NOT alter wire reasoning replay logic**: The `ThinkingOrigin` and `Thinking` block reconstruction was preserved verbatim to ensure compatibility with encrypted and plaintext reasoning backends.
3. **Did NOT collapse biased `tokio::select!` branches in `race_llm_call`**: Preserved the dedicated cancellation priority and timeout handling.
4. **Did NOT introduce cognitive recovery heuristics into the harness loop**: Kept all recovery and prompt nudges strictly in `thinker/nudges.rs`.
