# Capability Phase 3: Everything Externally Composable Is a Capability

- 日期：2026-10-05
- 分支：`phase3-capability-architecture`
- 状态：设计已获批准，待用户审阅书面 spec
- 修订：2026-10-05（收窄范围）——用户批准将本期完整闭环收窄为 MCP + 主 builtin + markdown-skill `AlephToolDyn` callable surface（主 builtin 与 markdown-skill 是两个独立 callable source family）；Plugin 不纳入 canonical `ToolHandlerRegistry`，保留 `ToolCatalog`/extension manager 旁路，另列 ExtensionHandler spec 为后续事项。修订理由：Task 1 源码 census 证实 Plugin 无 `ToolHandler` 实现（2026-05-20 已移除），不得从 metadata 伪造 handler。
- 修订：2026-10-05（Task 2 架构缺口修订）——Task 2 在实现时暴露两处架构缺口：(a) 主 builtin 的 handler 无法在进程级直接注册进 canonical registry，因为其执行上下文是 per-request late-bound 的，且 per-request allowlist 决定可见集；(b) markdown-skill 经 `static Lazy<AlephToolServer>` 维护，无 generation counter、无 owner scope、无 canonical registry 路径。本修订规定精确的分发数据流、owner/scoping 语义、替换行为、请求 allowlist 投影与迁移顺序（见 §5、§6、§9、§13）。
- 父级设计：`docs/superpowers/specs/2026-10-03-capability-tool-descriptor-design.md`
- 相关设计：`docs/superpowers/specs/2026-10-04-capability-phase2-durable-tool-design.md`、`docs/superpowers/specs/2026-10-04-capability-phase2b-safe-replay-design.md`
- 外部参考：Pi Durable 设计、`Everything-externally-composable-is-a-Capability.md`、`pi-durable-overview.md`

## 1. 目标与问题重述

本期要解决的问题不是再增加一个抽象 trait，也不是把所有模块强行改成相同的 registry。Aleph 当前已经有 Tool、MCP、Plugin、Skill、Agent、Runtime、ACP、RPC、CLI 和 Panel 等可动态接入的能力，但它们的发现、调用、替换、释放、恢复和协议投影仍部分由不同结构表达。同一事实可能在 registry、metadata、protocol DTO、recovery 分支和局部缓存中各有一份，导致语义漂移、生命周期泄漏、替换代际混淆，以及“调用发生了”被误报成“效果已经到达”。

用户目标的本质是：**任何需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都必须能够被明确建模为 Capability，并通过可验证的身份、生命周期、调用边界和投影契约被组合起来。** 第三期先把 Tool 做成第一个完整样板，再依据真实消费者判断其他能力是否应进入同一模型。

本期采用 Rust Core 作为能力事实源，Bridge/Interface 只负责 I/O 和协议适配；优先连通已有 Tool registry、descriptor、scope、SessionEvent 和 recovery 设施，删除重复路径，修复错误语义，再评估扩展边界。目标是完成一个真实闭环：

```text
动态注册
  -> descriptor + handler 同代绑定
  -> caller-visible resolve
  -> durable intent identity / recovery classification
  -> 可替换且可释放
  -> recovery 使用双重契约证明
  -> MCP/RPC/CLI/Panel 等多面投影
```

### 1.1 成功标准

只有同时满足以下条件，本期才算完成：

1. MCP、主 builtin 与 markdown-skill `AlephToolDyn` callable surface 至少两类真实 Tool 经过同一 `ToolHandlerRegistry` resolve。主 builtin 以进程级 thin router handler（over `ToolRegistry::execute_tool`）注册进 canonical registry；markdown-skill 以 install/hot-reload 路径注册进同一 canonical registry，持有真实 owner lifecycle。Plugin 不在本期 canonical registry 范围，保留现有 `ToolCatalog`/extension manager 旁路作为明确兼容边界。
2. descriptor 是 Tool identity、schema、revision、replay 和安全策略的单一语义来源。
3. 至少一个重复的 metadata/registry/旁路 handler 路径被删除，或严格降级为纯执行索引/兼容包装；主 builtin 的 `RegistryToolAdapter` 二次 handler 投影与 `to_metadata_form` 的 revision-1 descriptor 伪造必须被 canonical snapshot 投影取代。如果 Phase 1 已经删除该路径，第三期必须重新 census 并保持其不存在。
4. 至少一个 model/protocol/recovery 多面投影来自同一 descriptor；其中 recovery 面指 classification/lookup，不代表本期实际执行 replay。
5. registration scope 的 dispose 能让能力不可见，并有可观察的清理结果。
6. Phase 2 的 durable identity、current descriptor lookup 和 fail-closed recovery 形成同一条调用链。
7. unknown、replay refused、descriptor mismatch 不会被任一出口转成成功、许可或自动 retry。
8. `src/harness/` 不发生业务扩张；已有事件字段 wiring 仍须遵守 Phase 2 的限定范围。
9. 文档准确区分已完成范围、兼容层和未纳入本期的能力类型。

## 2. 范围与硬边界

### 2.1 本期范围

