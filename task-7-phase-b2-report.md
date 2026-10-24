# Task7 AI migration Phase B2 / 源码迁移交接

## Status and boundary / 状态与边界

- Root: `.`; branch: `refactor/modularization-p0`.
- B2 source edits and requested static checks are complete; compilation, behavioral tests and final integration acceptance remain Main-owned. / B2 源码改写与指定静态检查已完成，不代表编译或行为验收通过。
- Read `task-7-source-review.md` §10 and `task-7-match-boundary.md` under `.superpowers/sdd/2026-10-14-modularization-p0-implementation/`, then actual API helper/accessor definitions and Core caller bodies.
- Scope: only the four requested source files plus this report. Existing unrelated staged/unstaged/untracked work was preserved. No subagents.
- Count clarification / 数量澄清: the actual specified source contains **six production exhaustive matches plus one test exhaustive match**, not seven production plus one test. Including the explicitly excluded, already-legal advisory terminal match gives seven production decision sites plus one test site. B1's 11 exhaustive sites + B2's 7 exhaustive sites = the source-review census of 18. No additional site was invented or legal match rewritten to satisfy a count.

## Exact site mapping / 逐点映射

Old locations below refer to the pre-B2 source; new locations refer to this working tree.

| Site / 站点 | Before → after / 改写 | Source-equivalence review / 源码等价核对 |
|---|---|---|
| `src/providers/moa/advisory_view.rs:210` → `src/providers/moa/advisory_view.rs:210`, `build_advisory_view` | Exhaustive UnifiedMessage match → existing `dispatch_unified_message`; local mutable context is `(&mut rendered, &mut parts)`. | User callback keeps `text_of`, whitespace-only skip and untrimmed accepted user text. Closure-local `return` replaces loop `continue`; there is no work after dispatch in the loop, so the same next-message transition occurs. Assistant still clears/reuses parts, trims nonblank prose, appends tool-call renderings in order and emits only nonempty parts. ToolResult retains 4000-character budget, exact error/non-error tags and append-to-last-assistant behavior. |
| `src/providers/moa/advisory_view.rs:268` → `src/providers/moa/advisory_view.rs:267`, `content_mut` | Exhaustive mutable match → `msg.content_blocks_mut()`. | Preserves exact `fn content_mut(msg: &mut UnifiedMessage) -> &mut Vec<ContentBlock>` signature and original mutable vector borrow for all roles. Both existing callers, `apply_view_budget` and `mark_cache_breakpoints`, remain unchanged and transitively use the API owner accessor. No mutable dispatcher, extra vector or fallback. |
| `src/providers/moa/advisory_view.rs:361` → `src/providers/moa/advisory_view.rs:357`, `view_signature` | Exhaustive UnifiedMessage match → existing dispatcher returning borrowed `(role, content)` with unit context. | Roles remain `user` / `assistant` / `tool`; content iteration remains in original order. Role hashed first, then nonempty Text, serialized Json, or `image` followed by MIME type. Existing Thinking/ToolCall ignore policy and cache-control independence remain unchanged. Returning a slice instead of borrowing the Vec does not change the iterated blocks or hash calls. |
| `src/context/compact/compaction_window.rs:128` → `src/context/compact/compaction_window.rs:128`, `serialize_transcript` | Exhaustive UnifiedMessage match → existing dispatcher with `&mut lines`. | Still computes `transcript_text` then `cap_transcript_text` once per message, pushes exactly one line in original order and joins with newline. Exact labels remain `user: `, `assistant: `, `tool_result({tool_name}): `. No tool-result name/error label changes. |
| `src/context/compact/compactor.rs:1317` → `src/context/compact/compactor.rs:1319`, `deterministic_truncation` | Exhaustive UnifiedMessage match → existing dispatcher with `&mut lines`. | `head` remains `cap_transcript_text(text.lines().next().unwrap_or(""))`. Transcript rendering and head/cap are now common before dispatch, still once per message. Same one line, labels, first-line behavior, empty body behavior, caps and newline joining for every known role. |
| `src/context/budget/pressure.rs:368` → `src/context/budget/pressure.rs:375`, `estimate_message_tokens_split` | Exhaustive ContentBlock match → existing `dispatch_content_block`; six accumulators moved to implementation-local `TokenContext`. | Callback order follows actual API signature: Text, Json, Thinking, ToolCall, Image. Only selected callback runs. Strings/bool/usize defaults equal original initializers. Accumulators are destructured back to original locals after traversal; all downstream density, marginal reasoning, saturation, totals and ToolResult bucketing arithmetic stays unchanged. |
| `src/providers/moa/advisory_view.rs:555` → `src/providers/moa/advisory_view.rs:553`, `cache_breakpoints_mark_last_three_messages` test closure | Exhaustive content-access match → `m.content_blocks()`. | Original immutable Vec borrow is replaced with existing owner accessor's slice. Same iteration, `any(matches!(... cache_control: Some(_)))`, `Vec<bool>` result and `[false, false, true, true, true]` assertion. Only accompanying rustfmt layout changes. |
| **Unchanged / 保留:** `src/providers/moa/advisory_view.rs:251` → `src/providers/moa/advisory_view.rs:250`, advisory terminal match | Existing `Some(UnifiedMessage::Assistant { .. }) \| None` and `Some(_)` retained verbatim. | Empty view or trailing assistant still appends `ADVISORY_INSTRUCTION`; other terminal messages remain untouched. Already legal across the non_exhaustive boundary; not a migration site. |

