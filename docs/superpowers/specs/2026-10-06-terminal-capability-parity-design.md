# Terminal Capability Parity：TUI-first 终端能力架构设计

- 日期 / Date: 2026-10-06
- 状态 / Status: 原设计已批准；2026-10-06 人类交互输入例外已单独确认。实现计划仍待审阅，未授权产品实现 / Original design approved; human-interaction exception confirmed separately; implementation plans await review.
- 父级约束 / Parent constraints: `Everything externally composable is a Capability`（Zahir v3）、`AGENTS.md` R1–R10、`docs/reference/HARNESS_PHILOSOPHY.md`
- 相关设计 / Related designs: `docs/superpowers/specs/2026-10-05-capability-phase3-design.md`、`docs/superpowers/specs/2026-10-04-capability-phase2-durable-tool-design.md`、`docs/reference/TERMINAL_RUNTIME.md`、`docs/reference/FEATURE_LOCATOR.md`
- 参考实现 / References: herdr、Orca、Paseo；只提取生命周期、恢复、协议和验证机制，不复制它们的 UI 或 VT 实现

> 本文是设计契约，不是 implementation plan。原设计已批准进入 writing-plans；产品实现须等待书面计划审阅，且仅在独立 worktree 中进行 / Design approval permits planning, not product implementation.

## 1. 目标与问题重述 / Goal and problem statement

本次工作不是给 Panel 增加一个更大的 terminal widget，也不是把 herdr 的 API 原样搬进 Aleph。目标是把 Aleph 的终端能力重建成一个 **TUI-first、Core-owned、capability-driven 的多 agent 工作台**：

- PTY/VT 屏幕、前台进程、agent 识别、状态、cwd、会话生命周期和事件流由 Rust Core 持有；
- TUI、CLI、Panel、LLM tools、后台 continuation 都是 Core capability 的 I/O/projection face；
- herdr 的 terminal、sessions、panes、layouts、workspaces、agent status、prompt/send-keys/start、hooks、persistence/recovery 和 worktree 能力全部覆盖，但不复制 herdr 的内部模块或 `libghostty`/VT 实现；
- 主动写入必须服从严格 Aleph 策略：默认 `terminal_write=off`，只允许针对已识别且当前前台确认的 agent，复用 ExecTier/approval/audit，且不得绕过 `sandbox.command_policy`；spawn/close/hooks 另行治理；
- 清除“RPC 有方法但没有 canonical owner”“工具有类型但不可达”“Panel 有投影而 TUI 没接线”“调用成功被误报成效果到达”等断链。

当前结构已经有一部分能力，但不是同一生命周期：

```text
现状：PTY manager -> pty.* RPC/event
      runtime detector -> runtime.* RPC/event
      terminal tool -> 第三只读 lens
      TUI -> 部分 runtime/PTY 消费者
      Panel/CLI -> 另一些协议消费者
      写入、hooks、恢复、workspace/BSP -> 未统一
```

目标结构是：

```text
Capability descriptor / owner / policy / lifecycle
                 │
       Core canonical state + event sources
                 │
       typed projection / RPC / tool / task
          ┌──────┼────────┬─────────┐
         TUI    CLI     Panel     LLM/continuation
```

## 2. 已确认的架构决策 / Decisions already made

### 2.1 总体方案：A + B

采用 **按能力族垂直收敛（A）+ 最小内部 service（B）**：

- 按 Terminal Session/PTy、Runtime Agent、Workspace/Layout、Persistence/Recovery、Hooks 五个能力族组织事实和生命周期；
- 允许一个 Core 内部的 `TerminalRuntime` / `TerminalSessionStore` 作为 attach、恢复和跨族编排的依赖；
- 内部 service 不对外注册 capability，不持有第二套 descriptor、handler registry、protocol business rule 或权限判断；
- 每个外部可调用动词仍通过 canonical descriptor/registry；每个可订阅状态通过 canonical event/resource source；只有确实需要跨重启持续执行的行为才建 Task；
- `shared/protocol` 只保存 wire DTO、method/topic 名称和兼容序列化，不决定业务授权、恢复或状态推理。

### 2.2 写入策略：严格 Aleph policy