- 以 `ToolCapabilityDescriptor` 和 `ToolHandlerRegistry` 为核心，收敛 Tool 注册、resolve、替换、移除和投影。
- 统一主 builtin 与 markdown-skill 两个独立 `AlephToolDyn` callable source family（markdown-skill 经 `src/tools/server/`、`src/tools/markdown_skill/`）与 MCP tool 的 descriptor/handler 同代绑定；Plugin 保留现有 catalog/extension manager 旁路，不在本期统一。
- 主 builtin 采用进程级注册 + thin router handler（`ToolRegistry::execute_tool`），run-loop 从单一 canonical snapshot 构建 request-scoped visible projection；不创建第二个 handler map，不创建 test-only / dead-write adapter（见 §5.3）。
- markdown-skill 从 install/hot-reload 直达 canonical registry，具备 generation-safe replace 与真实 owner lifecycle；保留 hot reload，避免 async Drop，使用与 `ToolRegistrationScope`/`EffectScope` 一致的显式 owner disposal（见 §5.4、§6）。
- 统一 registration scope、handle、dispose 和 disposer 错误报告。
- 将 caller visibility、invocation policy、durable intent identity、outcome 关联和 recovery lookup 连接到同一 Tool identity；本期不新增 replay execution outcome 路径。
- 统一 MCP、RPC、CLI、Panel 或其他已有稳定消费者的 descriptor projection helper；至少接通两个真实出口。
- 复用 Phase 2 durable intent/replay 设施，保持 Safe Replay 的既有受限/VerifyOnly 闸门，不扩大为 exactly-once。
- 清理失效的局部 registry、metadata、permission/replay 判定和无消费者的旧 helper。
- 补充 `docs/reference/FEATURE_LOCATOR.md`，并按需更新 `docs/reference/ARCHITECTURE.md`、`docs/reference/TOOL_SYSTEM.md`。

### 2.2 非目标与红线

- 不把 `src/capability/mod.rs` 的进程级 `CapabilitySlot<T>` 直接改成动态 registry；它继续负责启动安装、拒绝和诊断语义。
- 不在 `src/harness/` 增加 registry、replay、权限、生命周期或平台 API 逻辑。Phase 2 已批准的事件字段搬运仍是唯一可能的 wiring 例外。
- 不一次性统一 Skill、Agent、Plugin、MCP、ACP、Resource、Task、Subscription 的全部注册流程。
- 不创建没有真实消费者的新 kind、全局 registry、ownership tree 或 scheduler。
- 不引入重型依赖或第二套持久化存储；不把 Aleph 改造成 Pi Durable 的通用 Task Scheduler。
- 不在 recovery 中按工具名添加特判；策略必须来自 descriptor 和调用时持久化 identity。
- 不保存 Rust handler、闭包、插件代码、凭据或可执行对象到 durable store。
- 不承诺 external effect 与 SQLite 之间的 exactly-once、通用幂等或事务耦合。
- 不将安全字段、replay policy、visibility 或 schema 在 MCP/RPC/CLI/Panel 中各复制一份。
- 不将 Plugin 纳入 canonical `ToolHandlerRegistry`，不从 `UnifiedTool` metadata 伪造 Plugin handler；Plugin 继续走现有 `ToolCatalog`/extension manager 旁路，后续由独立 ExtensionHandler spec 决定是否收敛。
- 不新增 `ToolSource` 变体来区分主 builtin 与 markdown-skill（见 §5.4 取舍）；不新增全局 Capability trait、第二套 durable store、Core 内平台 API 或大型依赖。

## 3. 参考项目映射与取舍

Pi Durable 的可迁移价值在于 effect sandwich、intent checkpoint、descriptor/replay 双重证明、request identity、memos、ownership 和投影单一真源，而不是它的完整调度器。Aleph 采用以下映射：

| 参考模式 | Aleph 映射 | 明确不复制 |
|---|---|---|
| effect sandwich | 现有 SessionEvent/Barrier 的 intent -> handler -> outcome | 通用 Task Scheduler |
| descriptor replay proof | `ToolCallRequested` 的调用时 identity + 当前 `ToolCapabilityDescriptor` | 按 name/idempotent 猜测 replay |
| requestId / call identity | 现有 `call_id`、reduction 和 outcome pairing | 新建 external-effect ledger |
| memo / durable fact | 复用 SessionEventStore、boundary repair、ResumeCoordinator | 统一所有状态到单一存储 |
| ownership tree | 先复用 `EffectScope`/`ToolRegistrationScope` | 在本期引入全局父子任务树 |
| projection as single source | descriptor projection helper | 每个协议自行维护 metadata |
| 存名字不存代码 | 持久化 identity/revision/fingerprint/effective input marker | 持久化 handler 或运行时对象 |

因此，本期的架构决策是：**先完成 Tool Capability 的真实闭环，不制造一个名义统一但内部仍有多份事实源的“大一统 Capability trait”。** 代价是非 Tool 能力仍会暂时保留局部语义；这是有意的分阶段边界，而不是宣称全仓已统一。

## 4. 现有模块映射

