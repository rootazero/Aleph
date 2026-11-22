# FIX-G2 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- G2-02: FIXED
- <ID>: SKIP(no location)
- G2-03: FIXED
## G2-01 — FIXED — 父行读取失败时计错并跳过 child 注册及父侧会话退休；保留缺失/legacy 归属行为。扩展现有测试，以真实 SQLite 元数据解码失败验证零注册、路由不变、侧会话保留，以及恢复后继承归属并退休侧会话。 / Count parent metadata read errors and defer registration/retirement; preserve missing/legacy attribution behavior. Extend the existing test with a real SQLite decode failure and recovery, asserting routing, child absence, side-session preservation, then inherited attribution and retirement.

Validation / 验证：按任务要求未运行 cargo，测试未执行；`rustfmt --check --edition 2021` 与 `git diff --check` 通过。 / No cargo or tests run as requested; formatting and whitespace checks passed.