- 配置默认 `terminal_write=off`；`ask`/`on` 也不能越过 caller identity、ownership、agent detection、foreground confirmation、ExecTier、approval、audit 和 sandbox command policy；
- 以下限制适用于 LLM/后台 agent 控制；人类交互字节流采用 §2.3 的独立授权例外 / These restrictions govern agent control, not the separately authorized human-interaction path.
- `prompt`/`send-keys` 是不同 capability verb，不能用一个字符串参数把二者混成任意输入；
- 文本写入必须非空，目标 session 必须存在且为 caller 可见，必须有当前识别 agent 和确认的 foreground process；
- agent 为 `Blocked` 时，`prompt` 默认拒绝；`send-keys` 是否允许由独立 descriptor policy 声明，用于回答阻塞中的确认/按键；
- prompt 使用 bracketed paste，并将 Enter 延迟为独立输入动作；不把换行直接解释成任意终端控制序列；
- `start`/`spawn`、`close`、hooks install/enable/disable 都是独立 capability，拥有独立 approval tier、审计事件和恢复语义；
- terminal capability 不能成为 `sandbox.command_policy` 的旁路。任何需要执行命令的 start/worktree/hook 操作必须经既有 command policy 或专门的等价治理能力；
- 当前 `ScopedToolService` approval verdict 未传入 terminal inline gate 的已知 seam 必须在写入能力接线前解决；不得删除 inline gate，也不得把“approval card 已显示”当成“执行已授权”。

### 2.3 已确认：人类交互输入例外 / Confirmed human-interaction exception

用户已选择区分人类交互与 agent 控制。完整交互 shell 采用独立 capability：

- 原始键入默认不授予；实际 operator TUI/Panel 连接必须先显式批准针对单一 session 的交互授权，授权绑定 actor、连接、session generation，断连、关闭、禁用或撤销时失效 / Raw typing requires an explicit, revocable connection-bound grant.
- 授权不能由 wire 中的 `human=true` 或调用者自报 UI 类型产生；LLM、后台任务、continuation 和普通 `tools.invoke` 不得取得或继承此授权 / Client-supplied labels cannot establish human provenance.
- 人类交互输入不要求前台为已识别 agent；LLM 的 prompt/send_keys 仍执行 §2.2 的检查。旧 `pty.input` 必须接入相同授权，不能成为兼容旁路 / Legacy input must use the same admission boundary.
- 初始 PTY 启动和自动 start/worktree/hook 命令仍执行 command policy。之后原始字节流无法逐命令治理；这是明确批准的安全例外，不得宣称对交互 shell 实现了逐命令 sandbox.command_policy / Interactive bytes are not command-policy-equivalent.
- 审计保存授权、动作元数据和结果，不默认保存可能含凭据的完整输入 / Audit grant and effect metadata without recording secret-bearing raw input by default.

## 3. Zahir capability 映射 / Capability model

本设计不新增一个笼统的 `TerminalCapability` trait，也不把所有资源塞入 Tool registry。每个 family 采用已有的 descriptor/handler/scope/projection 机制，按 kind 区分 callable、resource/event source 和 task。

### 3.1 Terminal Session / PTY family

**事实 owner：** 最小内部 `TerminalSessionStore`，复用 `src/gateway/pty/{session,manager,screen}`；PTY bytes、VT grid、cursor、alt screen、title、bracketed paste、live cwd、seq 和 attach snapshot 只在这一处派生。

**Callable capabilities：**

- `terminal.sessions.list`
- `terminal.sessions.read`
- `terminal.sessions.status`
- `terminal.sessions.wait`
- `terminal.sessions.explain`
- `terminal.sessions.prompt`
- `terminal.sessions.send_keys`
- `terminal.sessions.start` / `spawn`（外部别名是否保留由协议兼容表决定）
- `terminal.sessions.resize`
- `terminal.sessions.close`

已有只读 `terminal` tool 可暂时保留为 compatibility projection，但不能继续成为第二套动作定义；其五个 action 必须逐步投影到同一 descriptor/action schema。`pty.spawn/input/resize/close/list/attach` RPC 也必须明确标记为 projection 或 compatibility face，而不是新的 canonical operation。

**Resource/EventSource：** screen snapshot、screen patch、exit、runtime change、attach/reconnect 状态。事件必须带 session identity、generation/seq 和 owner-filtered visibility；漏帧仍要求 re-attach，不能由 TUI 猜测补帧。

### 3.2 Runtime Agent family

**事实 owner：** `src/gateway/pty/foreground.rs` + `src/gateway/runtime/`，沿用 `docs/reference/TERMINAL_RUNTIME.md` 的不变量：

