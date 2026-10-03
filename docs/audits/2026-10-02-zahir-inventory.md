# Zahir 清点（§十四）· 2026-10-02

- 基线：`main @ 1e334f92`（已含 `plugin-scope-round` 合并 69f272bf7 / c0ab9c5d6）
- 来源：Explore 子代理逐项 grep + `git blame -L n,n --porcelain`（hash 取前 8 位）；主会话对关键断言抽查复核（见末节）
- 总纲：`Everything-externally-composable-is-a-Capability.md` v3
- 拟定的 replay 策略是根据代码行为推断出来的，代码里目前没有这类声明

## 结论

1. **规模：这是一次“升一级”，不是整理。** 按名字存放可调用 / 可订阅东西的注册表约 **35 个**，其中 6 个没有撤销路径（ProviderRegistry、ProtocolRegistry、AgentRegistry、GenerationProviderRegistry、SearchRegistry、CardRegistry）。
2. **效果三明治的前两片已经有了。** 派发前写 `ToolCallRequested`，重启后 `boundary_repair` 给悬空调用补 `ToolError`。所以被杀的工具今天统一按“报告中断”处理：不重跑，也不会静默丢失。
3. **replay 声明有现成的种子。** `ToolDefinitionMetadata.idempotent: bool`（`src/tools/service.rs:188`）的语义就是“上次可能已到达服务端，重跑同样输入仍安全”。目前只有 `tools::retry` 在消费它，`boundary_repair` 还没读。
4. **缺口：**
   - 恢复时没有用到逐工具的 replay 信息；
   - 提交幂等不跨重启；
   - 取消没有“父等子”；
   - 没有对外的 MCP server 面；
   - ACP 的 `request_permission` 没有接入审批门；
   - 六类 EffectScope 之外的资源没有 owner。

## 清点 1：注册表

| 结构 | 位置 | 键 | 撤销路径 | commit |
|---|---|---|---|---|
| ToolCatalog（AI 可见目录） | src/tool_metadata/registry/mod.rs:62 | String | state.rs:39 `remove_by_mcp_server`、:62 `remove_skills` | 0ebd4268 |
| ToolHandlerRegistry（名字 → 执行体，ArcSwap + broadcast） | src/tools/registry.rs:18 | String | `unregister` :72，在 src/tools/handlers/registration.rs:193 调用 | 0ebd4268 |
| LoopToolRegistry | src/tools/runtime.rs:217 | String | 未见 | e66054ec |
| BuiltinToolRegistry（静态字段） | src/executor/builtin_registry/registry/struct_def.rs:28 | 字段 | 无 | bc721b8e |
| HandlerRegistry（JSON-RPC，137 个方法） | src/gateway/handlers/mod.rs:310 | String | `unregister` :1210 | bb5b6ac3 |
| PluginRegistry | src/extension/registry/plugin_registry/mod.rs:24 | String | `unregister`，在 src/extension/loader.rs:367 调用 | d147492b |
| SkillRegistry | src/skill/registry.rs:13 | SkillId | `remove` :92 | f6cc0290 |
| ProviderRegistry | src/providers/registry.rs:35 | String（DashMap） | **无** | a7eb4b49 |
| ProtocolRegistry | src/providers/protocols/registry.rs:25 | String | **无** | f111d49e |
| GenerationProviderRegistry | src/generation/registry.rs:43 | String | **无** | 01d16ced |
| SearchRegistry / ProviderFactoryRegistry | src/search/registry.rs:79；src/search/factory.rs:71 | String | **无** | 26ac5f22 / 8297b1fc |
| AcpAdapterManager | src/acp/manager/mod.rs:74 | String | lifecycle.rs 关闭会话 | 987da87a |
| McpManagerActor.clients | src/mcp/manager/actor.rs:69 | String | src/mcp/manager/handle.rs:319 `shutdown` | c1370308 |
| AgentRegistry | src/agents/registry.rs:74 | String | **无** | 6a145014 |
| CardRegistry（A2A） | src/a2a/service/card_registry.rs:12 | Vec | **无** | 8789bb34 |
| NodeRegistry / CommandTable | src/cluster/registry.rs:135；src/cluster/node_runtime.rs:32 | — / String | 未见 | 79a106d8 / b7936438 |
| MemoryExtensionRegistry | src/memory/extensions/registry.rs:46 | Vec | EffectScope dispose | d01ac7e1 |
| FlushRegistry | src/memory/flush/registry.rs:14 | String | 未见 | c0cd2b36 |
| FlowRegistry | src/orchestrator/flow_registry.rs:12 | ArcSwap<FlowSet> | 整体替换 | 0a79beeb |
| ChannelRegistry | src/gateway/channel_registry.rs:143 | ChannelId / String | 未见 | 71b4cbac |
| LinkManager / WebhookMountTable | src/gateway/link/manager.rs:166；src/gateway/webhook_receiver.rs:146 | BridgeId / 路径 | — | f113d384 / 2fb4a052 |
| Discord CommandRegistry | src/gateway/interfaces/discord/commands.rs:297 | 枚举 | 无 | c06dd289 |
| SessionRunRegistry | src/gateway/execution_engine/session_run_registry.rs:20 | session → run | run 结束 | 5cc14603 |
| AgentRunManager.active_runs | src/gateway/handlers/agent.rs:173 | run id | run 结束 | 24201e55 |
| RequestStateRegistry / StreamRegistry | src/gateway/middleware/request_state.rs:222；src/gateway/voice/streaming/relay.rs:56 | Uuid / — | 请求结束 | 014f66e0 / 8631331f |
| 浏览器：Engine / Tab / Profile / Route / Recording | src/browser/engine/registry.rs:18；tab_registry.rs:75；manager.rs:120；cdp_backend/routes.rs:155；cdp_backend/recording.rs:392 | 多种 | engine `shutdown` src/browser/engine/mod.rs:628 | 30f39b07 / 2cecf864 / 3a80c875 / d0a7189e / dad6933f |
| ProcessRegistry（后台进程） | src/builtin_tools/process_registry.rs:194 | u64 | 进程退出 | eff6df43 |
| ExecApprovalManager.pending | src/exec/manager.rs:315 | approval id | resolve | 950fa0d6 |
| ClarificationManager.pending | src/clarification/session.rs:255 | String | RetireOnAbandon src/clarification/ask.rs:205 | 6779fa1b |
| 其他：Loop / Template / Persona / Install（落盘 manifest）/ MultiProvider / PromptSize / Lane / EventVisibilityIndex / PresetCatalog | src/looping/mod.rs:84；src/teams/templates/loader.rs:67；src/group_chat/persona.rs:14；src/bundled/manifest.rs:19；src/thinker/mod.rs:203；src/thinker/prompt_size_registry.rs:137；src/gateway/lane.rs:370；src/gateway/event_visibility.rs:798；interfaces/webchat/src/preset_providers.rs:89 | 多为 String | 多数未见 | 见代理原始输出 |