| 模块 | 第三期职责 | 边界 |
|---|---|---|
| `src/tools/descriptor.rs` | `ToolCapabilityDescriptor` 的 identity、kind、schema、revision、replay、安全和来源语义 | 不注册、不执行、不调用平台 API |
| `src/tools/registry.rs` | `ToolHandlerRegistry`：Tool 注册、替换、移除、resolve、snapshot、同代 descriptor/handler 绑定 | 唯一 Tool callable truth；不跨 await 持锁 |
| `src/tools/registration_scope.rs` | 现有 Tool registration owner scope、handle、dispose 和报告语义 | 不新增同名 scope；不隐式拥有外部 invocation |
| `src/extension/effects/scope.rs` | 复用现有 EffectScope 所有权/清理契约 | 不复制第二套 disposer 协议 |
| `src/tools/handlers/builtin.rs` | 新增 `BuiltinRegistryRouter`：主 builtin 的进程级 thin router handler，over `Arc<dyn ToolRegistry>`；保留 `BuiltinHandler`（over `Arc<dyn AlephToolDyn>`）用于 markdown-skill 与既有 capability builtin | 不捕获 per-request late-bound context |
| `src/tools/server/mod.rs` + `src/tools/markdown_skill/` | `AlephToolServer` 保留为 tool store；新增 `MarkdownSkillRegistryOwner` 持有 canonical registry 写入 + `ToolRegistrationScope` | install/hot-reload 必须写 canonical registry；不绕过 registry 直接写旁路表 |
| `src/executor/tool_registry.rs` + `src/executor/builtin_registry/` | `ToolRegistry::execute_tool` 仍是主 builtin 的唯一执行面；`BuiltinToolRegistry` 继续持有 late-bound handles（workspace/session/gateway OnceCell） | 不再被 run-loop 二次投影为 `RegistryToolAdapter` |
| `src/tools/runtime.rs`（`LoopToolRegistry`） | 变成 request-scoped visible projection：只从 canonical snapshot 构建，不再拥有全局发现事实 | 不保存第二份 handler map |
| `src/tools/adapters/registry_adapter.rs` | `RegistryToolAdapter` 从主 builtin 投影路径移除/降级为兼容；`McpRegistryTool::from_registry_entry`（handler + descriptor -> `LoopTool`）推广为 canonical handler->LoopTool adapter | 不伪造 revision-1 descriptor |
| `src/tools/service.rs` / `src/tools/scoped/mod.rs` | `ScopedToolService::metadata_schema` uses the direct descriptor-backed field projection; `to_metadata_form` remains only as a compatibility wrapper until the existing harness test import is migrated | Do not maintain a second canonical identity/replay/visibility source |
| 主 builtin / markdown-skill（`src/tools/server/`、`src/tools/markdown_skill/`）/ MCP adapter | 注册 descriptor + handler，并持有 scope/handle | 不绕过 registry 直接写旁路表 |
| extension / Plugin adapter | 保留现有 `ToolCatalog` 行与 extension manager `call_plugin_tool` 旁路 | 本期不注册进 `ToolHandlerRegistry`，不从 metadata 伪造 handler |
| MCP/RPC/CLI/Panel projection | 把 descriptor 映射为协议或展示形状 | 不重新判断 replay、权限或成功语义 |
| `src/session/events.rs`、`reduction.rs`、`replay.rs`、`boundary_repair.rs` | 保存/读取 durable identity，执行 fail-closed recovery classification；`replay.rs` 继续持有 Phase 2B 的私有 permit/adapter 边界，但本期不放宽生产 VerifyOnly | 不引入第二个 registry 或调度器 |
| `src/harness/agent/act.rs` | 仅保留 Phase 2 已批准的 serial/parallel `ToolCallRequested` identity 字段搬运 | 不增加 replay、claim、permission 或 handler 执行逻辑 |
| `src/gateway/resume_coordinator.rs` | 沿用唯一 recovery coordinator | 不从协议层直接 dispatch handler |
| `src/harness/deps.rs` | 仅保留既有 lookup wiring seam | 不增加 recovery、registry、policy 决策 |

第一步实现前必须重新核对当前源码签名；本表是职责契约，不授权新增同名模块。

## 5. 注册与调用数据流

### 5.1 注册总览

```text
主 builtin（进程级 boot）
  -> BuiltinRegistryRouter(Arc<dyn ToolRegistry>)   ─┐
markdown-skill（install / hot-reload）              ├─> ToolHandlerRegistry
  -> BuiltinHandler(MarkdownCliTool) + owner scope  ─┤    (descriptor + handler 同代绑定,
MCP adapter（既有）                                   │     RegistrationHandle,
  -> McpHandler + ToolRegistrationScope            ─┘     snapshot/change feed)
```

Plugin / Extension 不进入上图：保留 `ToolCatalog` / extension manager `call_plugin_tool` 旁路，不从 `UnifiedTool` metadata 伪造 handler。

注册必须验证 name、schema、revision、descriptor/handler pairing 和 scope owner。失败时不得返回一个看似成功的半成品 handle。旧 API 若暂时保留，只能单向转发：

```text
legacy API -> canonical Tool Capability seam
```

不能允许新代码反向依赖 legacy API。

### 5.2 调用

```text
Agent / RPC / MCP / CLI caller
        │
        ▼
resolve(identity, caller context)
        │
        ├── visibility
        ├── invocation policy / approval
        ├── descriptor snapshot
        ├── captured handler
        └── call id
        ▼
commit intent -> execute captured handler -> commit outcome
        ▼
Tool result / structured error / event projection
```

resolve 必须先取得同一代 descriptor + handler snapshot，再执行调用策略和 handler。一次 invocation 捕获的 handler 不因 registry 替换而切换；替换只影响下一次 resolve。registry 锁不得跨越 handler 的异步执行。

