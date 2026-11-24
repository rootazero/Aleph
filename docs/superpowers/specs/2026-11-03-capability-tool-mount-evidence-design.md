# 生产 Capability mount 证据与 authority 对齐设计 / Production Capability Mount Evidence Design

**Date / 日期:** 2026-11-03
**Status / 状态:** Proposed next bounded task; implementation deferred to the restarted session / 下一有界任务建议；本会话仅准备交接
**Baseline / 基线:** `4b6da265a feat(capability): activate cold tools on first call` on local `main`; later handoff-only commits are allowed / 本地 `main`，允许后续纯交接提交。

## Intent / 意图

The overall goal is runtime lifecycle isolation and composability with one authority per fact. First-call activation for the bounded Tool-only WASM slice is complete. Production startup already mounts `ProjectionHost` with the canonical Tool registry, shared ownership tree and concrete Memory Resource. The next useful closure is evidence and documentation alignment, not another mount implementation.

总目标是运行时生命周期隔离和可组合性，并保持每项事实的唯一权威。Tool-only WASM 首调激活已经完成。生产启动已经把 canonical Tool registry、共享 ownership tree 和具体 Memory Resource 组合到 `ProjectionHost`。下一步应闭合生产接线证据及文档一致性，不重复实现 mount。

## Approaches / 方案

1. **Selected recommendation: evidence-first bounded alignment / 推荐采用：先证据后校准。** Reuse existing wiring and effect tests, add only missing effect-level evidence, then synchronize current authority claims. This keeps scope reviewable; it postpones general non-Tool lifecycle architecture.
2. **Another mount implementation / 再实现 mount。** Rejected: production composition already exists; another host/registry would duplicate ownership.
3. **Expand every kind now / 同时扩展所有 kind。** Deferred: Channel/Terminal/MCP/Feishu and durable effect producers require their own approved contracts and cannot be inferred from Tool activation.

These are the planner's recommendation for the next session, not a claim that a new architecture has already been approved. / 以上为下一会话建议，不声称新的架构已经获批。

## Current evidence / 当前证据

Line numbers are navigation hints at the baseline; function names are the stable anchors. / 行号仅为基线导航提示，函数名为稳定锚点。

| Fact / 事实 | Source / 来源 | Existing evidence / 既有证据 |
|---|---|---|
| One supplied ownership tree; canonical Tool registry shared with consumers / 共享树与 canonical registry | `src/bin/aleph-server/commands/start/mod.rs:242`, `:267`, `:289` | `boot_mounts_projection_host_from_canonical_registry_and_awaits_readiness` is a source-wiring guard, not runtime proof / 源码接线守卫，不等于运行时效果证明 |
| Subscribe before snapshot, Ready only after applied state / 订阅先于 snapshot，Ready 等待应用 | `src/capability/projection_host.rs::mount_sources`, `wait_until_ready` | `initial_delivery_via_real_applier_and_fails_closed` |
| Startup errors retain resources for common cleanup / 启动错误走共同清理 | `src/bin/aleph-server/commands/start/builder/startup_resources.rs::StartupResources::{cleanup,finish}` | `early_registration_error_drains_memory_and_its_projection_host`, `early_startup_failure_preserves_original_error_after_cleanup` |
| Memory stops before host closes; common cleanup precedes builtin scope disposal / Memory 先 stop，再关闭 host，随后 dispose builtin scope | `StartupResources::cleanup`; start module shutdown funnel | `boot_closes_projection_host_before_registry_scope_dispose` is an ordering guard / 顺序守卫 |
| Host close joins source and applier; caller cancellation does not detach teardown / close 等待两个 worker，调用者取消不丢收尾 | `ProjectionHost::close_and_await` | `cancelled_first_waiter_does_not_detach_teardown`, `idle_close_wakeup_joins_both_workers` |
| Concrete Memory Resource is already composed without a fake Tool entry / 已组合具体 Memory Resource，不伪造 Tool | `ProjectionHost::mount_with_memory`, `ZahirFacade::new_with_memory` | `resource_is_discoverable_without_fake_tool_entry`, `resource_lease_is_source_backed_and_controls_stay_independent` |

The Tool callable backend remains `ToolBackendAdapter`; Memory Resource is a narrow source-backed composition, not a generic backend for every Resource or a transfer of Memory lifecycle authority. `MemoryRuntime` owns derived resource admission/work, raw chat/archive stays available Off. / Tool 可调用后端仍为 `ToolBackendAdapter`；Memory Resource 是窄化的 source-backed 组合，不代表通用 Resource 后端或 Memory 生命周期权威转移。`MemoryRuntime` 继续拥有派生资源准入及任务，Off 时 raw chat/archive 保持可用。

## Deliverables and acceptance / 交付与验收

- A bilingual evidence matrix distinguishes production call sites, static wiring guards, real effect tests, newly executed results and historical results. Reuse passing evidence; do not manufacture a defect to justify code changes. / 双语证据表区分生产调用点、静态守卫、真实效果、新跑结果和历史结果；不为改代码制造缺陷。
- Prove shared authority through observable canonical publication/owner invalidation and existing mismatch rejection, not counts of constructors or test assertions copied from implementation. / 通过实际 publication、owner invalidation 和 authority mismatch 拒绝证明共享权威，不只数构造次数。
- Cover readiness, early startup error, orderly cleanup, cancelled cleanup waiter and repeated cleanup. Add real-effect RED/GREEN only when source comparison establishes a missing guarantee; bounded Notify/channel handshakes, no sleep races/global mocks/public test-only APIs. / 覆盖 readiness、早期错误、正常清理、取消等待者与重复清理；仅为确有缺口补真实 RED/GREEN。
- Synchronize current claims in `AGENTS.md` and `CLAUDE.md` together; correct their stale no-production-mount and all-other-kind-deferred wording to acknowledge the concrete Memory Resource exception. Match owning reference docs; preserve dated evidence as historical. / 同步两个宪法入口及所属 reference，修正“无生产 mount”等过期说法并列明 Memory Resource 例外，保留历史证据身份。
- Record remaining deferred work precisely and recommend one separately designed next slice; do not authorize it through this task. / 准确记录 deferred 项，建议一个后续独立设计切片，不由本任务默许实施。

## Constraints and stop conditions / 约束与停止条件

No new registry, tree, generation authority, Capability kind, lifecycle supervisor or provider proxy. No broad startup rewrite, raw archival ownership change, Feishu/Channel/Terminal mount, MCP lazy activation, EffectClaim producer or Safe Replay expansion. Do not claim universal stop, native unload, RSS reclamation, process isolation or external exactly-once. If evidence requires any such change, record the exact blocker and seek a separate design decision. / 禁止新增上述权威或扩展；发现必须如此时记录具体阻塞，另行设计。

Native execution: main agent sole writer, read-only children for review only. Local commits on the user-requested `main`; no push, force, reset, synthetic merge or remote synchronization. / Native 主 Agent 唯一写者；只读子任务负责审查。本地提交到用户要求的 main，不推送、强制重置、伪造 merge 或同步远端。

Before every Cargo command require `python qa/browser_live/check_memory.py` to print `ALLOW` and at least `4294967296` available bytes. Set `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`, run serially, require positive selected test counts. Native Linux/Windows execution remains waived; disclose it. / 每次 Cargo 前执行内存门禁，串行运行并要求实际选中测试；Linux/Windows 原生执行豁免继续有效并披露。