不存在独立的 slash command 注册表。`slash_command` 只作为 EffectScope 的一个步骤标签出现。

## 清点 2：多面动词

| 能力 | 面 | 各面是否同源 |
|---|---|---|
| 内置工具 | 工具面；RPC `tools.invoke` / `tools.catalog` / `tools.effective`（src/gateway/handlers/mod.rs:1015-1021） | 同源（ToolCatalog + ToolHandlerRegistry） |
| 浏览器 navigate / click | 只有工具面（src/builtin_tools/browser_tools），没有 `browser.*` RPC | 单面 |
| MCP 外部工具 | 工具面（src/tools/handlers/mcp.rs）；RPC `mcp.respond_approval` / `mcp.cancel_approval` | annotations 统一映射到 `ToolDefinitionMetadata` |
| ACP harness | 工具面 `AcpDelegateTool`（src/builtin_tools/acp_tools.rs:65）；团队调度 `acp:<harness>`（src/teams/dispatcher/acp_bridge.rs:12）；`team_members.acp_harness_id`（src/teams/store.rs:208） | **各面独立定义**，只共享 AcpAdapterManager |
| 子 agent | 工具面；RPC `agents.*`（13 个）、`subagent.*`；事件 SubagentSpawned / Returned | 同源（AgentRegistry + BackgroundAgentTracker） |
| 审批 | 工具门；RPC `exec.approval.resolve`、`exec.grants.list` / `exec.grant.revoke`；事件 Approved / Denied / Parked | 同源 |
| 中止 | `chat.abort`、`agent.cancel`、TUI `/stop`、CLI | 已合流到 `cancel_session`（src/gateway/handlers/agent.rs:395-405） |
| 技能 / 插件 / hooks | 工具面（skill_manage / plugin_manage / hooks_manage）；RPC `skills.*` / `plugins.*` + `plugin.*` / `hooks.*` | **工具 handler 与 RPC handler 分别实现**，只共享底层 registry |

P0-c 样板候选：ACP harness（三面各自独立定义），或技能 / 插件 / hooks（工具面与 RPC 两份 handler）。

## 清点 3：生命周期