intent/outcome 继续复用 Phase 2 既有 SessionEventStore、Barrier durability、RunReduction、boundary repair 和 ResumeCoordinator，不创建第二个 journal。调用 identity 至少关联 `call_id`、tool name、descriptor identity/revision、schema/implementation contract、replay policy、effective-input proof/fingerprint 和恢复 claim/permit（若适用）。

### 5.3 主 builtin：进程级 router + request-scoped visible projection

主 builtin 的执行上下文（`BuiltinToolRegistry`）是 late-bound 的：workspace/session/gateway 上下文经 `BuiltinToolRegistry` 内的 `Arc` handle / `OnceCell`（`memory_workspace_handle`、`session_context_handle`、`gateway_context`、`node_registry` 等）在每轮由 execution engine 写入，而不是在请求内重建。因此主 builtin 可以在进程级一次性注册，无需把 late-bound context 捕获进 handler。

数据流（生产）：

1. **进程级注册（boot）**：`start/mod.rs` 在创建 `tool_registry_phase2 = Arc::new(ToolHandlerRegistry::new())`（`src/bin/aleph-server/commands/start/mod.rs:224`）后，对 `BuiltinToolRegistry` 的每个主 builtin 名字：
   - 构造 `BuiltinRegistryRouter { name, inner: Arc<dyn ToolRegistry> }`，其中 `inner` 就是已装配好的 `Arc<BuiltinToolRegistry>`；
   - `BuiltinRegistryRouter::definition()` 由 `inner.get_tool(&name) -> Option<&UnifiedTool>` 投影为 loop-side `ToolDefinition`（复用 `RegistryToolAdapter` 现用的 `UnifiedTool -> ToolDefinition` 字段投影，保证 model-visible schema 逐字节不变）；
   - `descriptor = ToolCapabilityDescriptor::from_definition(&router.definition(), revision)`（`source=Builtin`、`replay_policy=Unsafe`、`idempotent` 来自 `is_idempotent_builtin_name`、`max_duration_ms` 来自 `resolve_tool_budget_ms`）；
   - `tool_registry_phase2.register(descriptor, Arc::new(router))`，并将返回的 `RegistrationHandle` 交给一个进程级 owner scope。
2. **调用（thin router）**：`BuiltinRegistryRouter::invoke(input)` = `inner.execute_tool(&name, input).await`，把 `crate::error::Result<Value>` 映射为 `Result<ToolOutput, ToolError>`。late-bound context 已在 `inner` 的 handle 内，router 无需每请求注入。
3. **request-scoped visible projection（run-loop）**：`run_loop/inner.rs` 不再调用 `build_registry_from_tools(self.tool_registry.clone(), &allowed_tools)`。改为读取**一份** canonical snapshot（`tool_registry_phase2.entries_snapshot()`），对每条 entry（主 builtin、markdown-skill、capability builtin 都是 `source=Builtin`；MCP 是 `source=Mcp`）用统一 allowlist predicate（`agent.is_tool_allowed(name) && slash_skill_scope::admits(...)`，MCP 另加 `mcp_handler_admitted` 的 face ⑤）过滤，再用现有 `McpRegistryTool::from_registry_entry(handler, descriptor)`（推广为 canonical handler->LoopTool adapter）包装进 `LoopToolRegistry`。

这样 canonical registry 成为唯一 handler store：主 builtin 的执行路径变为 `LoopTool.execute -> BuiltinRegistryRouter.invoke -> ToolRegistry::execute_tool`，router handler 是真实被调用的路径，不是 dead-write；`build_registry_from_tools` / `RegistryToolAdapter` 对主 builtin 的二次 handler 投影被删除，不产生第二份 handler map。

### 5.4 markdown-skill：install/hot-reload -> canonical registry + owner lifecycle

markdown-skill 现在经 `static Lazy<AlephToolServer>`（`src/gateway/handlers/markdown_skills.rs:27` 的 `MARKDOWN_SKILLS_SERVER`）维护，`AlephToolServer` 只有 `ToolMap = Arc<Mutex<HashMap<String, Arc<dyn AlephToolDyn>>>>` 与 `replace_tool`/`list_tools_arc`，没有 generation counter、没有 scope、没有 canonical registry 写入。因此 install（`markdown_skills.rs:392`）与 hot-reload（`start/mod.rs:2702-2704`）都只 `server.replace_tool(tool)`，run-loop 用 `join_markdown_skills` 从 `list_tools_arc` 快照拼接，与 canonical registry 完全无关。

数据流（生产）：

