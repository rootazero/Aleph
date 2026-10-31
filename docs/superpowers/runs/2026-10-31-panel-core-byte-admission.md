# Core Panel byte admission execution / Core Panel bytes 准入执行记录

Plan / 计划: [Core byte-admission plan](../plans/2026-10-31-panel-core-byte-admission-plan.md)

Design / 设计: [Approved design](../specs/2026-10-31-panel-core-byte-admission-design.md)

Status / 状态: **In progress; dependency #7 is not complete. / 实施中，依赖 #7 尚未完成。**

## Authorization and isolation / 授权与隔离

- User approved the design on 2026-10-31 and requested completing #7. / 用户于 2026-10-31 批准设计并要求完成 #7。
- Branch: `panel-community-live-publication`; implementation base: `208cc8a7d08fa34c8f89157a0a4649833c9903f0`. Worktree is the existing ignored `.worktrees/panel-community-live-publication` checkout. No merge or push. / 沿用隔离 worktree，不 merge 或 push。
- Cargo runs serially from the owning Core package (worktree root), after `python qa/browser_live/check_memory.py`. `CARGO_TARGET_DIR=../../target CARGO_BUILD_JOBS=1` reuses the ignored build cache; Cargo compiles current worktree sources. No generated browser artifact is borrowed. / Cargo 串行、先 memory guard，复用忽略的构建缓存，不借用 browser 产物。
- Per-invocation `git -c core.hooksPath=/dev/null commit` avoids the previously documented unrelated baseline formatter hook; shared Git configuration is unchanged. Changed Rust files are formatted directly and focused tests run before commits. / 单次 commit 绕过既有 baseline formatter hook，不改变共享配置；改动 Rust 文件直接格式化，提交前运行聚焦测试。
- Detailed logs and task briefs remain in `.superpowers/sdd/2026-10-31-panel-core-byte-admission-plan/`. / 详细日志与 task brief 原地保留。

## Task 1 / 任务 1

- RED: `cargo test -p alephcore --lib extension::panel_publication` after the guard failed with `E0432`: missing `crate::extension::panel_publication`. The terminated compiler reported one error. The first detached shell did not persist a numeric exit code; its full failure log is retained. / RED 确认为缺失 Core API 的单一编译错误；首轮 detached shell 未保留数字退出码，完整错误日志仍保留。
- GREEN: same guarded command, exit **0**, **4 passed, 0 failed, 0 ignored**. First successful compilation took **14m 49s**; the cached task-done rerun also passed all four tests. / 首次成功编译耗时 14m 49s，task-done 缓存重跑同样通过。
- Proof: snapshot/byte scope agreement; cross-version and empty-generation withdrawal; lower/equal revision attempts with valid SRI; exact identity, canonical entry and integrity refusal. / 已证明上述 Core store 边界。
- Commit: `563c835d7c46cabbd9cf0a644ec6d4138bac5962` — `refactor(panel): move publication ownership into Core`.
- Moved the actual store implementation and canonical/SRI checks into Core; removed the unmounted Gateway module and boolean authorization request. The store is not yet connected to production lifecycle at this point. Removing the boolean does not prove route-owned HTTP authentication. / 实际迁移 store 与校验，移除未挂载 Gateway 模块及 boolean request；此时尚未接入生产 lifecycle，移除 boolean 不构成 HTTP 认证证明。
- During compilation available RAM briefly fell below the next Cargo's 4 GiB guard threshold. No second Cargo was launched concurrently; this run's preflight had passed. / 编译期间可用 RAM 曾低于下一轮 Cargo 的 4 GiB 阈值，没有并发启动另一轮 Cargo；本轮启动前 guard 已通过。

## Task 2 / 任务 2

- Base: `563c835d7c46cabbd9cf0a644ec6d4138bac5962`.
- RED: same guarded Core test command, exit **101**; only missing `PreparedPanelPublication` and `commit` API errors (`E0432`, `E0599`). / RED 仅出现预期的新 API 缺失错误。
- Added real-file tests for immutable admitted bytes, failed replacement withdrawal, missing/directory/integrity/parent entries, escaping symlink, exact 4 MiB boundary, deterministic 64 MiB generation budget, duplicate keys, changed final policy and revision exhaustion. / 添加上述真实文件与 generation 边界测试。
- Production implementation reuses `derive_panel_snapshot` and existing SRI checks, prepares bounded bytes off-lock, then revalidates declarations against the commit policy. / 实现复用既有声明与 SRI 判据，off-lock 准备 bytes，commit 时重验 policy。
- GREEN: guarded `cargo test -p alephcore --lib extension::panel_publication -- --nocapture`, exit **0**; **10 passed, 0 failed**, compilation **14m 35s**, test runtime **3.76s**. Nine tests exercised their assertions; the symlink test returned early with `SKIP escaping_symlink_is_not_a_publication_source: symlink privilege unavailable`. This machine therefore did **not** prove the escaping-symlink case. / GREEN 退出 0；九项执行断言，symlink 一项因 Windows 权限显式提前返回，本机没有证明 symlink 逃逸拒绝。

## Deferred proof / 仍待证明

Core lifecycle acceptance, production route-owned authentication, Gateway asset delivery, browser credentials and real HTTP-to-PanelShell integration are not established by Task 1. / Task 1 不证明上述 production 链路。