| 资源 | 位置 | 清理 | 经 EffectScope | 泄漏风险 |
|---|---|---|---|---|
| 插件的六类效果 | src/extension/effects/scope.rs:16-21 | `dispose` :91 逆序 | 是 | 低 |
| 插件派生视图 | src/extension/lifecycle.rs `after_transition` | 重算 | 否 | 重算失败会留残影 |
| MCP stdio 子进程 / 外部连接 | src/mcp/transport/stdio.rs:385、:680；src/mcp/external/connection.rs:1538 | close + Drop | 插件来源的经 McpScope（src/extension/registrar/mcp_registrar.rs:298），配置来源的不经 | 中 |
| 浏览器引擎 / Chrome MCP | src/browser/engine/mod.rs:628；src/browser/chrome_mcp.rs:217 | shutdown + Drop | 否 | 中（崩溃后可能留下 Chrome 孤儿进程） |
| **后台 shell 进程** | src/builtin_tools/process_registry.rs:194 | 进程退出 | 否 | **高（没有 owner）** |
| 后台子 agent | src/agents/background_tracker.rs `global()` | Drop :1467；TTL 3600s（:17） | 否 | 中（进程级静态持有） |
| git worktree / cgroup | src/sandbox/worktree.rs:87；src/sandbox/cgroup_v2.rs:308 | Drop | 否 | 中（被杀时 Drop 不执行） |
| 会话媒体临时文件 | src/gateway/execution_engine/run_loop/inner.rs:1878；src/media/processor.rs:82 | Drop + 显式清理 | 否 | 中 |
| ACP 会话 / 适配器进程 | src/acp/manager/lifecycle.rs | 显式 | 否 | 中 |
| run 槽位、并发、排队、幂等槽、恢复槽、emitter、steer、watcher、mDNS、监控、澄清、InFlightGuard、实例锁 | 见代理原始输出 | Drop | 否 | 低 |

六类之外应优先纳入 Scope 的资源：后台进程、后台子 agent、浏览器 profile、worktree、cgroup、ACP 会话、配置来源的 MCP server。

## 清点 4：Scope 消费者

| 级别 | 可见性 | 生命周期 |
|---|---|---|
| Runtime | `ScopeKey::Global`（src/extension/visibility.rs:47）；`ScopeId::Org`（src/scope/mod.rs:43，只有词汇，没有生产者） | 进程级单例：McpManagerActor、BackgroundAgentTracker::global()、GLOBAL_RESUME_COORDINATOR（resume_coordinator.rs:53）、InstanceLock |
| Session | `ScopeId::Personal` / `Project`（task-local `current_scope`）；EventVisibilityIndex | 会话日志、`GrantScope::Session`（src/sandbox/exec_approval/grants.rs:76）、DenialLedger、SessionRunRegistry、`cancel_session`、`media::cleanup` |
| Run | 无 | engine `cancel(run_id)`（src/gateway/execution_engine/engine.rs:523）、RunSlot、RunStarted / RunFinished |
| Task | 无 | `adjudicate_orphaned_tasks`；子 agent CancelGuard（src/agents/subagent_tool/spawn.rs:559）。没有 Scope 类型 |
| Turn | 无 | 只有 `TurnStarted` 事件（src/session/events.rs:415），没有挂在 Turn 上的清理，**不建** |

另外注意：`src/scope/mod.rs::ScopeId` 是第三套 “scope” 词汇（数据隔离），和 `ScopeKey`（插件可见性）、`EffectScope`（所有权）并存。spec 需要给三者划清边界并统一命名。

## 清点 5：双向协议面

| 方向 | 面 | 现状 |
|---|---|---|
| 出口 | **MCP server** | **不存在**。代码里所有 `tools/list` 都是 client 侧；`plugin-scope-round` 的 “MCP 面” 指的是插件声明的 MCP server，由 Aleph 作为 client 加载 |
| 出口 | JSON-RPC Gateway | 137 个方法，没有 browser 或 acp 方法 |
| 出口 | CLI | `al` / `aleph` / `aleph-tui`，都走 Gateway RPC |
| 入口 | MCP client | src/mcp/external/connection.rs：initialize :584、tools/list :710、tools/call :1233 |
| 入口 | ACP client | src/acp/protocol.rs：`session/new` :63、`session/prompt` :79 |
| 类型真源 | MCP / ACP | MCP 是手写 JSON-RPC（src/mcp/jsonrpc.rs、src/mcp/modern/）；ACP 只有 src/acp/protocol.rs 一份 |
| 审批 | ACP `session/request_permission` | src/acp/incoming.rs `permission_allows` 走纯策略（ApproveAll / DenyAll / ApproveReads），**不接入 ExecApprovalManager，不写 Approved / Denied 事件，没有人参与** |
| 审批 | MCP | `destructiveHint` 映射为 `requires_approval`（src/tools/adapters/mcp_adapter.rs:15），进入审批门 |

## 清点 6：持久性

### 6a. 工具 replay