1. **owner 构造（boot）**：`start/mod.rs` 创建 `MarkdownSkillRegistryOwner`，持有 `Arc<ToolHandlerRegistry>`（= `tool_registry_phase2.clone()`）、一个 `ToolRegistrationScope` 和一张 per-name `RegistrationHandle` map。owner 被传入 install handler 与 `SkillWatcher` callback（两处都有 `tool_registry_phase2` 在作用域内；`markdown_skills_server()` 这个 static 不持有 registry，写入发生在 call site）。
2. **install/hot-reload**：对每个 `MarkdownCliTool`：
   - `handler = Arc::new(BuiltinHandler::new(name, Arc::new(tool) as Arc<dyn AlephToolDyn>))`（`BuiltinHandler` over `Arc<dyn AlephToolDyn>`，`src/tools/handlers/builtin.rs`）；
   - `descriptor = ToolCapabilityDescriptor::from_definition(&handler.definition(), revision)`（`source=Builtin`，与主 builtin 同命名空间，见下取舍）；
   - `new_handle = registry.replace(descriptor, handler)`（replace 原子换代，旧 entry 被换下）；
   - owner 把 `new_handle` 存入 per-name map 并 `scope.track(new_handle)`；旧 handle（若有）显式 dispose（换代后已是 stale no-op）。
   - `AlephToolServer::replace_tool(tool)` 仍更新 `ToolMap` 以保留对 markdown CLI 子进程的 store 引用；但 canonical registry 是 identity + 分发的唯一事实源。
3. **removal（watcher 报删除）**：owner 对对应 name 调用 `registry.unregister(handle)`，使后续 resolve 不可见。
4. **shutdown**：owner 经 `ToolRegistrationScope::dispose` 逆序释放全部 handle，报告 `ToolDisposeReport`。不实现 `async Drop`——`MarkdownCliTool` 无 async 资源需清理，替换时旧 `Arc` 引用计数归零即释放；有 `async` 清理需求的路径沿用显式 `dispose`。

取舍（诚实声明）：markdown-skill 与主 builtin 在 canonical registry 中同用 `source=Builtin`（沿用 `BuiltinHandler::definition()` 的既有投影）。本期不新增 `ToolSource::MarkdownSkill { spec_path }` 变体，因为 run-loop 的 allowlist predicate 对两个 family 一致、投影无需按 source 区分，且新增变体会波及 descriptor/metadata/`to_metadata_definition`/`to_unified_tool`。代价是：(a) canonical registry 无法仅凭 `source` 区分两个 family；(b) 同名冲突由 registry 单一命名空间语义裁决。为防用户 skill 静默遮蔽核心 builtin，规定：markdown-skill 的 install/replace 若命中 boot 已注册的核心 builtin 名，必须 fail-closed（结构化 conflict 错误写进 install 响应），不得静默 replace。若未来有消费者需要 source 级 provenance，再独立评估新增变体。

### 5.5 统一 allowlist 与投影

`run_loop/inner.rs` 现按三个来源分别构建 `LoopToolRegistry`：`build_registry_from_tools`（主 builtin，用 `&allowed_tools: Vec<UnifiedTool>`）、`join_mcp_tools`（MCP snapshot）、`join_markdown_skills`（`AlephToolServer` 快照）。三个来源的过滤谓词本已一致（`agent.is_tool_allowed && slash_skill_scope::admits`，注释 `inner.rs:251` 也说明每个 source 重新推导同一谓词）。收敛后三者合一：run-loop 对 canonical snapshot 单遍投影，用同一谓词过滤，MCP 另经 `mcp_handler_admitted`。`build_registry_from_tools` 与 `join_markdown_skills` 删除，`join_mcp_tools` 推广为 canonical join（或等价重命名），主 builtin 与 markdown-skill 的 entry 经 `McpRegistryTool::from_registry_entry` 进入投影。

## 6. 生命周期、替换与释放

### 6.1 Registration 状态

内部语义采用以下状态转换；是否公开 enum 由实现计划结合现有 API 决定：

```text
Absent
  --register--> Active
  --replace---> Active(new generation)
  --unregister/owner dispose--> Retiring
  --entry removed + disposer completed--> Disposed
```

规则：

- 只有 Active entry 可以被新调用 resolve。
- replace 必须先验证新 entry，再以一个同步边界替换，订阅者不能看到半注册状态。
- unregister 先让新 resolve 看不到旧 entry，再执行 disposer。
- Retiring 的旧 handle 不得重新激活旧 registration。
- Disposed handle 再次 dispose 是幂等结果，不重复清理。
- disposer 失败必须进入已有 EffectScope/DisposeReport；不能报告“全部成功”。
- registry 关闭后不接受新注册/新 resolve，并按现有 scope 规则清理。

### 6.2 owner scope 与 markdown-skill 生命周期

`MarkdownSkillRegistryOwner` 是 markdown-skill family 的 owner：持有 `ToolRegistrationScope`（逆序释放、失败报告、幂等）与 per-name handle map（针对 replace 的定点 dispose）。replace 换代时旧 handle dispose 为 stale no-op（generation guard 保证旧 handle 不能移除替换后的 entry——这是 Task 2/4 的 review focus 之一），新 handle 被 track。shutdown 经 `ToolRegistrationScope::dispose` 显式释放，不用 `async Drop`。

主 builtin 的进程级注册由一个 boot 级 owner scope 持有，进程生命周期内不替换（主 builtin 集在启动后稳定；替换/移除仅对 markdown-skill 与 MCP 适用）。

### 6.3 In-flight invocation

registration scope 拥有 registry entry；已经取得 snapshot 的 invocation 由现有 invocation/in-flight cancellation 语义管理。registration dispose 不强杀已开始的外部操作，也不等待一个可能反向等待 dispose 的 handler；旧调用继续使用 captured handler，并在完成或未知时按已有结果/repair 规则记录。

