# Trusted Panel project context run / Panel 可信项目上下文运行记录

Status / 状态: implementation in progress / 实施中。

Base: `52e13cb3d`; branch: `panel-trusted-context`. Approved [design](../specs/2026-11-01-panel-trusted-project-context-design.md) and [implementation plan](../plans/2026-11-01-panel-trusted-project-context-implementation-plan.md). No merge or push. / 设计与计划均已批准；不合并、不推送。

## Decisions / 决定

- The approved design takes precedence over the plan's contradictory instruction to preserve browser content on selection refusal: invalidate and dispose first; refusal leaves project Panels unavailable. / 切换前失效并清理；拒绝后保持不可用。
- Focused private-selector tests compile the production source module against the regular library; they do not prove Gateway wiring. Real RPC/HTTP acceptance must use the independent production-library integration target. The monolithic lib-test harness previously hit LLVM OOM in three configurations. / 私有模块测试不冒充真实 ingress 验收；沿用独立 production-library target。
- Selector generations are context metadata, never Core publication revisions. Any response metadata must remain non-authorizing and must not contain roots. / selector generation 仅用于上下文隔离，不是 Core revision，不包含 root。
- Selector ownership must remain tied to the admitting user when a device is rebound; project roster overlap cannot transfer a previous user's selection. / 设备换绑不能继承旧用户 selection，即使两人均为项目成员。

## Verification / 验证

Commands run from the worktree root unless stated otherwise. Every Cargo command is preceded by `python qa/browser_live/check_memory.py` and uses `CARGO_BUILD_JOBS=1`. Build artifacts reuse the main checkout's target directory serially; no concurrent Cargo commands. / 所有 Cargo 前执行内存守卫，单 job 串行构建；仅复用构建产物。

Initial selector RED: `cargo test -p alephcore --test panel_project_context -- --test-threads=1` — regular production library compiled; the test then failed because the new source module did not exist. / 正常 production library 编译通过，测试因新模块不存在而编译失败。

The first implementation attempt referenced three `cfg(test)`-only SecurityStore setters; replaced with the real public device upsert API. The first behavioral run reported **6 passed, 1 failed**: the fixture attempted two active projects with the same owner/workspace and hit `idx_projects_owner_path_active`, before the authorization assertion. The archived fixture now uses a distinct workspace. / 首次类型错误来自测试专用 setter；已改用公开设备 API。首次行为运行 6 通过、1 失败，原因为 fixture 唯一约束冲突，已修正。

Module registration is deferred to Task 2's runtime wiring; the Task 1 target compiles the exact private production module source and keeps its tests out of the monolithic harness. The selector also stores the admitting user as device-binding provenance, checked against current SecurityStore on each use; this is not a second identity authority. / 模块在 Task 2 接线时注册；Task 1 直接编译同一私有源码。保存准入用户仅用于绑定来源检查，每次仍查询 SecurityStore。

The approved design/plan documentation commit uses a per-invocation disabled Git hook; no global hook configuration changed. Product verification is recorded separately below. / 文档提交仅本次绕过 hook，未修改全局配置；产品验证独立记录。

Task 1 final focused verification: **7 passed, 0 failed**. A temporary omission of the active-project gate produced the expected behavioral RED (archived claim incorrectly returned `Ok` instead of `Err`); restoring the gate returned the full target to **7 passed, 0 failed**. / Task 1 最终 7 通过、0 失败；临时省略 active gate 的突变测试实际捕获 archived claim 被接受，恢复后全绿。

## Remaining / 剩余

RPC context, HTTP context, browser lifecycle, production Gateway/browser evidence and fresh whole-branch review remain open. The private selector tests do not establish runtime integration. / RPC、HTTP、browser、真实验收与整分支审查仍未完成；私有 selector 测试不证明 runtime 接线。