今天对所有工具的处理都一样：`ToolCallRequested`（src/harness/agent/act.rs:484，ef2e9983）在派发前落盘；重启后 `boundary_repair`（src/session/boundary_repair.rs:340，8f243f3a）补一条 `ToolError`。默认措辞是 “may have completed”；对被拒或停在审批的调用，措辞是 “did not run … no file writes …”。是否重试交给模型判断。

现有元数据：`requires_approval`（src/tools/service.rs:177）、`idempotent`（:188，目前只被 `tools::retry` 消费）、`concurrent_safe`（:212）、`requires_confirmation`（src/tools/traits.rs:104）。全仓库只有 4 个文件设置了 `idempotent: true`。

| 工具 | 副作用 | 拟定 |
|---|---|---|
| file read / grep / search（src/builtin_tools/file_ops/read.rs:136 等） | 只读 | Safe |
| file write / file ops | 整份覆写 | Keyed 或 Unsafe，交 spec 定 |
| bash_exec（src/builtin_tools/bash_exec.rs:134） | 任意 | Unsafe |
| web_fetch（src/builtin_tools/web_fetch/mod.rs:475） | 以 GET 为主 | Safe |
| browser_tools、AcpDelegateTool、channel_message、media_send、cron_manage、skill_install、plugin_manage | 外发 / 改状态 | Unsafe |
| 子 agent | 递归 | 已有 Spawned / Returned 配对，交 spec 定 |
| MCP 工具 | 看 annotations | readOnlyHint 为 Safe，idempotentHint 为 Keyed 候选，其余 Unsafe |

### 6b. 意图戳

| 开始 | 结束 | 位置 |
|---|---|---|
| RunStarted | RunFinished | src/session/events.rs:376 / :396；src/session/marker_balance.rs:35 负责收口 |
| ResumeAttempted | 下一个 RunFinished | events.rs:404 |
| TurnStarted | 无 | events.rs:415 |
| ToolCallRequested | ToolResult / ToolError | events.rs:518 / :548 |
| ToolCallParked | Approved / Denied | events.rs:543；写入 src/tools/scoped/dispatch.rs:1271，经 src/session/call_log.rs:45（尽力而为，返回 `()`） |
| SubagentSpawned | SubagentReturned | events.rs:561 / :567 |
| MCP ServerStarted | — | src/mcp/manager/types.rs:660，只在进程内总线，不进会话日志 |
| 浏览器会话 | — | 没有标记 |

矛盾检测：`LogContradiction`、`DanglingProvenance`（src/session/reduction.rs:265）。

### 6c. 提交幂等

`IdempotencyGuard`（src/gateway/idempotency.rs:104）只在内存中，带 TTL；`gateway.require_idempotency_key` 默认 false（src/gateway/config.rs:265）。没有会话级的 requestId。

### 6d. 审批 / hook 决策

- 已完成的审批结果作为 Approved / Denied 进入会话日志。
- 等待中的审批（`ExecApprovalManager.pending`）只在内存，重启后由 boundary_repair 收口为 “did not run”，不会重新弹窗。
- `GrantScope::Always` 落盘到 `~/.aleph/approval-grants.json`；`GrantScope::Session` 和 DenialLedger 只在内存。
- hook 和 compaction 阈值没有找到落盘点。

**判断：D5（durable memo）在审批路径上已基本满足，不需要单独立项。**

### 6e. 取消传播

- `cancel_session` 会遍历子 run，但只发信号：src/gateway/execution_engine/engine.rs:643 `let _ = self.cancel(&child).await;`，不等子 run 结束。
- 没有找到“父等子”的 join。
- 前台 / 后台：子 agent 有 `run_in_background`（src/agents/subagent_tool/parse.rs:288），`synthesize` 要求前台（:422）。
- **未核实**：`chat.abort` 是否会连带取消后台子 agent（`running_runs_of_session` 的范围）。

## 复核记录（主会话抽查，`1e334f92`）

| 断言 | 结果 |
|---|---|
| 没有出口 MCP server | 确认。`tools/list` 只出现在 client 侧和测试中 |
| ACP permission 不接审批门 | 确认。`permission_allows` 走 `PermissionPolicy` 三选一；src/acp 里没有 `ExecApprovalManager` |
| `ToolCallRequested` 在派发前写入 | 确认（act.rs:484 附近） |
| 子取消只发信号不等待 | 确认（engine.rs:643） |
| `idempotent` 字段语义 | 确认（service.rs:180-188 注释） |
| `require_idempotency_key` 默认 false | 确认 |
| `approval-grants.json` 落盘 | 确认 |

**未复核**：其余注册表的行号和 commit hash，均由子代理给出；浏览器工具没有逐行 blame；`chat.abort` 对后台子 agent 的影响。
