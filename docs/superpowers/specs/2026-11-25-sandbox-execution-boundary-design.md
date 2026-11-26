# Sandbox execution boundary / 沙箱执行边界

**Status / 状态:** Operator approved after independent builder check / 独立可实施性审阅后，用户已批准。

## Intent / 意图

让 Agent 看到的隔离状态和真正约束子进程的策略一致。Team 和 subagent 的 Git worktree 隔离应当与 OS 进程隔离叠加，而不是二选一。先接通现有运行、授权和驱动，再增强失败诊断、删除旧执行器。成功标准是选定的工作区和现有 OS 安全策略真正到达执行器，而不是新增了一个抽象或 prompt 标签。

An Agent's reported execution posture should match the process boundary in use. A Git worktree separates checkouts; the existing OS backend enforces process isolation. Compose the two without introducing a second policy engine.

## Approach and evidence / 方案与证据

| 维度 / Dimension | `sandbox-runtime` (`f7a0f19`) | Codex (`1885417cd4`) | Aleph (`f4864b17e`) / decision |
|---|---|---|---|
| 策略到启动 / Policy to spawn | `src/sandbox/sandbox-manager.ts:1164,2677,3481` coordinates configuration, lifecycle and wrapping. | `codex-rs/sandboxing/src/manager.rs:120,359,580` turns typed inputs into execution requests, including direct spawn. | `src/sandbox/workspace/mod.rs:327,712–779` already shares preparation; keep it and its late checks. |
| 可用性与拒绝 / Availability and denial | `src/sandbox/linux-sandbox-utils.ts:1304` distinguishes dependency errors and warnings; Windows has a separate status probe. | `codex-rs/sandboxing/src/manager.rs:191` has typed transform errors; `codex-rs/core/src/tools/orchestrator.rs:391–435` handles denial after an attempt. | `src/sandbox/workspace/mod.rs:206–225,740–770` checks readiness at different times. Separate known backend unavailability from unsupported policy, approval denial and runtime failure. Do not adopt escalation-by-retry. |
| 工作区和权威 / Workspace and authority | Command wrapping uses effective configuration and cwd. | `codex-rs/sandboxing/src/manager.rs:100–142` carries execution and policy inputs together. | `src/teams/dispatcher/runner.rs:434–474` passes both workspace and sandbox override. `src/orchestrator/dispatch.rs:1199–1217,1354–1376` selects a handle for the harness but does not bind it for exec tools. `src/builtin_tools/bash_exec.rs:462–526` carries workspace but not the selected override across spawn. Fix scope propagation, not the removed model-facing selector. |
| 展示和生命周期 / Reporting and lifecycle | `src/sandbox/sandbox-manager.ts:3540,3656` separates command cleanup and reset. | Typed execution and denial are separate from policy declarations. | `src/sandbox/factory.rs:115–132` rejects while reporting unrestricted execution; `src/sandbox/workspace/mod.rs:123–158,174–202` execution and reporting disagree on the no-run fallback root. Reuse authoritative inputs; label unobserved readiness honestly. |

**Selected / 已选：** 在方案 A 中，统一两种运行的工作区输入，保留共用生产 OS 执行器；删除 worktree 直启进程路径。用户已选择 Team 和 subagent 的 worktree 纳入 OS 沙箱。代价：原先能借宿主权限完成的命令可能需要既有授权或被拒绝；三平台分别验证，不追求未经验证的后端功能等价。方案 B 只修连线和诊断，保留旧 worktree 执行边界，迁移成本较低，但会留下两套语义，因此未选。完全重写或引入独立 sandbox registry 无复用收益，不采用。

**A, selected:** unify the execution boundary and apply the mounted OS sandbox to worktree commands. **B, not selected:** repair projection/routing while retaining the former non-OS executor. A has a higher compatibility cost but removes the split in enforcement. Neither option calls for a new provider or registry.

## Contract / 契约

### 1. One production execution boundary / 一个生产执行边界