- 进程 detection 优先于屏幕文案；
- `PROBE_MIN_INTERVAL_MS=500`、`PROBE_RECHECK_MS=3000`、`PROBE_MISSES_TO_FORGET=6`；
- cwd 优先级 OSC7 > foreground process cwd > spawn cwd；
- `quiet_since` 只是 observed output silence，不得派生 `Idle`；
- `start_flush_loop` 是唯一 16ms 时钟；
- `RuntimeAgentEntry` 是 runtime wire 的 canonical row。

**Callable/resource faces：** runtime agent list/status/wait/explain，以及 agent manifest inspection。状态 `Working/Blocked/Idle/Unknown` 只能从同一检测结果投影，不得在 TUI、Panel、tool 中重复推理。

### 3.3 Workspace / Layout family

**事实 owner：** Core-owned workspace model，存储逻辑 tab/pane/tree/active selection/worktree binding，不存像素或 ratatui layout。

**Callable capabilities：** workspace list/create/open/close、tab create/select/close、pane split/close/focus/resize、layout save/restore、active session binding。

BSP 是派生树：节点保存 split direction、ratio、children 或 leaf session reference；禁止把渲染后的矩形当成 durable truth。TUI 的拖拽只提交逻辑 ratio，Core 返回规范化后的 tree；Panel/CLI 可使用相同 tree projection。

### 3.4 Persistence / Recovery family

**事实 owner：** 复用 SessionEventStore、durable intent、`boundary_repair`、`ResumeCoordinator` 和现有 process journal；不新增 terminal 专用数据库或第二套 scheduler。

持久化的是：session intent、logical workspace/layout、worktree identity、owner/created_by、spawn metadata、last observed sequence、recovery classification 和 operator-visible tombstone；不持久化 PTY master fd、Rust handler、闭包、凭据或不可重建运行时对象。

恢复分成三个不可混淆的动作：

```text
observe/reconcile -> classify -> operator/model chooses resume/recreate/abandon
```

`Unknown`、`LostWithRestart`、`ReplayRefused`、`DescriptorMismatch` 都必须 fail-closed，不能转成成功、许可或静默自动重放。PTY 的真实进程/FD 恢复若平台能力不足，必须返回结构化“不可恢复 + 下一步”而不是伪造已恢复 session。

### 3.5 Hooks family

hooks install/enable/disable/status/repair 是独立 capability，不能作为 terminal prompt 的隐式副作用。第一阶段只支持 lifecycle-capable agents 的显式用户配置写入：

- 目标 agent、配置路径和变更摘要必须可审计；
- 需要显式 consent，并服从 approval/command policy；
- install/update/remove 要有 durable intent/outcome；
- hooks 触发的 runtime event 只进入 Core event source，不直接调用 TUI；
- hook 不得获得任意 filesystem/process 权限，除非其 descriptor 明确声明并经过相应 gate；
- 恢复时区分“配置写入成功”“agent 尚未重新加载”“hook 触发未知”，不能合并成一个 success。

## 4. Canonical ownership matrix

| 外部事实 / 动作 | Canonical owner | 对外 kind | 现有兼容面 | 不能成为 owner 的地方 |
|---|---|---|---|---|
| PTY child/master、VT grid、screen seq | `TerminalSessionStore` over `src/gateway/pty` | Resource + EventSource | `pty.screen`, `pty.exit`, `pty.attach` | TUI、Panel、protocol DTO |
| session create/input/resize/close | PTY operation descriptors + PTY handler | Callable | `pty.spawn/input/resize/close` | raw gateway handler、tool inline action map |
| agent foreground/process/cwd | `RuntimeAgent` detector/store | Resource + EventSource | `runtime.agents.list/changed` | terminal tool、TUI status inference |
| read/status/wait/explain | descriptor-backed Tool projection | Callable | `terminal` read actions | standalone duplicate tool logic |
| prompt/send-keys | write operation descriptors | Callable + audited Effect | future terminal write/tool/RPC | `sandbox.command_policy` bypass、TUI local write |
| start/spawn/close | lifecycle descriptors | Callable + audited Effect | legacy pty verbs / herdr alias | one generic “terminal command” dispatcher |
| tabs/panes/BSP/active session | `WorkspaceStore` | Resource + Callable | TUI local layout, future workspace RPC | ratatui state, Panel-specific layout model |
| session/workspace recovery | `RecoveryCoordinator` + SessionEventStore | Task/Resource | process journal, resume RPC | PTY manager guessed restart |
| worktree binding | existing sandbox/worktree owner + workspace projection | Resource + Callable | project/worktree faces | terminal handler creating arbitrary dirs |
| hooks config lifecycle | Hooks capability owner | Callable + Resource | `hooks_manage`, `hooks.*` | prompt/start side effects |
| auth/approval/audit | existing policy/approval/event systems | Policy/effect metadata | `ScopedToolService`, RPC auth | TUI boolean flags, terminal local checks |

