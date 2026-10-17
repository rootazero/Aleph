# TUI refactor delivery / TUI 重构交付记录

Approved design: [design](../specs/2026-10-14-tui-streaming-interaction-refactor-design.md). Implementation branch: `feat/tui-streaming-refactor`; isolated worktree, no main merge or push.

## Implemented / 已实施

- One shared `Transcript` owns live/replay rows; TUI renders Step, nested tools and `/detail brief|full`. Completed history is built offscreen and applied only to its originating session/generation. Active attach remains explicitly incomplete until authoritative terminal recovery; no guessed replay/live splice.
- Compact latest-user chip, at most 48 terminal cells and exactly one row, only after the original fully leaves above the viewport. Short text uses its actual width; overflow uses ASCII `...`. Literal `[click to jump]`, `/jump-user`, measured identity anchors, stale hit rejection and modal mouse isolation share the existing viewport/region infrastructure. Default background inherits terminal opacity; portable terminals provide no per-cell alpha.
- Typed public clarification contracts carry request/question/run identity without Core-only option values. Core compares expected identity and advances the cursor under one lock. Single modal controller preserves drafts across hide/reopen (`Ctrl+P` or `/requests`); acknowledgement and pending snapshots determine completion. Missing cancellation correlation is never guessed.
- Bounded RPC queue (8), bounded stream/topic batches (64), independent stream-loss counter, scoped pending/history/approval/replay/tool-output responses. After continuity loss, an authoritative run-status probe releases a lost terminal before rebuilding recorded history. Replayed requests have their own sequence; stale approvals cannot resurrect retired cards.
- `/tool-output <tool_call_id>` uses existing masked `trace.tool_output`, explicit `n` next page, UTF-8 byte offsets, 64 KiB pages and 1 MiB view ceiling. Expired storage is explicit and cannot trigger infinite paging.
- Core exit text is bounded to the current run's starting sequence; a failure/cancel before its first answer cannot reuse the previous run's reply.

中文：本轮重点是连通已有共享归并器、澄清注册表、回放和工具详情 RPC；TUI 保持纯 I/O。主要清理位置为 `app/trace.rs` 的重复转录写入、旧阻塞 history/side-question/approval 入口、主循环无限排空及旧的源码字符串接线断言。共享 `Reasoning` 与旧命令兼容入口仍有消费者，保留；未声称全仓不存在历史技术债。

## Validation / 验证

Each Cargo invocation was serialized through `py -B qa/terminal/memory_guard.py`, checking actual Windows available physical memory >= 4 GiB, with `CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0`. Tests/build paths were inspected before execution; no HOME/USERPROFILE override was used as isolation evidence. Core clarification filesystem fixtures explicitly use owned TempDir roots rather than side-effecting default configuration; theme fixture likewise uses an owned TempDir. No installed Gateway, real keyring or vault was exercised.

| Scope / 范围 | Executed result / 已执行结果 |
|---|---|
| Protocol lib | 407 passed |
| Shared UI lib, default / no-default | 223 passed each |
| Client connection tests | 5 passed, including saturated stream queue followed by RPC reply |
| CLI lib | 250 passed; `doctor_checks_run` excluded |
| Core clarification registry | 24 passed |
| Core clarification RPC handlers | 9 passed |
| Core ask-user tool | 13 passed |
| Core failure / cancellation reply provenance | 1 + 1 passed |
| Core lib compile | passed; existing warnings retained |
| TUI lib | 544 passed; 1 existing real-Gateway test ignored |
| Protocol/shared/client/TUI/CLI all-target Clippy with `-D warnings` | passed; final rerun after review fixes |
| Panel compile | passed (`panel-check.log`); no Panel runtime tests |

## Explicit limits / 明确边界

- CLI's unrelated `doctor_checks_run` launches external npm/node/rustup; its effects were not proven contained, so it was excluded. TUI's existing real-Gateway/browser test remains ignored. The exact unmodified `just test-shared` recipe was therefore not claimed as executed; equivalent package checks used the stated exclusions.
- No production Gateway acceptance, real-terminal mouse/opacity inspection, `qa/transcript_fold` real drain capture, performance benchmark, desktop packaging or blanket full-workspace tests/Clippy were run. No claim of measured Rust performance superiority or live/replay runtime byte parity.
- Some pre-existing slash-command RPCs still await in their handlers; priority resolve/pending/approval/history/replay/tool detail paths moved to bounded work. Panel Step migration is outside this TUI change; cross-interface compile validates additive wire compatibility only.
- Settled Step bodies use reducer change invalidation; legacy plain text rows still hash their full content for correctness on same-size edits. No benchmark was run for long legacy histories.
- `Transcript` retains a compatibility mutable Vec view for existing presentation toggles and fixtures; reducer ownership is singular, but closing that API further is future tightening, not claimed complete here.
- Repository commit hook runs broad Core formatting/Clippy, and its baseline formatting check failed on unrelated existing files. Commits disable that hook per invocation after explicit scoped formatting, tests and Clippy; no unrelated formatting was staged.

Final review / 最终审查：独立只读审查指出迟到审批快照和回放身份误失效，两项均修正并对审批复活增加行为回归；先前转录审查指出永久缺失 trace 的无限恢复重建等问题，亦已修正。设计中的真实 Gateway/终端验收仍未完成。