这意味着短时间内可能出现“entry 已不可见但旧调用仍运行”。这是可观测且诚实的状态，避免卸载时强行切断外部效果制造半执行结果。未来若需要父 scope 等待所有 owned invocation，必须另行设计 ownership tree，不能在本期隐式改变 ToolRegistrationScope 语义。

## 7. Replay、恢复与错误契约

### 7.1 判定与执行分层

恢复严格分为：

```text
Replay classification -> authorization / one-shot permit -> execution
```

`ReplayDecision` 不是 bool，不得直接作为许可。Phase 2B 当前生产边界仍是受限/VerifyOnly。本期只实现 classification/descriptor lookup 与现有 durable intent/recovery 的连线，不新增 permit 消费、handler recovery invocation、durable replay claim 或 per-call crash budget。Phase 2B 要求的 `SessionEvent::ToolCallEffectiveInput`（guardrail 后的 effective-input marker）仍是未来允许实际 Safe replay 前的前置契约，不能由本期的 lookup 连线替代。

未来若再次申请放宽 Safe replay，至少需要同时满足保存/当前 schema version、revision 和 replay contract fingerprint 的严格证明、有效 effective input、durable claim、per-call budget、同一不可变 registry snapshot 和一次性 permit；这些是 Phase 2B 的执行闸门，不是本期的实现验收项。

### 7.2 Unknown outcome

未知结果在所有出口保持未知：

```text
unknown outcome
  ├── cannot become success
  ├── cannot become permission granted
  ├── cannot trigger automatic retry
  └── must remain visible to repair/operator/model path
```

MCP、RPC、CLI、Panel 可以改变展示方式，但不能改变判定。不能让 CLI 返回成功码、Panel 显示 completed、MCP 丢失 replay refused 原因，或 recovery 填充空结果继续执行。

### 7.3 Durable intent 与 at-least-once 边界

本期只把 Tool descriptor identity、当前 descriptor lookup 和现有 durable intent/recovery classification 连接起来；不新增 recovery handler invocation，也不改变 Phase 2B 的生产状态：当前仍为 VerifyOnly，不能通过本期代码产生新的外部 effect。

Phase 2B 已批准的未来执行契约仍是：

```text
commit intent -> execute captured handler -> commit outcome
```

该契约的已知上限是 at-least-once，而不是 exactly-once：若未来 Safe replay 获得独立批准，effect 已发生但 outcome 未落盘时可能在 per-call budget 内再次调用，effect invocation count 与 durable receipt count 可以不同。`call_id` 是相关性 identity，不是 handler-level dedup key。第三期不得把这段未来语义展示为当前已执行能力。

在本期，旧事件、缺 descriptor、schema/revision/fingerprint mismatch、Unsafe、blocked/sanitized input 无有效 effective-input proof 和其他不满足 classification 的情况全部 fail-closed，保持现有 unknown/VerifyOnly repair。

## 8. 可见性、安全与可观测性

### 8.1 Discover 与 invoke 分离

```text
registry contains entry != caller may discover != caller may invoke
```

resolve 依次处理 registry identity、caller visibility、invocation policy/approval 和 invocation snapshot。不能用“查不到”代替权限拒绝，因为那会丢失审计事实。内部保留结构化原因：

- `NotRegistered`
- `NotVisible`
- `InvocationDenied`
- `ApprovalRequired`
- `DescriptorMismatch`
- `RegistryClosed`

协议层可以映射错误文本，但不删除原因，也不复制权限 denylist。caller identity 和 transport 信息由 Bridge 注入 Core，Core 决定最终语义。

### 8.2 关联字段

生命周期、日志、SessionEvent、MCP/RPC response 和 Panel DTO 必须能关联：

- capability identity；
- registration generation；
- descriptor revision；
- owner scope；
- invocation/call id；
- caller kind；
- outcome state；
- dispose report；
- replay claim/permit id（若适用）。

优先复用现有 event/session/error facilities，不引入独立 tracing framework 或平台观测服务。不能出现 registry 已卸载但 event 仍显示 active、handler 返回 unknown 但 transport 显示 completed、或替换后新旧调用无法区分 revision 的情况。

## 9. 迁移顺序

### 阶段 A：主 builtin 进程级注册（单一 Tool seam 的第一半）

1. 源码清点 `BuiltinToolRegistry` 的 `get_tool`/`execute_tool` 与 `RegistryToolAdapter` 的 `UnifiedTool -> ToolDefinition` 投影。
2. 新增 `BuiltinRegistryRouter`（`src/tools/handlers/builtin.rs`），实现 `ToolHandler`（`definition()` 由 `inner.get_tool` 投影、`invoke()` 由 `inner.execute_tool` 委托）。
3. `start/mod.rs` 在 `tool_registry_phase2` 创建后注册全部主 builtin（descriptor 由 `from_definition` + `is_idempotent_builtin_name` + `resolve_tool_budget_ms`），handle 交给 boot owner scope。
4. 新增测试：每个主 builtin 可经 `ToolHandlerRegistry::resolve` 取得 handler，且 `invoke` 委托到 `execute_tool`；descriptor 的 schema 与 `RegistryToolAdapter` 时代逐字节一致。

### 阶段 B：markdown-skill canonical 路径与 owner lifecycle