**Registration rule：**每个 callable row 必须有 descriptor identity、schema、revision/generation、scope owner、policy/effect metadata、projection adapters 和 outcome/recovery classification。每个 resource/event row 必须声明 visibility、replay/attach semantics、sequence/gap behavior 和 disposal owner。不能仅因为已有一个 RPC method 就视为已注册 capability。

## 5. 内部 service 边界 / Minimal internal service boundary

允许的内部组合形态：

```text
TerminalRuntime
 ├── TerminalSessionStore       // PTY + VT state, no external registry
 ├── RuntimeAgentStore          // detection facts and watch
 ├── WorkspaceStore             // logical tabs/panes/worktree bindings
 ├── RecoveryAdapter             // durable intent/reconcile only
 └── HookAdapter                 // explicit config lifecycle only
```

`TerminalRuntime` 只提供 typed internal dependencies and orchestration：attach、reconnect、session-to-agent correlation、workspace binding 和 shutdown ordering。它不得：

- 暴露第二套 `register(name, handler)`；
- 自己生成 ToolCatalog/ToolHandlerRegistry entries；
- 自己决定 caller 是否获批、是否属于 operator、是否可绕过 sandbox；
- 自己把 `quiet_since`/屏幕文字推断成 `Idle`；
- 把 RPC method name 当业务 identity；
- 在 `src/harness/` 增加恢复策略、调度器或平台 API。

外部写入路径统一为：

```text
caller
 -> canonical descriptor resolve (identity + owner + visibility)
 -> policy/approval/ExecTier + sandbox.command_policy
 -> durable intent barrier
 -> captured handler on TerminalRuntime
 -> effect/outcome event
 -> protocol/tool/TUI projection
```

resolve 时捕获同代 descriptor + handler；registry replacement 不改变 in-flight invocation。dispose 先让新 resolve 不可见，再按现有 scope 语义清理；不强杀已开始的 PTY 外部效果。

## 6. Protocol and projection contract

### 6.1 Wire types

`shared/protocol` 新增或扩展 typed DTO 时遵守以下规则：

- session id、workspace id、pane id、layout revision、screen seq、descriptor revision、approval id 必须是明确字段，不把它们藏进自由 JSON；
- snapshot/patch/event/command response 分离；command response 说明“intent accepted/started/effect observed/unknown”，不能只返回 `success: true`；
- screen snapshot/patch 沿用 `PtyScreenFrame`/`PtyAttachResponse` 的原子 geometry + seq 语义；
- workspace layout 使用逻辑 BSP DTO，不发送客户端渲染矩形作为真相；
- Runtime row 沿用 `RuntimeAgentEntry`，不复制一套 TUI 状态 enum；
- 新字段要保持旧 Panel/CLI 的反序列化兼容，旧客户端不能把“缺字段”解释成“无 session/已恢复”；
- method/topic 常量和 server census 的 literal 约束要同时满足，不能因共用常量让 method census 漏记。

### 6.2 Projection faces

| Face | 允许做什么 | 禁止做什么 |
|---|---|---|
| TUI | 订阅、请求 snapshot、提交 typed command、展示 refusal/recovery、维护纯 UI focus/input state | agent 状态推理、权限决定、PTY bytes 解析、第二份 layout truth |
| CLI | 调用同一 RPC/capability projection、输出结构化结果和恢复提示 | 直接访问 Core store、另写 command policy |
| Panel | 与 TUI 相同的 typed state/event projection | 复刻 herdr/Orca server-side layout/PTY logic |
| LLM Tool | descriptor-backed action/resource projection、模型安全说明、结构化 refusal | 通过字符串 action 绕过 capability gate |
| Background continuation | 只调用授权且有 durable intent 的 capability；遵循 ownership/cancel/recovery | 静默键入、自动选择恢复策略、伪造 operator approval |

