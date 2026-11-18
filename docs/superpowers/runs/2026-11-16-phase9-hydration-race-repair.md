# Phase9 same-session hydration repair / 同会话历史恢复修复

## Independent finding / 独立发现

The actual [first QUALITY review](2026-11-16-phase9-quality-first-review.md), reviewer `0eab2546-4aff-40f3-b1ee-97599a24f6fd`, is **BLOCK / NOT COMPLETE**. Epoch/session identity alone cannot distinguish a newer send or streamed transcript on the same session while reconnect hydration awaits persisted traces. Runtime15 did not exercise that interleaving and is not reused as acceptance of the repair. / 同 epoch、同会话并不表示客户端投影未前进；保留原 BLOCK，不改写旧证据。

## Reproduction / 复现

[The deliberate RED receipt](2026-11-16-phase9-hydration-progress-red.json) records exit 101 and **1 passed / 2 failed / 1,562 filtered**. Both failures use owned oneshot barriers and the production `await_current_history` boundary, not a timer or an alternate reducer:

- Release history, hold traces, append a real user message and assistant stream via `ChatState`; release old traces. The epoch/session-only checkpoint allowed replacement and lost both new rows.
- Hold the later response while a live assistant bubble receives another `append_chunk`, without incrementing `sends`; releasing the old snapshot removed the latest chunk.
- The unchanged-snapshot positive control succeeds, so rejecting every snapshot cannot satisfy the regression.

/ 使用确定性的 future 屏障延迟旧响应；断言实际消息内容，不以“调用发生过”代替保留效果。

## Repair and trade-offs / 修复与代价

Only `interfaces/webchat/src/components/chat_sidebar.rs` changes production behavior:

- Capture the existing epoch, session key, send count, rendered messages, active run, phase, reasoning and requested history window before waiting. After waiting, reject a result if any of those existing values changed or were disposed. This is a temporary checkpoint, not a second source of identity or business state.
- Gather history and traces before the destructive transcript commit. History metadata, messages, plan and last-run notice then commit without another await on the Panel event loop.
- Do **not** delay transcript rendering for the optional context estimate. Capture a fresh checkpoint after the synchronous replay; apply its eventual gauge result and return old run-follow metadata only if the current state still matches. Otherwise preserve the newer live turn.
- A raised history window invalidates the older smaller-window request, without invalidating an otherwise unchanged newer request.

The cost is a temporary clone/comparison of the visible transcript at each asynchronous checkpoint. There is no new mutable generation counter, global executor, Gateway transport or canonical state map. Discarding stale results favors retaining the newer live projection over forcing an obsolete persisted snapshot into it. / 临时快照有复制和比较开销；取舍是丢弃过期恢复结果，而不是覆盖新消息，不虚称性能收益。

The first implementation passed 1,567 Panel cases but awaited the optional gauge before rendering. Its subsequent default build was intentionally cancelled and its logs preserved while refining that latency boundary. The final v2 implementation passed **1,567 Panel tests / 0 failed / 0 ignored** in 1.08s: [terminal native suite](2026-11-16-phase9-panel-hydration-v2-green.json). Separate canonical [default](2026-11-16-phase9-panel-hydration-v2-default.json) and [QA](2026-11-16-phase9-panel-hydration-v2-qa.json) builds both pass. [Actual runtime16](2026-11-16-phase9-runtime-attempt16.json) passes for both native windows, 79 raw-correlated events per surface and exit0/unforced, with real HTTP QA payload binding and unchanged served assets across the run. Its ordinary schedule supplements rather than replaces the deterministic race regression. All nine saved files of the **newly fixed** default bundle were then [restored byte-exact](2026-11-16-phase9-hydration-v2-default-restoration.json), without the QA JS ingress. / 最终 v2 已完成测试、分立构建、真实双窗口和新默认产物恢复；旧运行记录不冒充新修复证据。

## Open gates / 开放闸门

Implementation, affected builds, fresh runtime validation and the ordered independent SPEC→QUALITY re-review are complete. [Actual SPEC](2026-11-16-phase9-spec-hydration-review.md) and [actual QUALITY](2026-11-16-phase9-quality-hydration-review.md) both independently close the original code P1, while both overall verdicts remain **BLOCK / NOT COMPLETE solely for D**. / 两份真实独立审查已按顺序完成，确认代码缺陷闭合，不改写整体验收阻塞。 D's complete WebKit ownership and valid 10 warmup + 30 measured performance acceptance remain **OPEN / BLOCKED**, independently of this code defect. No release, cross-platform, merge, push or overall Phase9 PASS claim is made. / 不扩大授权，不绕过 OS policy，不降低性能闸门。