1. 新增 `MarkdownSkillRegistryOwner`，wire 进 install handler 与 `SkillWatcher` callback。
2. install/hot-reload 走 `registry.replace` + owner track/dispose；removal 走 `registry.unregister`；shutdown 走 `ToolRegistrationScope::dispose`。
3. 增加 generation-safe replace 测试、stale handle no-op 测试、核心 builtin 名冲突 fail-closed 测试。
4. 保持 `AlephToolServer` 的 tool store 职责不变（CLI 子进程管理）。

### 阶段 C：run-loop 单快照投影（单一 Tool seam 的第二半）

1. `run_loop/inner.rs` 改为从 canonical snapshot 单遍投影，删除 `build_registry_from_tools` 与 `join_markdown_skills`，推广 `join_mcp_tools`/`McpRegistryTool::from_registry_entry`。
2. 验证 allowlist predicate 对三个 family 一致；MCP face ⑤ 过滤保留。
3. 删除主 builtin 的 `RegistryToolAdapter` 二次投影；确认无第二份 handler map、无 dead-write adapter。
4. 测试：allowlist 收窄后 canonical snapshot 投影与三源时代可见集一致。

### 阶段 D：生命周期与错误收敛

1. 统一 `ToolRegistrationScope`、EffectScope 和 registration handle 的 dispose 语义。
2. 删除重复注册/注销、旁路 handler map 和局部 descriptor cache。
3. 统一 conflict、not visible、closed、mismatch、unknown outcome 结构化错误。
4. 增加 replace、unregister、dispose、in-flight 和并发测试。

### 阶段 E：descriptor 投影迁移（`to_metadata_form` 生产路径退休）

1. `ScopedToolService::metadata_schema`（`src/tools/scoped/mod.rs`）已改用 `ToolDefinition::to_metadata_definition`，不再通过 revision-1 descriptor 伪造 canonical identity。
2. `to_metadata_form` 现在只是兼容 wrapper；`src/harness/tests/tools_surface.rs` 仍有测试导入，因此本期不扩大 harness 清理范围。待单独批准的 harness cleanup 迁移该导入后，才能删除 wrapper 与对应测试。
3. 验证 gating（health/deferred/rewriter）顺序不变。

### 阶段 F：durable intent/recovery 连线

1. 核对 `SessionEvent`、`ReplayRequest`、`ReplayPermit`、boundary repair 当前字段。
2. 复用现有事件和 store，不新增平行 journal。
3. 在现有 Tool invocation/recovery 接缝连接 durable intent 所需的 descriptor identity 与当前 descriptor lookup；恢复只执行 fail-closed classification，不新增 handler recovery execution 或新的 outcome 写入路径。
4. 通过 descriptor lookup + revision/contract compatibility 判定恢复。
5. 保持 Safe Replay 已批准范围；不将 Phase 2B VerifyOnly 改成通用 exactly-once。

### 阶段 G：多面投影

1. 统一 descriptor projection helper。
2. 接入两个已存在且职责不同的消费者：MCP 投影与现有 model-visible `ToolDefinition`/ToolCatalog 投影。
3. 验证 discovery、invocation、error、unknown outcome 一致。
4. 删除 transport 自己维护的 metadata/replay/permission 分支。

### 阶段 H：扩展边界评估

Tool 闭环完成后，才评估 Agent、Task、Resource、Subscription 等下一期能力。每种能力必须单独回答：是否动态发现、是否稳定 identity、是否可替换 handler、是否跨进程/崩溃、是否需要 subscription/ownership、是否有可复用事实源。本期不因名称相似而自动泛化。

## 10. 测试与验证契约

### 10.1 Registry/lifecycle

- descriptor 注册、替换、移除、snapshot、revision/change feed；
- 重复注册、坏 schema、缺 handler、未知 revision、descriptor/handler mismatch；
- EffectScope/ToolRegistrationScope dispose 的逆序、幂等和失败报告；
- replace 后旧 invocation 使用旧 handler，新 invocation 使用新 handler；
- registry close 禁止新 resolve/register；
- registry 锁不跨 await；
- concurrent resolve/replace/dispose 不产生半注册状态；
- `BuiltinRegistryRouter`：每个主 builtin 可 resolve，`invoke` 委托 `execute_tool`，late-bound handle 更新后 router 无需重注册即见新 context；
- `MarkdownSkillRegistryOwner`：install/replace 后 resolve 到新 generation；removal 后 resolve 不可见；shutdown dispose 逆序且幂等；核心 builtin 名冲突 fail-closed。

### 10.2 Projection

- 同一 descriptor 至少驱动两个已有出口/消费者；
- MCP/RPC/CLI/Panel 投影的 identity、schema、revision 和错误原因一致；
- projection 不重新决定 replay、权限或成功状态；
- registry snapshot/change feed lag 按已有重建语义处理，不能假装无变化；
- run-loop 单快照投影的可见集与三源时代一致（含 allowlist 收窄、slash-skill scope、MCP face ⑤、defer_mcp_tools 提升）。

### 10.3 Durable/recovery

