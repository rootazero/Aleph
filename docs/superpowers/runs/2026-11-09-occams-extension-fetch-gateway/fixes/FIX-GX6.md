# FIX-GX6 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- GX6-01: FIXED
- GX6-02: FIXED
- GX6-03: FIXED
- GX6-06: FIXED
- GX6-05: FIXED
- GX6-04: FIXED
- GX6-07: FIXED
- GX6-08: FIXED
- GX6-10: FIXED
- GX6-09: FIXED
- GX6-11: FIXED
- GX6-14: FIXED
- GX6-12: FIXED
- GX6-15: FIXED
- GX6-13: FIXED
- <ID>: SKIP(no location)
- GX6-16: FIXED
- GX6-17: FIXED


---

# 第二轮修复复核 / second fix wave (appended)

# GX6 second fix wave / 第二轮修复

以下为实现状态，不代表编译或测试已经通过。Implementation status only; compilation and tests were not run.

## GX6-01 — FIXED — Session rows retain the admission-derived stable principal; POST/GET/DELETE check ownership before touching, replacing the stream, or deleting. 保存 admission 的稳定 principal，并在三种 HTTP 操作变更 session 前校验；不保存 bearer，不改变既有准入闸。

## GX6-04 — FIXED — Responses flat function tools and named/string choices map directly to existing internal types; unsupported kinds/choices return explicit errors. 平面工具定义直接转换，不再回绕 Chat parser，也不静默丢弃无法表达的选项。

## GX6-05 — FIXED — Responses message/function_call/function_call_output input maps to UnifiedMessage using call_id; streamed and non-streamed function-call output share id/call_id construction. 工具结果以 call_id 关联，输入 item id 不会覆盖它；两种输出共用同一规则。

## GX6-08 — FIXED — Chat system messages become ordered provider context exactly once, not extra user messages; developer is explicitly rejected because the current unified contract cannot preserve its distinct semantics. 保留所有 system 指令的顺序，避免重复；不再悄悄混同 developer。

## GX6-09 — REVERTED — Removed the blanket nonempty-stop rejection and restored this hunk's 79dacfa33 behavior. RequestPayload has no per-request stop field and HttpProvider has no request override; static provider stop_sequences is not an equivalent API. 撤销兼容性回退；原先接受却不转发 stop 的缺口仍在，未以 Gateway 截断或新 provider API 冒充修复。

## GX6-15 — FIXED — Sanitization-empty TTS returns crate-internal Skipped; the emitter sends text without resetting or incrementing failures. 无可朗读内容不影响 voice 失败计数；真实失败和成功清零策略保留，新增 registry 消费端回归断言。

## GX6-16 — FIXED — Telegram code spans bypass other inline markup passes while retaining escaping; Slack fenced-code content and closing fences are restored. code 内容保持字面量，普通文本仍走现有 marker/link 逻辑，并修复误删的 Slack code-block 输出。

## GX6-17 — FIXED — Session/request-scoped cancellation tokens are registered before tool dispatch, used by notifications/cancelled and the existing execute_with_cancel entry, and removed by the request guard. 取消通知无响应；完成时清理登记，不通过 abort 跳过工具清理，未绕过鉴权或审批入口。

### Validation and limits / 验证与边界

- Added adjacent focused regression tests for ownership (including all three HTTP verbs and notifier retention), flat Responses tools/choices, tool-call correlation (including SSE), ordered Chat context/developer rejection, TTS skip accounting, literal Telegram code/Slack fences, and session-scoped cancellation lifecycle. 为上述修复新增或加强邻近回归测试；GX6-09 为撤销项，没有声称修复后的测试。
- `git diff --check` passed. 仅完成 whitespace 检查。
- Standalone `rustfmt --check` parsed all 12 touched Rust files without syntax diagnostics, but reported formatting differences in nine files; this is not a passing formatting check or a type check. 独立语法解析无诊断，但格式检查并非全绿。
- Per task instruction, no cargo command, compilation, Rust test execution, clippy, or full test suite was run. 遵守禁令，未运行上述验证，不声称 red/green。
- No live provider/Telegram/Slack integration or real long-running-tool/approval cancellation test was run; cancellation remains cooperative through the existing tool boundary. 未做真实外部服务与耗时工具/审批链路验证。
- The requested graphify queries were attempted, but this worktree lacks graphify-out/graph.json; scoped source inspection was used without generating graph files. 查询图缺失，未扩展为图重建任务。
- No unrelated shared-worktree changes were reverted, and no commit, stash, or checkout was performed. 未回退其他批次改动，未改写 git 历史。