当前 TUI `interfaces/tui` 继续独立于 `alephcore`，只依赖 `aleph-client` + `aleph-protocol`。TUI-first 意味着 coverage、交互质量和验证优先，不意味着把业务逻辑移入 TUI。

## 7. herdr / Orca / Paseo parity mapping

| Parity area | Aleph mapping | 明确取舍 |
|---|---|---|
| herdr agents/sessions | Terminal session + RuntimeAgent descriptors and list/status/wait | 不复制 herdr registry；ownership 由 Aleph policy 统一 |
| herdr prompt/send-keys/start | 独立 write/lifecycle descriptors，严格 terminal policy | 不提供默认 unrestricted write；不把 prompt 当 shell exec |
| herdr panes/layouts/workspaces | Core WorkspaceStore + logical BSP + typed projections | 不复制 herdr/Orca UI component tree；不存像素 |
| herdr hooks | Hooks capability + explicit consent + audit | 不隐式修改用户配置；不让 hook 成为隐藏 write path |
| herdr handoff/recovery | durable intent + process journal + reconcile/classify | 不声称 portable-pty fd 可跨平台透明恢复 |
| Orca terminal stream/input/runtime | existing PTY screen patch + RuntimeAgent event source + guarded write | 不引入第二 WebSocket/VT stack；沿用 gateway transport |
| Paseo snapshot/restore/stream/input | typed snapshot/restore/stream/input DTO and revision checks | restore 不是无条件 replay；必须由 policy/owner/descriptor 证明 |
| Paseo workspace recovery/worktree | WorkspaceStore references existing sandbox/worktree owner | 不允许 terminal handler 任意创建/切换 workspace |

Parity 的“完成”按 capability contract 计，不按 herdr 的文件/端点数量计：每项必须有 canonical owner、descriptor/typed schema、授权、lifecycle、projection、recovery classification、测试和文档入口。

## 8. 分阶段实施边界 / Staged implementation boundary

实施计划应把全量 parity 拆成可编译、可回滚的垂直切片；以下顺序是设计约束，不是已授权的文件级计划：

### Phase 0 — Census and seam hardening

- 清点现有 `pty.*`、`runtime.*`、`terminal` tool、`hooks.*`、workspace/project/worktree 和 TUI commands 的重复事实；
- 明确 canonical owner matrix 与 compatibility face；
- 搭好 approval verdict 传递 seam；
- 验证 `ToolHandlerRegistry`/descriptor/projection 机制对 terminal verbs 的适用边界；
- 不改变用户可见写入默认值。

### Phase 1 — Read-only canonical convergence

- 把 list/read/status/wait/explain 和 screen/runtime event projection 接到 canonical descriptors/resources；
- TUI session picker、attach/reconnect、agent panel 使用同一 typed contract；
- 保持旧 `pty.*` 和 `terminal` tool 的兼容 projection，删除重复状态推理；
- 加入 projection-hole、wire-key equality、seq-gap/re-attach 和 ownership matrix tests。

### Phase 2 — TUI workspace and tabs/BSP

- Core 维护 logical workspace/tab/pane tree、active pane 和 session binding；
- TUI 只做 navigation、drag-to-ratio、render；Panel/CLI 消费同一 projection；
- layout revision 和 optimistic conflict/refusal 要结构化表达；
- worktree 只引用既有 sandbox/worktree capability，不另造路径治理。

### Phase 3 — Guarded write and lifecycle

- 先 `send_keys` 与 `prompt`，再 `start/spawn`、`close`；
- 每个 verb 独立 descriptor、scope、approval tier、audit/outcome；
- bracketed paste + delayed Enter、foreground confirmation、agent identity/status 和 owner admission 在 Core 完成；
- TUI 发送 typed command，禁止本地 PTY write；
- 写入效果未知时保持 unknown，不自动重复。

### Phase 4 — Hooks and recovery

- lifecycle-capable agent hooks 的 install/enable/disable/status/repair；
- workspace/session durable intent、process journal reconcile、restart handoff、recreate/abandon operator path；
- 只有当前 descriptor、stored identity、effective input、policy 和 one-shot claim 全部满足时，才另行申请 Safe replay 设计；本 spec 不授权自动 replay。

### Phase 5 — Parity closure and cleanup