- durable classification：legacy/malformed identity、缺 descriptor、Unsafe、schema/revision/fingerprint mismatch 都 VerifyOnly；
- effective input 不可证明时，blocked/sanitized call 不会被恢复执行；
- 重复 classification/reduction 不追加伪造 outcome；
- recovery projection 使用原始 call identity，unknown 不被合成 success/error；
- 本期不新增 replay permit 消费、handler recovery invocation、durable replay claim 或 per-call crash budget；这些仍由 Phase 2B 独立 gate 管理；
- delegated VerifyOnly repair 没有 replay adapter/permit，不能调用 handler。

### 10.4 架构与源码清理

- `src/harness/` 无业务扩张，Phase 2 允许的字段 wiring 除外；
- 除 Plugin 的 catalog/extension manager 兼容旁路外，不存在绕过 `ToolHandlerRegistry` 的 Tool 生产/调用路径；
- `build_registry_from_tools`、`join_markdown_skills`、`to_metadata_form` 与主 builtin 的 `RegistryToolAdapter` 投影已删除或严格降级，且有明确理由；
- `git diff --check`、项目 Rust check/clippy/目标测试按当前项目门禁执行；
- 最终以 staged tree 验证提交内容，commit 后 worktree clean。

## 11. 文档更新契约

`docs/reference/FEATURE_LOCATOR.md` 必须补充：

- Tool Capability descriptor 的定义入口；
- `ToolHandlerRegistry` 唯一事实源和 `LoopToolRegistry` 纯消费者边界；
- registration scope、owner、dispose/disposer report 规则（含 `MarkdownSkillRegistryOwner`）；
- `BuiltinRegistryRouter` 与 `ToolRegistry::execute_tool` 的 thin router 关系；
- model/protocol/recovery projection 入口；
- discover 与 invoke 的分离；
- replay 默认 Unsafe、调用时 identity 和当前 descriptor 双重检查；
- unknown outcome 的 fail-closed 规则；
- 本期尚未统一的 Skill/Agent/Plugin/MCP/ACP/Resource/Task registry。

必要时同步 `docs/reference/ARCHITECTURE.md` 和 `docs/reference/TOOL_SYSTEM.md`，但不能把尚未实现的未来形态写成当前事实。文档中的状态、文件锚点和成功标准必须以最终代码为准。

## 12. 明确延后事项

- 全局 Capability trait 或统一所有 kind 的 registry；
- Agent/Task/Resource/Subscription 的 ownership tree；
- 通用 durable scheduler；
- external-effect ledger 和 exactly-once；
- Safe Replay 的完整执行放宽；
- approval/hook durable memo 的全量迁移；
- ACP agent server 与所有 transport 的统一能力目录；
- 跨进程 registry snapshot 的持久化；
- 没有真实消费者的预先抽象。
- Plugin 的 `ExtensionHandler` 收敛设计（将 extension/Plugin callable 统一进 canonical registry）。
- `ToolSource::MarkdownSkill { spec_path }` 变体（若未来需要 source 级 provenance 区分主 builtin 与 markdown-skill）。

## 13. 设计批准记录

本设计在对话中按四个阶段完成审阅并获批准：

1. 总体目标、问题重述、参考映射、范围和硬边界；
2. Tool Capability 的模块映射与数据流；
3. registration 生命周期、替换、durable intent 与 replay 契约；
4. 安全边界、可观测性、迁移顺序与本期验收范围。

后续步骤是：用户审阅本书面 spec；若批准，才进入 `writing-plans` 阶段生成实现计划。实现计划必须继续遵守独立 worktree、先连线后扩展、任务分支隔离、分阶段验证和旧路径清理要求。

**修订记录（2026-10-05）**：用户批准收窄范围。基于 Task 1 源码 census（记录于 `.superpowers/sdd/2026-10-05-capability-phase3/task-1-report.md`，该报告未提交），将成功标准 #1 从「builtin + extension + MCP 全部 canonical」收窄为「MCP + 主 builtin + markdown-skill `AlephToolDyn` callable surface」；Plugin 无 `ToolHandler` 实现，保留 `ToolCatalog`/extension manager 旁路作为兼容边界，并另列 ExtensionHandler spec 为延后事项。`to_metadata_form` 仍是 live production path，仅在 builtin canonicalization 与全部 caller 迁移后才删除/降级。

**修订记录（2026-10-05，Task 2 架构缺口）**：Task 2 实现暴露两处架构缺口，据此规定精确分发语义：

1. **主 builtin**：进程级注册 `BuiltinRegistryRouter`（thin router over `ToolRegistry::execute_tool`），late-bound context 留在 `BuiltinToolRegistry` 的 Arc handle/OnceCell 内，不捕获进 handler；run-loop 从单一 canonical snapshot（`entries_snapshot()`）构建 request-scoped visible projection，用统一 allowlist predicate 过滤，删除 `build_registry_from_tools`/`RegistryToolAdapter` 的二次 handler 投影。不创建第二份 handler map、不创建 test-only/dead-write adapter。
2. **markdown-skill**：install/hot-reload 经 `MarkdownSkillRegistryOwner` 写入 canonical registry（`registry.replace` + `ToolRegistrationScope` track/dispose），generation-safe replace，removal 走 `unregister`，shutdown 走显式 `dispose`，不用 `async Drop`。保留 `AlephToolServer` 的 tool store 职责。与主 builtin 同用 `source=Builtin`，核心 builtin 名冲突 fail-closed。

两份文档只改 spec + plan，不改任何 `src/tests`。commit message：`docs: specify callable dispatch architecture`。