- 现有 `build_sandbox` 挂载的 OS sandbox 仍为执行权威；Team 和 subagent 只选择受信工作区，不得替换成另一执行器。迁移时先连通 Team 与 subagent 的工作区输入，使选定根目录同时到达 prompt、文件工具、Bash/Code Exec/Code Check 的真正执行，再移除 `WorktreeSandbox`。现有仅传递它的 gateway/orchestrator override 字段及占位透传一起清理；保留独立的 workspace override 语义。不得新增生产的、可替换安全权威的 override seam。
- 保留 `WorktreeHandle` 的 Git 创建和清理职责及 `FsScope` 的文件工具约束。嵌套 subagent 未请求新 worktree 时继承父 worktree；明确请求新 worktree 时，以该受信 checkout 的当前提交为源，不默认回到主 checkout，也不复制未提交改动。没有父 worktree 时取当前运行已授权的项目或执行工作区。Team 后台调度从已授权的项目 scope/绑定取 checkout（通过现有项目解析器），不从自由文本、调用参数或 daemon cwd 推断。无绑定、已确认不是 Git 仓库、或维护者用 `ALEPH_TEAM_MEMBER_WORKTREE=0` 禁用时，自动 Team 任务可使用普通 OS 沙箱继续，并准确标明没有 worktree；显式请求的 subagent 无受信 Git checkout 时失败。绑定查询失败、checkout 已失效或创建失败须报告，不转成无绑定或悄悄改用父项目。ACP 外部成员和显式不请求 worktree 的 Team 调用不变。
- 重用 `EXEC_WORKSPACE`、已有作用域传递设施及文件作用域中已有的工作区事实；如有必要，在现有执行工作区承载者中区分已挂载 worktree 和普通根，不新增并行的作用域事实源。worktree 根决定 OS 基线策略，受检查的 `cwd` 只决定子进程启动目录；从子目录执行仍可写 worktree 内的 `target`。普通运行保留现有 `cwd` 策略语义，其 prompt 只陈述基线而不保证单次调用的可写范围。启动前验证 worktree 根仍存在，不由 session fallback 创建或重建。
- 父 checkout 不加入默认可写根；显式额外写权限仍走现有审批，只有原策略和平台允许且现有授权链批准才可生效，不添加父 checkout 专属许可或不可覆盖的新禁令。读取仍受现有平台策略约束，不宣称 worktree 提供读取隔离。宿主侧 Git 创建、清理不属于 Agent 启动的进程。
- 保留 `CARGO_TARGET_DIR=<worktree>/target`，从受信根派生，放在现有 OS 命令准备边界；不解析 shell 文本来识别 Cargo，不为了兼容开放父仓库、默认网络或增加新的写入例外。普通项目和已配置的宿主服务不继承 worktree 特例。
- 不允许有自动清理所有者的 Team/subagent worktree 发起 detached Bash，明确报错、不给 job id、不启动子进程；同步命令继续可用。非 worktree 的后台 Bash 保留授权工作区和调用归属。Team 超时后在既有有界清理窗口内等待被取消的 Rust task 结束，不把固定睡眠当作确认；超出窗口记录未确认，沿用尽力清理。正常完成先收同步命令结果再清理，取消/超时沿用各驱动终止机制和 worktree 的清理保护。本轮不保证三平台的原生进程树已全部退出后才移除目录，也不重写进程监督器；验收区分 Rust task 确认与原生进程确认。
- Browser companion 沿用其既有 OS `ManagedSandbox` 挂载及普通调用的工作区语义。它的 `start` 同样会创建由 manager 而非 worktree owner 管理的长期子进程，因此从自动清理的 worktree 调用 `start` 时明确拒绝，而不是在 worktree 外另选工作区或修改已运行 companion 的生命周期。保留共享 `prepare_command`、钩子、限流、审批、后验 cwd 检查、child/proxy 所有权和现有输出处理，不另设平行实现。

The boot-mounted sandbox remains authoritative. A worktree fixes the policy root while a checked child cwd only controls launch location; existing approvals still govern explicit additional writes. Nested worktrees start from the trusted parent's committed HEAD. Team uses an authorized project binding, not daemon cwd. An automatic Team run with no binding, no Git checkout, or an operator-disabled worktree remains an ordinary OS-sandboxed run; setup failures do not silently fall back. Owned worktrees reject detached Bash and new browser companions. Cancellation waits only within a bounded window for Rust task completion; it does not certify native process-tree exit. Ordinary background Bash and companion lifecycle stay unchanged.