- 用 herdr/Orca/Paseo acceptance matrix 逐项验证；
- 删除已被 canonical projection 取代的死代码、旧 action maps、重复 wire DTO 和 unreachable handlers；
- 更新 `FEATURE_LOCATOR.md`、`TERMINAL_RUNTIME.md`、`TOOL_SYSTEM.md`、`ARCHITECTURE.md` 和必要 bilingual docs；
- graphify update 后复核新增连接、孤立节点和跨边界依赖。

## 9. 错误、授权与并发契约 / Failure, authorization, concurrency

1. **Visibility is not authorization.** 能列出 session 不等于能写入；每个 addressed verb 重新执行 owner admission 和 policy gate。
2. **Approval is a verdict, not a side effect.** approval card 的展示、用户批准、descriptor policy 和实际 handler dispatch 必须是可追踪的同一 call identity；缺 verdict fail-closed。
3. **Unknown is not false/success.** PTY exit race、server restart、lost fd、descriptor replacement、event lag、hook reload 未知都返回 typed unknown/refusal，并给出下一步。
4. **No lock across external await.** registry/store lock 在捕获 snapshot 后释放；PTY reader/flush loop 不被 TUI/RPC await 阻塞。
5. **Single clock.** 16ms flush loop 是 screen publishing 唯一节拍；agent probe 使用已有独立最小间隔，不由 TUI tick 派生。
6. **Cancellation.** session/workspace close 应先阻止新 commands，再等待/观察 owned operations 的已定义终止状态；不得把“发出 cancel signal”报告成“子进程已退出”。
7. **Generation safety.** hot reload/replace 后旧 handle 不能删除新 generation；in-flight old handler 可完成，但新 resolve 只能得到新 generation。
8. **No model-side hidden recovery.** harness 只传递已解析 identity/结果；模型决定 retry/resume/abandon，harness 不替模型挑恢复策略。

## 10. 验证契约 / Verification contract

### 10.1 Unit and contract tests

- descriptor registration、same-generation handler binding、replace/unregister/dispose；
- ownership predicate 在 list、addressed RPC、tool、event delivery 五个面一致；
- `PtyScreenFrame` seq gap 强制 attach；attach snapshot 原子包含 geometry/cursor/modes/cwd；
- RuntimeAgent 状态只由 Core detector 产生，`quiet_since` 不会产生 Idle；cwd precedence；probe interval；
- write preflight：off/ask/on、identity、owner、agent detection、foreground、Blocked、empty input、sandbox policy、approval verdict；
- prompt bracketed paste/delayed Enter 与 send_keys separation；
- workspace BSP normalization、layout revision conflict、pane/session binding；
- hooks consent/audit/partial failure/unknown reload；
- durable intent/outcome pairing、legacy event decode、descriptor mismatch、lost restart、repeated recovery fail-closed；
- TUI projection tests：refused vs unavailable、reconnect resubscription、screen patch apply、no core dependency。

### 10.2 Integration and QA

按 `docs/reference/SUBSYSTEM_ROUTING.md` 与 `qa/terminal/run.sh` 执行：

```text
qa/terminal/run.sh identify
qa/terminal/run.sh wait
qa/terminal/run.sh quiet
qa/terminal/run.sh cwd
qa/terminal/run.sh real
qa/terminal/run.sh tui
```

新增 acceptance 必须覆盖：

- 从 TUI 创建/attach tab，读取 screen，检测 agent，切换 pane；
- server reconnect 后自动重新订阅并通过 attach 修复漏帧；
- 未识别 agent、非 owner、无 approval、sandbox refusal、旧 generation 写入均拒绝；
- approved `send_keys`/`prompt` 的效果可在 screen/runtime event 中观察，不能只验证 handler 被调用；
- close/restart/reconcile 后不会把 lost process 显示为 completed；
- hooks/worktree 只修改明确授权的配置/路径；
- Panel/CLI/TUI 对同一 session/layout/runtime snapshot 显示同一事实。

### 10.3 Resource discipline

执行 cargo 前必须遵守用户约束：第二行检查 `MemAvailable`，低于 4G 时等待；批量完成 2–3 个 implementation tasks 后再 compile；Windows checkout 使用 `git -c core.autocrlf=true` 做 porcelain 检查。实现必须在新 worktree/非 `main` 分支，完成后经过 review/integration，再提交。

## 11. 文档和 feature locator 更新 / Documentation updates

实现完成时至少更新：