## Five block token costs / 五类 block 计费

- **Text:** `push_part` into both prose and text_only, preserving block ordering/separators. / 同步累积 prose 与 text_only。
- **Json:** unchanged `value_as_model_text(value)`; string values add `estimate_tokens_aware(&text, prose_ratio)` individually to result_text_tokens; non-string values append to structured. / 字符串按实际模型文本与 prose anchor 计费，其他 JSON 保持 structured 密度。
- **Thinking:** append only to prose and set has_thinking=true; the existing common-density marginal-cost calculation remains byte-for-byte unchanged. / 推理边际成本算法未变。
- **ToolCall:** append exact `format!("{name} {arguments}")` to structured, subsequently estimated at CODE_RATIO. / 名称、空格、参数顺序与密度未变。
- **Image:** increment image_count once per block; original multiplication by IMAGE_TOKENS_ESTIMATE unchanged. / 每块固定计费不变。

## Verification evidence / 实际验证证据

1. Direct scoped formatting check / 直接范围格式检查:

   ```sh
   rustfmt --edition 2021 --check src/providers/moa/advisory_view.rs src/context/compact/compaction_window.rs src/context/compact/compactor.rs src/context/budget/pressure.rs
   ```

   Exit 0, no output on the final run. First check identified only the shortened test closure's layout; corrected that block with a targeted edit and reran successfully. / 最终通过；首次仅发现该测试闭包格式需调整，已修正。

2. `git diff --check`: exit 0, no output. This also passed on the pre-edit working-tree baseline. / 工作树差异空白检查通过。
3. Reviewed the four-file diff against the original bodies and actual owner argument order/lifetimes. No new catch-all, panic, unknown-variant policy or dropped message/block path introduced. Existing legal wildcard policies were left intact. / 完成逐臂源码核对，不是运行时等价测试。
4. SHA-256 before/after equality confirmed for all five B1 files and both inspected API source files / 下列文件前后 SHA-256 一致:
   - `src/providers/protocols/openai_chat/proto_impl.rs`
   - `src/providers/protocols/anthropic/proto_impl.rs`
   - `src/providers/protocols/gemini/proto_impl.rs`
   - `src/providers/responses/shared.rs`
   - `src/providers/ollama.rs`
   - `crates/aleph-slot-api/src/ai.rs`
   - `crates/aleph-slot-api/src/ai_message.rs`

## Tradeoff and remaining uncertainty / 取舍与剩余不确定性

- Reused owner decomposition keeps enum exhaustiveness in the owner crate and business policy in Core. It introduces positional callback verbosity; future variant expansion must explicitly maintain owner helpers and consumers. No new API helper or wire DTO was needed. / 保持单一 owner，代价是回调位置参数更冗长。
- Static source equivalence and rustfmt parsing do **not** prove type checking, closure borrow inference, runtime behavior, future-variant behavior, performance or external consumer compatibility. / 静态核对不替代类型检查、借用检查与行为测试。
- No genuine API/signature blocker was found. Compilation may still expose remaining integration errors; no Cargo result or full-tree exhaustiveness acceptance is claimed. / 未发现必须扩 API 的阻塞，但仍需 Main 编译验收。
- Main should run the planned API/external borrowed-payload/sink/drop checks plus Core advisory, transcript/truncation and pressure tests after B2, under the existing per-Cargo memory probe/build-jobs guard. Phase A 261/0 and its memory probe are supplied handoff history, not rerun evidence from this session. / 原有测试与资源保护仍由后续集成执行。

## Not done / 明确未做

No Cargo, rustc, tests, clippy, full-workspace formatting, staging, commit, reset, push, install, graph work, subagents, API edits, dependency/wire changes or unrelated source changes. No performance/all-platform/runtime QA. No final Task7 integration/acceptance claim; that remains Main-owned. / 未执行编译和测试，未触碰 B1/API 或其他任务；最终验收仍归主 Agent。