### 2. Accurate posture and typed refusal / 如实投影及类型化拒绝

- 明确区分禁用（拒绝执行）、后端不可用、策略不受支持、授权拒绝、启动/运行失败。普通执行在能够无副作用判断硬依赖缺失时，先拒绝，后续 hooks/审批/代理和启动不发生；托管启动仍单独验证其更强的可用性要求。保留驱动后验检查，不能把探测成功表达成启动保证。禁止不隔离重试或自动放宽策略。探测必须是有界、只读的；不把宿主进程探测塞进每次 prompt 组装。
- Prompt 从生产执行器的基线和运行已选定的受信根得到声明；无明确运行根时，有 session 则按执行的 session 派生规则报告，否则不宣称知道具体可写根。禁用不能投影成 `DangerFullAccess` 或网络全开。未运行时不声称内核已落实策略；动态审批也不投影成基线事实。子 Agent 不再自己合成仅有 Git 隔离的 OS 说明。
- `bash`、`code_exec`、`code_check` 仍是 Tool Capabilities，经现有 Tool registry 和 scoped admission 投影及调用。进程沙箱是 Core 的执行与隔离机制，不注册为模型可选择的类型或无消费者的 backend。失败要沿实际工具返回和运维诊断通路呈现，不能只新增日志或只改 prompt；也不能把后端错误映射为授权成功。

Distinguish disabled, unavailable, unsupported, denied and failed. Apply cheap hard-dependency checks before side effects; keep each platform's authoritative launch checks. Report configured posture and selected scope, not a promise of kernel enforcement or a record of temporary elevation. The existing Tool Capabilities are the external surface.

## Delivery and verification / 交付与验收

1. **连线优先 / Wire first:** 在隔离分支用行为回归证明从 Team 和 subagent 入口到受信工作区、prompt、文件与真实 exec 消费者的连线；用带记录的现有驱动测试 seam 证明实际传入 cwd/策略，而不是只断言方法被调用。保留普通 foreground/background 与显式合法 cwd 的回归；验证嵌套继承和显式嵌套 HEAD、同一 session 的不同根、可信 Team 项目绑定和无绑定分支；从子目录执行仍写入 worktree 内 target，未授权写父 checkout 被拒、现有显式审批不回归。明确需 worktree 但创建失败、已清理和禁用状态不掉回父 cwd。
2. **收敛和增强 / Consolidate and extend:** 统一根和基线的投影，接入类型化拒绝。证明不可用不启动、不等待审批或启动代理；不可表示的策略仍失败，启动时的真实失败仍可区分。验证 worktree 不会留下新启动的后台 Bash 或 browser companion，普通 managed 调用、现有输出和生命周期回归保持。
3. **清理 / Remove dead paths:** 移除 `src/sandbox/worktree.rs` 中的直启执行器、重复 hook/drain 调用和只属于旧语义的 summary、exports、request fields、测试/文档描述；保留 Git handle 及通用 helper。清单以生产引用计数为准，不为了收敛改写无关 dispatch 或 `src/harness/`。
4. **验证并提交 / Verify and commit:** 依 `docs/reference/DEVELOPMENT.md` 跑适用验证，记录命令和结果。用本机支持的 OS 后端运行无害工作区读写、拒绝和常用构建冒烟；其他平台分别记录可获得的编译、CI 或真机证据，不用 mock 或跨编译冒充真机。校对 `docs/reference/FEATURE_LOCATOR.md` §3.15、`docs/reference/SANDBOX.md` 及有实际变化的相邻 reference；§3.8 仍为内容安全，不混同进程隔离。仅提交到隔离分支，不合 main。

**Builder choices / 留给实施者：** 具体 Rust 类型、字段命名、错误映射和探测缓存由实现者按现有模式选择；不要求新增依赖，不承诺未测性能提升。若同一行为可以通过现有作用域设施表达，复用优先。

**Non-goals / 不做：** 新驱动、新代理、TLS 中间人、自动重试或降级、扩大授权范围、重写整个 sandbox 或 harness、承诺 Windows/Linux 与 macOS 能力一致、变更已有的审批策略。