- `docs/reference/FEATURE_LOCATOR.md`：新增 terminal capability canonical owner、TUI-first、workspace/BSP、write verbs、hooks、recovery 和 QA 入口；保留旧路径的 compatibility 标记；
- `docs/reference/TERMINAL_RUNTIME.md`：补充 capability ownership、descriptor/projection、write authorization、workspace/recovery 生命周期；
- `docs/reference/TOOL_SYSTEM.md`：说明 terminal Tool 是 descriptor-backed projection，不能自建 action registry；
- `docs/reference/ARCHITECTURE.md`：记录 TerminalRuntime 最小内部 service 与 R1/R4/R8/R10 边界；
- 需要时同步 `docs/reference/SUBSYSTEM_ROUTING.md`、`docs/reference/HARNESS_PHILOSOPHY.md` 和双语 release/QA 文档。

文档必须区分：已完成 capability、兼容 projection、仅 classification 的 recovery、尚未授权的 Safe replay，以及平台无法提供的真实 FD handoff；不得用“fully recovered”掩盖 recreate/unknown。

## 12. 非目标 / Non-goals

- 不复制 herdr 的 UI、VT/libghostty、registry 结构或默认不受限写入；
- 不把 `TerminalRuntime` 变成新的 God object、RPC router、Tool registry 或 scheduler；
- 不将复杂业务 UI 移入 TUI；TUI 仍是纯 I/O thin client；
- 不在 `src/harness/` 添加 terminal policy、recovery decision、platform API 或 scheduler；
- 不引入第二套 durable store、external-effect ledger 或 exactly-once 保证；
- 不把 `idempotent`、`quiet_since`、`handler called` 或 `approval card shown` 当成效果到达证明；
- 不默认打开 agent terminal write；prompt/send-keys/start/close/hooks 不能绕过各自治理。人类原始输入只允许 §2.3 的显式、不可委托例外，不能宣称它具有逐命令 command policy；
- 不在本 spec 内授权 Safe replay 的实际执行；那需要独立 crash-window/claim/idempotency 设计与批准；
- 不声称所有平台都能恢复原 PTY FD；不能恢复时必须诚实报告并提供 operator/model 可选择的重建路径。

## 13. 实现计划必须明确的决策 / Decisions the implementation plans must settle

1. `terminal.sessions.*` 的 canonical descriptor 命名是否应作为唯一新命名空间，还是应保留一层 `pty.*`/`terminal` compatibility alias？如何保证 method census 和 ToolCatalog 不产生两个 identity？
2. `TerminalSessionStore` 与现有 process journal/worktree owner 的最小接口是否足够支持 Windows ConPTY、Unix PTY 和重启后的 reconcile，而不承诺不可移植的 FD handoff？
3. workspace/layout 是否应先只支持单 workspace + logical BSP，再开放 herdr 的多 workspace API，还是 parity acceptance 要求第一阶段就支持多 workspace？
4. `start` 的语义是“启动一个已配置 agent session”还是“创建 PTY 并执行启动命令”？两者是否必须拆成不同 descriptor，以避免把 agent lifecycle 与 shell spawn 混淆？
5. hooks 的第一批 lifecycle-capable agents 清单和显式 consent UX 应由哪一份现有 agent manifest/config contract 定义？
6. `ScopedToolService` approval verdict seam 的最小类型和传播范围是什么，才能修复 terminal 写入而不改变其他 operator tools 的既有安全语义？
7. recovery 的第一期验收是否只要求 durable intent + reconcile/classification + operator-driven recreate，还是必须同时交付真实的跨重启 process handoff？后者受平台能力影响，应避免把无法证明的恢复写进 acceptance。
8. 是否批准按 §8 的五个 phase 进入 writing-plans？

## 14. 决策记录 / Decision record

截至本文生成时已确认：

- full herdr parity：批准为范围目标；
- strict Aleph write policy：批准；
- architecture：A + B（能力族垂直收敛 + 最小内部 service）；
- human interaction：已批准 §2.3 的独立、不可委托交互授权例外；
- implementation：原 spec 已批准进入 writing-plans；必须再审阅计划，才能修改产品代码；
- current repository：`main` clean at `a5300221d`；隔离目录为 `D:/Workspace/Aleph/.worktrees/terminal-capability-parity`，分支 `feat/terminal-capability-parity`；
- current terminal read-only behavior and existing wire types remain compatibility inputs, not permission to create a second canonical registry。
