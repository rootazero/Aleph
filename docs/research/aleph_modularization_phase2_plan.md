# Aleph 模块化第二阶段实施计划 / Modularization Phase 2 Implementation Plan

> 状态 / Status：待实施的设计与验收契约，不是实现完成报告。
> 已确认基线 / Verified baseline：第一阶段提交为 `0cc141a0d5ed4e81a18ffcaa2cb6d1c9c336d0b5`（简称 `0cc141a0d`）；当前审查 HEAD 为 `589b424cc2febb21957b4d72cea932c0ad78ad1d`（简称 `589b424cc`），其新增内容是本阶段计划文档，不能把当前 HEAD 写成第一阶段基线。
> 工作区实际状态：Rust/runtime 路径无已跟踪未提交变更；本会话新增的 office-hours 设计记录位于 `docs/designs/`，后续需单独纳入 diff 审查。
> 工作目录 / Working directory：`.`，共享工作区。本轮任务明确指定该检出；不要因旧路径说明切换到其他卷或复制仓库。
> 下一会话默认工作包 / Default next-session package：**A：受控 WASM runtime-owner lazy activation**。**B：canonical plugin producer + per-provider entry fencing + long-lived projection** 独立 gate，不随 A 自动获准。
> 续跑入口 / Resume prompt：[aleph_modularization_phase2_prompt.md](aleph_modularization_phase2_prompt.md)。总路线背景：[aleph_modularization_plan.md](aleph_modularization_plan.md)。

## 1. 用户目标与成功定义 / User Goal and Success Criteria

长期方向是开箱即用、可替换模块、单一权威与可观测生命周期；第二阶段**不是泛化所有插件或所有 Capability kind**。本次先把“声明发现 / declaration discovery”和“运行时启动 / runtime activation”拆开，再在现有 runtime owner 下完成受控 WASM lazy activation。

两个验收单元必须分别报告：

- **A 成功**：发现、列举、catalog、diagnostic、`describe/resolve` 不创建 WASM VM；获准的真实调用通过明确的 owner activation 请求单飞启动；真实模块完成 mount 才 Ready；失败回滚、停止安全、竞态隔离、控制事务与状态投影有证据。A 必须证明生产 owner 的路径，而非只对纯状态机调用 `transition`。
- **B 成功**：插件 handler 真正由 canonical `ToolHandlerRegistry` producer 发布；每条 entry 绑定正确 provider instance/generation；完整工具调用链通过同一 registry；停掉一个插件不影响 builtin 或其他插件；已存在的长寿命视图收到真实更新并重建。B 的入口依赖 A 交付和契约冻结，并需独立工作包授权。

**诚实边界 / Honest boundary**：若当前 canonical producer 仍未接线，A 的端到端仅指 `ExtensionManager` activation owner → 实际 WASM 调用/清理，不是模型 → canonical plugin tool 的完整闭环。不得把 metadata/catalog 注册或 legacy tool-name index 当作 callable producer 完成。若用户要求“完整插件工具调用”，必须先把 B 纳入前置依赖并重新审定范围，不能用 A 的局部测试替代。

**为什么 / Why**：先证明副作用归属和生命周期安全，再扩展 callable publication，可隔离错误来源。**代价 / Trade-off**：A 不交付所有用户可见的插件工具闭环，需维护明确的兼容边界，B 不能被省略。

## 2. 已确认基线与接线 / Confirmed Baseline and Wiring

以下运行时代码事实以第一阶段提交 `0cc141a0d5ed4e81a18ffcaa2cb6d1c9c336d0b5` 为基准；当前 `HEAD=589b424cc2febb21957b4d72cea932c0ad78ad1` 已复核，运行时代码没有已跟踪差异，新增的是计划/设计文档。函数名优先于易漂移的行号。下一会话须复查 HEAD/diff，有变化先 review，不 reset 他人工作。

| 路径与准确符号 / Path and symbol | 当前事实 / Current behavior | 第二阶段含义 / Implication |
|---|---|---|
| `src/capability/provider.rs`：`ProviderLifecycle`、`ProviderHandle`、`ProviderState`、`ProviderLifecycle::transition` | 纯 state+generation；`Starting` 分配新 generation，`checked_add` 溢出拒绝；没有启动进程/模块的副作用 | 保持纯契约，runtime owner 驱动；当前 enum **没有 Cold** |
| 同文件：`ProviderLifecycle::acquire_call`、`ProviderHandle::acquire_call`、`ProviderCallError` | 原子检查 Ready + expected generation，返回 `Result<(), ProviderCallError>`；不计数、不 drain、不取消正在执行的工作 | 不能把准入检查当 unload 安全证明 |
| `src/tools/registry.rs`：`ToolHandlerRegistry::new`、`new_with_provider`、`provider` | 默认构造唯一全局 `aleph/tools`，`ProviderLifecycle::ready`，generation 1；provider 是 registry 级字段 | 不能把该 handle 改成某插件的状态，否则整个工具面一起消失 |
| 同文件：`ProviderCallHandler`、`RegistryEntry::for_provider`、`resolve_entry`、`snapshot_state`、`unregister` | 导出的 handler 包装调用点准入；descriptor/handler 同代取出 | 复用已提交 fencing，不复制第二套 registry；B 才细化 per-entry provider |
| 同文件：`CanonicalToolState`、`plugin_declarations_snapshot`、`reconcile_plugin_tools` | producer 接口明确返回 `ToolError::Execution`，cause = `plugin producer reconciliation is not mounted`；拒绝时不发布 | 原始 state/catalog 不可绕过 handler admission；A 不将该错误改成假成功 |
| `src/capability/facade.rs`：`BackendLease`；`src/capability/zahir_facade.rs`：`resolve`、`validate_lease`、`subscribe` | lease 带 provider generation；旧代/非 Ready 返回 `StaleProvider`；订阅为同步 snapshot-anchored notification，无 durable replay | 不把同步查询改成隐式 async startup；不把 broadcast 说成恢复源 |
| `src/extension/plugin_state.rs`：`PluginsConfig::activation`、`is_enabled_for`、`set_enabled`、`save` | `<data_dir>/plugins.toml` durable enabled，保留 origin 默认、显式 disabled、NotEnabled 区分；save 复用 atomic writer | desired-state 唯一权威；不得新增 `.disabled`、另一个 enabled map 或配置文件 |
| `src/extension/mod.rs`：`ExtensionManager::ensure_loaded` | `load_guard` 双重检查 → `load_all_locked`；名为 lazy-loading，但实际延迟的是整次 extension load，不是 per-provider lazy activation | 管理读侧若调用此入口，今天仍可能触发 eager mount，A 必须拆开 |
| `src/extension/lifecycle.rs`：`load_all_locked`、`discover_and_mount`、`scan_plugins_blocking`、`build_record`、`admit` | 扫描/解析放 blocking pool；shadow/origin/trust admission 后直接 `mount_parsed`；reload 先 unmount 全部 | 提炼 discovery 与 mount 的边界，不能以改名伪装无副作用发现 |
| 同文件：`mount`、`mount_inner`、`mount_parsed`、`fail_mount`、`unmount_inner`、`unmount_all`、`reload_plugin`、`reload` | mount 六步 EffectScope；失败逆序 dispose；unmount 后恢复 Disabled row，dispose 失败也可能仍呈 Disabled | A 需表达 actual 停止失败，不能沿用 Disabled 冒充 Stopped |
| `src/extension/effects/scope.rs`：`STEP_LABELS`、`EffectScope::dispose`、`DisposeReport::all_ok` | `registry_row` → `wasm_module` → `mcp_server` → `service` → `memory_extension` → `slash_command`；逆序清理，错误/恐慌汇总 | 延迟生成的 effect 仍必须由对应实例 scope 拥有，不能悬挂在 scope 外 |
| `src/extension/loader.rs`：`load_wasm_effect`、`PluginLoader::call_tool`、`call_bound`、`unload_wasm_mount`、`shutdown` | VM 在 mount 时创建；load 和 mount identity 捕获在一个 loader write guard 内；disposer 只移除同一 mount | 保留真实 identity 防护，而非仅比较字符串 plugin id |
| `src/extension/runtime/wasm/mod.rs`：`WasmRuntime::load_plugin_with`、`mounted_instance`、`remove_if_current`、`binding_is_current`、`call_bound` | `WasmMountIdentity` / `WasmBinding` 持有实际 instance Arc；旧 binding 不可执行替代实例；Arc 延长 VM 分配存活不等于继续执行许可 | 可复用存活/身份机制，但须审计 loader guard 与停止顺序 |
| `src/extension/plugin_ops.rs`：`call_plugin_tool`、`execute_plugin_hook`、`ensure_runtime_mounted` | 只接受已 active 且已 mounted；真实 guest 调用 `spawn_blocking`，loader owned guard 随调用持有；不是 lazy 启动入口 | A 在明确调用路径请求 owner activation，不能在 loader 缺模块时偷偷 load |
| 同文件：`set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> bool` | 先改 live config，再 save；保存失败只 warn；配置锁释放后另取 mount/unmount 锁；bool 合并 preference/runtime 变化 | 事务顺序与错误传播是必修项，不是现有保证 |
| `src/gateway/handlers/plugins/handlers/manage.rs`：`set_enabled_via_registry` | 忽略 toggle bool，返回 `{ok:true}` | RPC 必须传播结构化失败，尤其 persistence failure |
| `src/gateway/handlers/extensions/lifecycle.rs`：`handle_toggle` plugin 分支 | 调 manager 后无条件 `Ok(())`，供 Hub `extensions.toggle` 使用 | Hub 路由不能继续吞错误；其 MCP/skill 分支不扩展为 lazy |
| `src/builtin_tools/plugin_manage.rs`：`PluginAction::Enable/Disable` 分支 | 消费 bool，生成 enabled/disabled/already 文案 | 需区分持久化、期望状态、实际状态，不能“保存成功”等于 Ready |
| `src/extension/types/plugins.rs`：`PluginStatus::is_active`、`is_terminal`、`label` | 当前 `Loaded`/`Pending` 都 active；Pending 已 mount、等待 MCP dependency，不是冷态 | 冷态不能复用 Pending/Loaded；新状态须审计所有 exhaustive match/wire consumer |
| `src/extension/readiness.rs`：`derive_readiness`、`write_readiness`；`src/extension/lifecycle.rs`：`watch_server_starts`、`activation_settled` | readiness watcher 在 load_guard 下发布；write 返回的是 is_active 是否变化，不是任意 readiness 是否变化 | 不能靠该 bool 自动保证 Cold/Starting/Ready 的每次显示更新；MCP 语义保留 |
| `src/extension/projection.rs`：`Views::derive_plugin_projection`、`republish_plugin_projections`；`src/extension/lifecycle.rs`：`Views::after_transition` | lifecycle 是 projection 发布唯一入口，重建 hooks/skills/agents/tool metadata 并通知 MCP face | 保持唯一重建入口；真实 readiness 变化也要通知/重建 |
| `src/extension/mod.rs`：`sync_runtime_snapshots`、`plugin_tool_revision`、`active_plugin_tools_snapshot`、`resolve_active_plugin_tool`、`set_tool_handler_registry` | active tool index 是 metadata 视图，不是 canonical callable；canonical Arc 只允许首次 load 前挂载，同 Arc 重挂可，不同 Arc 拒绝，且不 seed callables | 不以 cache/revision/attachment 存在证明 producer 已接通 |

**第一阶段结论 / Phase 1 conclusion**：provider lifecycle fencing、generation、call admission 已完成且已提交；生产 transition 仍 dormant。生产默认全局 provider 恒 Ready / generation 1；`new_with_provider` 是装配缝，不能把测试中驱动 Starting 的证据说成生产已 lazy。已有 `ExtensionManager` mount 副作用与新增 provider state machine 尚未接成同一个 runtime-owner 生命周期。

## 3. 设计原则与单一事实源 / Principles and Single Sources of Truth

| 事实 / Fact | 唯一权威 / Authority | 允许的派生 / Allowed projection | 禁止 / Forbidden |
|---|---|---|---|
| 用户是否希望启用 | `PluginsConfig` / `src/extension/plugin_state.rs`，durable enabled + origin default | desired_enabled 与原因 | catalog、ProviderHandle 自己持久化另一份 enabled |
| 实际启动/停止、scope 归属、单飞协调 | `ExtensionManager` / `src/extension/lifecycle.rs` | 当前实例引用、in-progress operation、diagnostic | catalog/diagnostic/describe/resolve 直接启动 |
| 单个实例 state + generation | `ProviderHandle` / `src/capability/provider.rs` | runtime status snapshot；manager 保存同一个 handle 的引用 | manager 再维护一套可独立写的 state/generation |
| 可调用 descriptor + handler | `ToolHandlerRegistry` | catalog/LLM tool list/MCP/diagnostic | `PluginRegistry` / active tool index 成为第二 callable registry |
| mounted VM identity 与存活 | 实际 `WasmMountIdentity` / `WasmBinding` 与 loader map | 实例一致性检查 | plugin id / revision 相同就授权旧 completion 或 disposer |
| Capability owner generation | `OwnershipTree`，lease 捕获 owner generation | validation | 与 provider generation 或 registry revision 混为一数 |
| registry revision | canonical registry 的真实事务 | snapshot cursor | 用生命周期事件虚增 revision 假装 entry 改了 |

**Single writer, multiple views / 单写者多视图**：manager 可以保存“plugin instance → shared ProviderHandle / operation / scope”的 runtime ownership 关联，这不是另一个 callable registry，也不是另一个 durable desired-state store；状态仅经同一 handle 迁移。声明目录存 manifest facts，不存可执行 handler，不授予调用许可。

**为什么**：authority 明确才能测试“效果到达”。**后续更难**：同步 facade 和异步启动不能合并，必须有独立调用协议；动态索引必须处理失效，而不能缓存一份“看似最新”的权限。

## 4. 范围与非目标 / Scope and Non-goals

### 4.1 A：受控 WASM / Controlled WASM

默认推荐先覆盖显式纳入的、tool-only WASM 插件：没有启动即必需的 service、MCP、memory、hook、context 初始化依赖。该资格从解析的 manifest 得出，不由 catalog 推测；具体字段判定在契约冻结时确认。对混合插件保留现有 eager 兼容行为并明确标注 `activation_mode`/原因，不静默改变启动依赖，也不声称“所有插件已 lazy”。若要扩大到这些插件，先停止问用户，不暗加自动 eager fallback。

A 包括：discovery/mount 拆分；明确 Cold；原子控制事务；runtime-owner activation 单飞；真实调用入口；安全 stop/reload/shutdown；实例身份与 generation fencing；管理状态和现有 projection 更新；效果级测试。任何被纳入 A 的 WASM，不得在 boot/read 路径启动。

### 4.2 B：独立 producer gate / Independent Producer Gate

A 验收后另行冻结：canonical plugin producer admission/publication、每 entry provider identity、handler 包装与 lease 关联、事务 rollback、同名冲突 fail-closed、长寿命 tool/catalog/facade/MCP projection 更新路径。不得通过给每个插件建一个独立 ToolHandlerRegistry 规避 canonical 问题。

### 4.3 明确不做 / Explicit Non-goals

- MCP lazy activation；其 existing readiness、transient server 管理不得被 A 泛化重写。
- Gateway / Terminal / Memory backend 统一（仅允许 plugin 控制 RPC 薄适配，不等于 Gateway backend 接入）。
- sidecar supervisor、Bridge 自动拉起/重启策略、Rust dylib 或热卸载。
- Safe Replay、EffectClaim producer integration、external-effect exactly-once、durable scheduler。
- 跨 Capability kinds、slot 总框架、DI container、级联重启、harness 编排智能。
- 全新权限模型、自动授予 trust/context/secrets、以 lazy activation 绕过 sandbox/operator/confirmation。
- A 中 canonical plugin callable producer 完整接线、B 的长寿命 projection 大改、自动 push。

**取舍**：缩小 A 才能验证真实 owner；代价是混合插件和完整 callable 集成仍需独立设计，不以“通用框架”预付复杂度。

## 5. 生命周期与状态语义 / Lifecycle: Desired vs Actual

### 5.1 两个正交维度 / Orthogonal dimensions

`desired_enabled` 来自 durable config，包含显式 false / origin 默认未启用的原因；trust 是额外 admission guard，不由 enabled 隐式授予。`actual_state` 来自当前实例 handle。保存 enabled=true **不保证** Ready；停止也不自动把 durable enabled 写 false（如 reload/shutdown）。

| 状态 / State | 准确语义 / Meaning | 可调用 / Callable |
|---|---|---|
| discovered | 已识别声明，未完成 admission/activation 决策，无 VM | 否 |
| disabled | desired 不允许启动；原因可区分显式 disabled / NotEnabled；不能掩盖仍在 stopping/failed 的残留实例 | 否 |
| cold | desired enabled、trust/资格已确认、尚未请求启动，无 scope runtime effect/VM | 否；真实调用可显式请求 activation |
| starting | 唯一 owner operation 正在创建该 generation 的 mount；等待者共享结果 | 否，不可提前发布可调用 handler |
| ready | 当前实例 mount 所需效果全部成功，identity/scope 已绑定且仍 current | 是，但仍必须过 permission/visibility 与调用点 fencing |
| stopping | 已封闭新调用/activation，正在等待既有调用或安全保留资源、dispose | 否；先前已准入的调用按契约完成 |
| failed | 启动、调用导致实例不可用或停止失败；保留 phase/error/cleanup detail | 否；不能自动说已 stopped |
| stopped | 该实例已完成逻辑停止及所需清理，无新调用许可；物理 VM 可因安全 Arc 暂留 | 否；desired 仍 true 时可由明确新请求再启动 |

`Blocked` 保留为独立 policy 拒绝原因，不混入冷态。现有 MCP `Pending` 仅表示已 mount 后等待声明依赖，继续保留其非依赖能力语义；不能用 `Pending`、`Loaded`、`is_active()` 表示冷 WASM。

### 5.2 推荐迁移契约 / Recommended Transition Contract

以下是**拟新增契约，不是当前 enum 已支持的边**。冻结时优先在纯 `ProviderState` 中增加 `Cold`，而非额外 mutable enum。

```text
Discovered --desired false--> Disabled
Discovered --enabled + eligible--> Cold
Disabled --durable enable + admission--> Cold
Cold --explicit activation--> Starting(generation + 1)
Starting --mount complete, current identity--> Ready
Starting --failure or invalidated operation + rollback--> Failed
Ready --disable/reload/shutdown--> Stopping
Stopping --safe lifetime + cleanup complete--> Stopped
Stopping --cleanup/drain failure--> Failed
Stopped/Failed --explicit retry/reconcile + allowed--> Cold 或受控 Starting
Cold --durable disable--> Disabled
Stopped --desired false--> Disabled
```

- generation **只在 Starting 进入时**分配，overflow fail-closed；discovery、读请求、投影重建不递增。
- 禁止 Ready→Disabled 跳过 stop；Starting 中 disable 先失效 operation，阻止 Ready commit，待其完成/回滚，再表达 disabled 的 desired 与 failed/stopped 的 actual，不伪造 Starting→Stopped 成功。
- Failed→Stopped 仅在确实完成补偿清理后允许；启动失败初始报告仍 Failed。没有自动恢复策略或无限 retry。
- `Stopped` 是逻辑停止；如仍被 binding Arc 持有，不承诺立即释放内存。若无法证明在途执行资源安全存活，则不能 unload，也不能报清理已完成。
- 所有 public/wire 状态 match 都须审计，包括 `shared/protocol/src/plugins.rs`、diagnostic/Hub 输出及必要的消费者；不要把 Cold 映射为 Loaded 求兼容。

**为什么**：状态是可验证的事实而非 UI 文案。**代价**：加 Cold 会触及 exhaustive matches/wire 兼容；应同步最小状态消费面，不借机重写整个 Panel/TUI。

## 6. Provider 粒度与 canonical 约束 / Provider Granularity and Canonical Registry

1. builtin 继续属于全局 `aleph/tools`。单个 WASM 实例有自己共享的 ProviderHandle，由 manager 创建并驱动；稳定 provider id 与每次 mount identity、generation 分开表达，不能相互替代。
2. A 的该 handle 管理 runtime activation，**不赋给全局 registry**。A 不改全局 readiness，也不以返回空全局 snapshot 模拟某插件关闭。
3. B 中每个 callable entry 关联 provider handle + captured generation + instance identity；注册事务需要 validate descriptor/handler definition、provider 归属、duplicate key/name、closed registry。通过后才发布，失败不改变任何 canonical value/revision。
4. lease 同时验证 descriptor freshness、owner、visibility、provider generation；不得把所有 provider generation 压成一个全局数。旧 handler 在 invocation 时仍拒绝，不只 resolve 时检查。
5. `reconcile_plugin_tools()` 的未挂载错误必须保留到 B 真正完成 producer gate；catalog 和 declarations 可存在，但不给它们 handler 执行权。
6. B 的 disposer 必须以 registration token/revision + instance identity 只撤销自己的 entry，不能按 name 清掉替代实例。A 继续复用 `load_wasm_effect` 的真实 mount identity 防护。

**为什么**：防止“停一个插件停全局”与第二 registry。**代价**：B 要调整 entry/lease/publication 契约，不能靠把 `new_with_provider` 多调用几次完成。

## 7. 分阶段 DAG / Implementation DAG

```text
P0 基线复核 + plan review + 契约冻结
  └─ P1 控制事务（persist → publish → runtime reconcile）
       └─ P2 A：声明发现拆分 + owner 单飞 activation + 明确调用入口
            └─ P3 A：stop / reload / shutdown / cancellation / lifetime
                 └─ P4 A：管理状态投影与真实通知 + 效果验收
                      └─ GA：A 独立验收，冻结后续 entry/identity 契约
                           ├─ P6：A 文档/交付（本会话默认结束点）
                           └─ GB：独立授权与 plan review
                                └─ P5 B：canonical producer + per-entry fencing
                                     + long-lived projection → B 验收 → P6 文档
```

P2/P3 高耦合，优先顺序实施，不虚构并行。P0 后可双线进行 core owner 与只读 caller/测试调查，但文件写权限必须互斥；独立测试文件可并行编写，cargo 共用 target 时由一个验证 worker 串行运行。主 agent 负责分析、契约、编排与验收；实现与测试执行委派 subagent，收到结果后看实际 diff 和命令 exit 证据。

- **P0 旁路冻结 / Bypass freeze**：实施前必须审计并统一所有会创建或销毁 WASM 实例的入口。当前 `ExtensionManager::mount` 是公共启动原语（`src/extension/lifecycle.rs:155-158`），`discover_and_mount` 先执行 `unmount_all`（`src/extension/lifecycle.rs:230-246`），`call_plugin_tool` 按 plugin id 获取并调用 loader（`src/extension/plugin_ops.rs:18-29`），`reload_plugin` 忽略除 NotFound 外的 unmount 结果（`src/extension/lifecycle.rs:175-180`）。A 必须明确这些路径是否属于显式管理 activation、如何纳入同一 RuntimeOwner，以及如何捕获并在 guest 边界重新验证 instance/mount identity/generation；读侧 discovery 不得为了刷新声明而停止既有实例，清理 residual 必须 fail-closed 保留所有权。
- **P0 caller census 结论 / Caller census**：管理 list/read 当前仍可能触发启动：`src/gateway/handlers/plugins/handlers/manage.rs:23-72`、`src/builtin_tools/plugin_manage.rs:213-225` 调用 `ensure_loaded`，offline CLI `src/bin/aleph-server/commands/plugins.rs:17-69` 调用 `load_all`；A 必须将这些读路径改为纯 declaration discovery。`plugins.callTool`（`src/gateway/handlers/plugins/handlers/runtime.rs:8-42`）是现有 tool call owner path；`plugin.reload`（`:48-83`）和 enable/disable 属于显式管理控制。hook/service/context 调用（`src/extension/hooks/executor.rs:751-780`、`src/extension/service_ops.rs:13-57`、`src/extension/context_adapter.rs:228-375`）不扩大 A 的 tool-only eligibility，混合插件保留既有 eager 兼容；其中 context adapter 的 permit 持有至 native call 返回、调用前后重新验证 binding/grant，可作为 A lifetime/fencing 参考。offline uninstall `src/bin/aleph-server/commands/plugins.rs:175-204` 直接删除目录，必须在 P3 前纳入 lifecycle stop/unmount 或 fail-closed 阻止删除，不能留下运行中的 VM/binding/watcher。
- **P0 RuntimeOwner 字段/锁序合同 / RuntimeOwner contract**：每个 mounted incarnation 绑定 `plugin_id + mount_id + generation`、共享 `ProviderHandle`、单飞 operation、open/closed admission、in-flight permit、现有 `EffectScope` ownership 和 exact-instance loader binding/disposer。`admit` 的 permit 覆盖整个 guest/native 执行和结果返回；`begin_stop` 先关闭新 admission，再等 permit 清零，之后才允许 exact disposer；旧 completion/disposer 只能 stale/no-op。建议锁序为 `load_guard → 短 registry/status 读取 → owner operation/admission → loader guard → guest mutex`，不得持有 registry/loader/lifecycle 锁跨越对方需要的 await，不得在 load_guard 内等待 watcher；既有 watcher cancel/join 先于 disposer。RuntimeOwner 不复制 durable enabled、callable registry、EffectScope disposer map。

- **P0 资格与停止证据冻结 / Eligibility and stop proof**：tool-only 资格不能只看 WASM 类型，必须由 manifest 与 adapter 产生的 capabilities 共同证明，并在 P2 前冻结可执行 predicate。`spawn_blocking` waiter drop 不构成 guest 中断证据；P3 前必须证明 eligible guest 与相关 blocking host call 的预算/interruption/join 行为，无法证明时保留资源并报告实际 Failed/residual，不得报 Stopped/unloaded。

### P0：Review 与冻结 / Review and Freeze

- **允许**：只读本计划 §2 路径、项目 `AGENTS.md`、`docs/reference/{REDLINES,HARNESS_PHILOSOPHY,PLUGIN_SYSTEM,TOOL_SYSTEM,SECURITY,DEVELOPMENT,SUBSYSTEM_ROUTING,FEATURE_LOCATOR}.md`，现有 modularization tests/fixtures，`src/bin/aleph-server/commands/plugins.rs` 等真实调用面。
- **交付**：HEAD/status 与共享脏文件归属；discovery/runtime 调用 census；A eligibility、Cold 表达、activation API、停止策略、锁顺序、结果 schema、状态 wire 影响的短评审。
- **验收**：能画出一个真实调用从权限闸到 owner 的路径；明确 canonical producer 尚缺；确认只实施 A。不能以“代码稍后再看”通过。
- **为什么/代价**：先冻边界避免 A 悄悄长成 B；会增加前置 review，但减少返工。

### P1：控制事务 / Serialized Control Transaction

- **允许**：`src/extension/{plugin_state,plugin_ops,lifecycle,mod}.rs`；薄适配 `src/gateway/handlers/plugins/handlers/manage.rs`、`src/gateway/handlers/extensions/lifecycle.rs`、`src/builtin_tools/plugin_manage.rs`，以及真实消费 toggle 的测试/CLI 调用面。
- **契约**：单一控制序列覆盖读取当前 config、clone candidate、变更、`candidate.save()` 成功、替换 live config、发布 desired、runtime reconcile。沿用原子 writer；不允许持有 cfg write guard 后重入会取该锁的 mount。私有 `_locked` helper/owner operation 可以拆锁，但顺序 token 不得释放给另一个 toggle/reload 使其越序。
- persistence fail → 返回结构化错误、disk/live/runtime 不变；save success + runtime fail → 保留已保存 desired，如实返回 reconcile failure/actual Failed，不回写旧 desired 假装事务全撤销；重启可据 durable desired 重建。
- **建议结果字段 / Proposed result, not existing API**：`plugin_id`、`desired_enabled`、`preference_changed`、`persisted`、`actual_state`、`provider_generation`、`operation_id`、`runtime_outcome`、`error_phase`、`cleanup_report`。错误能携带 committed desired；NoChange 与 Failed 分开，不能再用 bool 包含所有结果。
- enable 对 A 冷插件仅达 Cold，不自动启动；disable 先保存 desired=false 再封闭启动/调用、停止现存实例；未发现 id 可保存偏好但必须回报 `not_discovered`，不能声称已启动。各 RPC 可保留 not-found policy，但不得吞失败。
- **禁止**：RPC/Hub 自己持一套 enabled、直接开关 loader；修改 MCP/skill 分支启动语义；对 untracked 外部配置文件做不受控写入。
- **交付/验收**：串行双 toggle 测试；保存失败无副作用；save 后 runtime fail 的 structured result；RPC、Hub `extensions.toggle`、builtin tool 均不假成功；同值 toggle 可执行待完成 reconcile 而非一律 no-op。
- **为什么/代价**：磁盘才是重启后意图来源；序列化会增加 toggle 延迟，runtime 失败需要客户端理解“desired 已提交但 actual 失败”。

### P2：A lazy owner / Discovery Split and Explicit Activation

- **允许**：`src/extension/{lifecycle,plugin_ops,mod,loader}.rs`、`src/extension/types/plugins.rs`、`src/capability/provider.rs`，必要的既有 effects/fixture 文件；优先复用现有结构。
- discovery 只产生验证过的声明、origin/scope/shadow/trust 与 desired projection；eligible WASM 不跑 `wasm_module/service`，不制造 Ready、Loaded 或 active capability。
- **明确实际调用入口 / Explicit invocation protocol**：在既有 `ExtensionManager::call_plugin_tool` 这类 owner 调用面调用拟新增的私有 `request_runtime_activation(plugin instance, reason, caller context)`（名称待 P0 冻结，不是现有函数）。权限/visibility/trust/desired 检查后，Cold→Starting 单飞；join 同一实例 operation；获得 Ready 结果后绑定该 mount/generation，再在实际 guest 调用边界复验并执行。不从 catalog/`describe`/`resolve` 调用它，不从 loader missing 分支直接创建 VM。
- A 可用可信测试/现有合法调用者调用此 owner API；审计真实 callers，不能创造一个绕过 ToolService 授权的公共快车道。cold 在 canonical tool list 中未暴露时，不声称模型可以自动发现并完成调用。
- manager 保存单飞 operation，caller cancellation 不使共享启动无人清理；mount 成功且 scope 安装、identity/generation/current desired 均匹配后才 commit Ready。所有 effects 归入该实例 scope。
- **禁止**：为完成 A 修改 `reconcile_plugin_tools` 成功路径、新建 callable registry、用 Pending active 让 cold 进入 tool list；将 mixed WASM 的 hooks/context/service 自动纳入 lazy。
- **交付/验收**：boot 与反复查询 VM 数为 0；并发真实请求只创建一个实例、一个 generation；fixture 返回真实 payload；disabled/trust deny 零启动；失败无残余 effect 且状态 Failed。
- **为什么/代价**：调用协议显式才不会把读操作变成副作用；代价是查询到冷声明不等于可以取得 callable lease，B 或显式调用者需负责衔接。

### P3：安全停止与替换 / Stop, Reload, Shutdown

- **允许**：P2 owner/loader/runtime 路径、`src/extension/effects/{scope,disposer}.rs` 的必要变更与对应测试；沿真实 shutdown 调用链做局部适配。
- 停止顺序：失效 activation operation/封闭新准入 → 等待既有调用安全完成或证明其资源独立存活 → dispose 原实例 scope → 发布 Stopped 或 Failed。reload/shutdown 不修改 durable enabled；eligible WASM reload 默认回 Cold，由下一真实需求重新 Starting。
- 推荐 A 复用 owned loader guard + 实例 Arc 的已存在存活机制，明确等到 guard 释放再卸载；以 barrier 证明在途 guest 不被释放。若锁顺序不允许安全等待，改为明确 permit/Arc 执行引用方案并复审；没有证据时选择“不 unload，保留资源并报 stopping/failed”，不能先 dispose 后补说明。
- `spawn_blocking` 开始后的任务不可当作普通 future 一 drop 就已停止；旧 completion、watcher、disposer 都须同时校验 plugin instance/mount identity/generation。新实例不能被旧清理删除，旧启动不能复活 disabled/replaced 实例。
- **交付/验收**：在途调用、late startup、reload 同 id、shutdown cancellation、cleanup error 均有 barrier 驱动效果测试；不只有 acquire_call 失败断言。
- **为什么/代价**：admission 只解决入口，不保证资源存活；等待 guard 会串行化其他插件调用，permit 方案更灵活但更复杂。本阶段不顺手解决所有插件并行性能。

### P4：A 状态投影与通知 / A Status Projection and Notifications

- **允许**：`src/extension/{projection,readiness,lifecycle,mod}.rs`，必要的 `shared/protocol/src/plugins.rs`、diagnostic/Hub/RPC 状态适配、相应最小 consumer/test；改消费者前先列准确文件。
- management 可见 discovered/disabled/cold 等声明；可调用视图仅 Ready 且满足授权。声明视图与 callable 视图命名明确；不得扩大 `is_active` 让 Cold 活跃。
- 经同一 `Views::after_transition` 或其明确委托的唯一发布入口重建。Cold→Starting→Ready、Ready→Stopping→Stopped/Failed 要有真实状态通知/重建路径，不能只依赖 is_active bool 翻转。A 覆盖已有管理/metadata 视图，B 的长寿命 canonical 投影另验。
- `plugin_tool_revision` 是 metadata revision，不是 canonical registry revision；provider broadcast 是 notification，lag 后从当前权威 snapshot 重建。subscribe-before-snapshot/去竞态按既有模式复用，不承诺历史 replay；durable recovery 只认 `SessionEventStore`，本阶段不新增生命周期 event-sourcing。
- **交付/验收**：先创建 observer，再通过真实 owner 变迁，observer 最终确实看到新状态与能力撤回；lag/重连 snapshot 不复活旧实例；投影读取不增加 load/generation；不伪造 canonical revision。
- **为什么/代价**：一次性 snapshot 测试不能证明在线客户端更新；保留多种 revision 会让观测面更严格，但避免制造错误全序。

### P5：B producer 独立 gate / B Producer, Separately Approved

- **前置**：GA 通过、GB 独立批准；若 A 未完成，不准提前混入。
- **允许（审批后）**：`src/tools/registry.rs`、`src/capability/{backend,facade,zahir_facade}.rs`、`src/extension/{lifecycle,mod,projection}.rs`、`src/tool_metadata/registry/`，列明的 tool refresh/MCP consumer 与 producer 测试；仅 Tool kind。
- producer 从已验证声明与真实 mounted handler 生成 descriptor/handler 对，走 canonical admission；同事务关联 provider identity、注册效果与撤销 token。不能把未验证 `CanonicalToolState` 直接塞入权威。
- 长寿命 projection 在创建后发生 provider start/stop/restart/replace 时也能刷新；通知监听者从权威重建，不让 query 触发 activation；各 source revision 保留真实语义。
- **验收**：真实 WASM → canonical handler → ToolService 既有授权/调用/输出链闭环；旧 handler/lease fail-closed；一插件停止不影响其他插件/builtin；同名冲突无部分提交；旧 disposer 不撤销新 entry；长期持有 facade/catalog/MCP observer 能看到变化。
- **为什么/代价**：这是 callable authority 的变更，不是 metadata 工作；需要更广回归与 wire/entry 设计，不能用 A 的 runtime test 代替。

### P6：文档交付 / Documentation and Delivery

- **允许**：本计划及 prompt、`docs/research/aleph_modularization_plan.md`、相关 `docs/reference/{PLUGIN_SYSTEM,FEATURE_LOCATOR,TOOL_SYSTEM,ARCHITECTURE}.md` 的事实更新；不需修改所有 reference。如改 `AGENTS.md` 必须同步 `CLAUDE.md`，但本阶段优先不动这两个入口。
- **交付/验收**：标明 A/B 分别 implemented/deferred、准确 production call graph、实际命令/exit 证据、资源门、未验项；删除被替代的旧状态说明，不制造第二套规范。
- **为什么/代价**：新会话依赖文档延续；需要维护事实边界，不能把愿景用完成时书写。

## 9. 并发、取消、失败与竞态 / Concurrency, Cancellation, Failure, Races

1. **控制顺序**：全局 config 文档写入需统一序列，因为不同插件也共写一个 `plugins.toml`。toggle、reload、discovery admission 与 runtime commit 使用同一控制顺序或有明确 operation token；不能只有 per-plugin 锁而丢失别人的配置更新。
2. **锁顺序**：P0 画出 control/load_guard → 短 config/registry/operation guard → loader 的顺序；std mutex 不跨 await。不要持有 loader guard 等 owner lock，同时 stop 持有 owner lock 等 loader；guest 执行区不得回取 lifecycle lock。等待 activation、blocking job、watcher 时释放会被完成方需要的锁。
3. **单飞不是错误重试**：Starting 的并发调用 join 同一个结果；Disabled/Blocked 拒绝；Failed 仅显式 retry，不自动选择恢复策略。caller 超时只取消其等待，不令共享结果丢失；operation owner 必须收尾并唤醒全部 waiter。
4. **取消与 panic**：各 await 边界注入取消；`spawn_blocking` 返回前保留资源归属。operation panic/join failure 映射 Failed，部分 scope 逆序回滚；不能残留永远 Starting 或无人接收的完成结果。
5. **enable/disable**：disable 已持久化后，新的 activation 不得进入；旧 Starting 即使成功构建 VM 也只能因 token 失效被清理，不得发布 Ready。disable→enable 用新 operation/generation，不复用失效 token。
6. **reload/同 id 替换**：plugin id 不是 mount identity。旧 watcher/disposer/completion 必须比对 current instance + generation + mount identity；旧绑定只返回 stale，不能自动绑定新 export。
7. **停止安全**：`acquire_call` 返回 unit，仅是 admission linearization point。新调用拒绝和旧调用存活是两张独立证明；stop 失败保留 Failed/cleanup detail，禁止将 `DisposeReport` 的错误吞掉变 Stopped。
8. **持久化故障**：candidate 未保存绝不替换 live；save 完成后 caller 被取消，owner 仍须对已提交 desired reconcile，或留下明确可查询的失败/待协调状态。崩溃发生在 save 后发布前，重启从 durable desired 恢复，不宣称 exactly-once。
9. **读写竞态**：snapshot 可过时但不能授权；执行前再校验 Ready、generation、identity 和既有安全闸。读 catalog/diagnostic/facade 多次不得推进状态。
10. **清理幂等**：同一 scope 最多消费一次；旧 mount 的 no-op disposer可以成功，但不能据此证明当前实例已停止。需要验证实际模块 map、服务/entry 副作用已撤回且替代实例仍在。

## 10. 测试矩阵 / Effect-level Test Matrix

优先复用 `src/extension/runtime/wasm/tests/guest_fixture.rs` 的真实 guest 和 `src/extension/loader.rs` 的 mount identity 测试；`EMPTY_WASM_MODULE` 只能证明 load/unload，不足以证明 handler 调用。可增加小型有确定 payload 的真实 WASM fixture，禁止下载不固定远端产物。

用 `Barrier`、`Notify`、oneshot、受控 blocking gate 明确“启动已进入/调用已进入/清理已进入”；timeout 只是死锁上界，不是排序手段。不得以 sleep、只数 mock 调用次数、只 grep 到函数名为通过证据。

| 场景 / Case | 同步安排 / Control | 必须断言的效果 / Required effect |
|---|---|---|
| boot/ensure discovery + 重复 list/catalog/diagnostic/describe/resolve | 真 fixture、加载边界 observer | eligible VM 实际未 mount，generation 不变；无 service/handler 外泄；只读不会 Starting |
| disabled / NotEnabled / trust Blocked | 全部入口尝试真实请求 | 零 VM 创建，错误原因区分；durable false 未被读侧改写 |
| 首次真实调用 | owner activation → guest export | Starting 前不可调用；mount 完成才 Ready；返回 guest payload，不是 mock success |
| N 个并发首调 | 在 load 进入点 barrier 卡住 | 一个 current instance/一次 generation 分配；所有 waiter 得到同一结果，释放后均可按契约执行 |
| save failure | 可注入 atomic writer error，或可控不可写目标，避免只依赖 chmod | live/disk/runtime 不变；RPC/Hub/builtin 都失败，无 `{ok:true}` |
| save success + runtime failure | 缺失/损坏真实 wasm 或 mount step failure | desired 已保存、actual Failed；结构化 partial outcome；scope 无残余 |
| 双 toggle / 两插件共写 | 控制保存与 reconcile barrier | 完成顺序符合线性化，磁盘与 live 最终一致，无丢失更新 |
| Starting 时 disable/reload | load completion barrier 与新 operation | 迟到结果不 Ready；旧 VM 被清理；新实例未被旧 completion 影响 |
| 等待者取消/leader caller drop | join 前后取消，owner 继续 | 不重复启动、不永远 Starting，其他 waiter 收到结果；无泄漏 |
| guest in flight 时 stop | guest 执行 gate + dispose gate | 新调用被拒；旧调用资源存活/按约完成；实际 unload 晚于安全边界 |
| blocking job 取消/shutdown | 在真正执行后 drop async waiter | 不假装 job 已停；最终可观测清理或明确 Failed/保留资源 |
| 同 id 新旧 mount | 持有旧 binding、旧 disposer 后加载新实例 | 旧 binding 拒绝，新 guest 返回新 payload，旧 disposer 不移除新 VM |
| disposer failure/panic | 精确失败 step，后续清理可观察 | 继续逆序清理，报告残余；actual 不说 Stopped |
| Failed 显式 retry / generation overflow | 受控 retry / 极值 fixture | 新 generation、旧句柄失效；overflow 不改变现有 state/generation |
| 已存在 observer + readiness 变更 | 先订阅后 transition；模拟 lag | 确实看到 cold/starting/ready/stopping/failed，重新 snapshot 正确；无假 revision |
| mixed WASM / MCP / static 回归 | 既有 fixture 与 explicit eligibility | A 外插件保持原契约；Pending 不是 Cold，非依赖能力行为未误改 |
| B 才要求：canonical plugin 调用隔离 | 真实 registry/ToolService + 两插件/builtin | effect 真到 handler；停一个不影响其余；旧 lease、旧 handler 拒绝；不走第二 registry |
| B 才要求：长寿命 projection | 持有已有 facade/catalog/MCP adapter | start/stop/restart 后在线结果更新；不是重建整个测试对象后才正确 |

所有成功断言要同时问“被拒绝的一边”和“仍应继续的一边”；计数能辅助，必须同时验证实际 mounted identity、guest 输出、registry entry 与 projection 撤回。测试共享 `ALEPH_HOME`/全局 manager 时复用既有串行隔离，不污染用户真实数据。

## 11. 验证命令与资源门 / Verification Commands and Resource Gate

以下是**下一实施会话的候选命令，本文编写时未运行 cargo，不得当作 PASS 记录**。先用仓库约定的 Python 解释器（优先 `.venv/bin/python`，按 `docs/reference/STYLE.md`），检查工具存在；不为文档任务安装环境。

### 11.1 每次 cargo 前 / Before Every Cargo Invocation

```bash
cd .
/usr/bin/vm_stat
python qa/browser_live/check_memory.py --self-test
python qa/browser_live/check_memory.py
```

- 复用已有 `qa/browser_live/check_memory.py`，不另写一套有偏差的探针。macOS 从 **vm_stat 首行真实 page size** 计算，不假定 4096/16384；现有 available estimate = `(Pages free + Pages speculative + Pages inactive) * actual_page_size`，不是物理保证，也不叠加 compressed/wired。
- **可用内存 < 4 GiB（4294967296 bytes）时等待/不启动 cargo**；探测失败同样 fail-closed。exit 0 放行、1 阻止、2 探测错误。记录 page size、计数、字节数、时间与正在运行的 build。
- 重链接建议至少 10 GiB；不要并发启动多个 cargo 争抢同一 target/RAM。不得擅自 kill 其他共享会话进程；需用户授权才可清理他人资源。
- 若不足，先推进非编译 review/文档；可在下一个合理检查点复测，不无限 sleep/poll，不把资源不足当代码失败。持续不足时报告 blocked 与所需资源。

### 11.2 分层验证 / Layered Verification

每条命令分别经内存 gate；测试 worker 回传完整日志路径、exit code 与有效测试数量，防止 filter 0 tests 伪绿。

```bash
cargo fmt --all --check
cargo test -p alephcore --lib capability::provider::
cargo test -p alephcore --lib extension::loader::
cargo test -p alephcore --lib extension::lifecycle::
cargo test -p alephcore --lib extension::plugin_ops::
cargo test -p alephcore --lib tools::registry::
cargo test -p alephcore --lib capability::zahir_facade::
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
```

若 test 模块位置与 filter 不符，先列出测试再选准确名字；新增 A barrier 测试也必须真的被运行。不要因为某个旧测试编译通过就声称并发/取消已覆盖。

按 `docs/reference/DEVELOPMENT.md` 选择并记录最小可信验证集，不以 `cargo check` 代替 `#[cfg(test)]` 编译：

```bash
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
just test-shared
# Only if relevant desktop limbs change:
cargo check -p aleph-desktop-macos -p aleph-desktop-windows -p aleph-desktop-linux
just _stage-shell-placeholders
cargo clippy --workspace --all-targets
```

若 wire/shared 受影响运行 `just test-shared`；`interfaces/webchat/` 有任何改动需跑 `cargo test -p aleph-panel --lib`，Panel 出厂形态用 `just wasm`；不因本计划包含候选命令就盲跑不相关平台。新真实 WASM fixture 有构建工具缺失时记录 blocked，不改用 mock 声称 guest 测试通过。

最终执行 `git diff --check`、`git diff --stat`、`git status --short`，分离本轮文件与共享他人变更；新文件 untracked 时普通 diff/stat 不包含它们，须另做显式文件/no-index 检查。未运行/blocked 检查逐项写明，不能借用第一阶段历史测试当本轮通过。

## 12. 风险与待决策 / Risks and Decisions

| 问题 / Question | 推荐默认 / Recommended default | 必须停下询问 / Stop and ask when |
|---|---|---|
| A 是否必须完整模型工具调用 | 仅验证 runtime owner；B 延后单独包 | 用户验收要求 canonical plugin tool E2E；必须重新排 B 前置依赖 |
| 哪些 WASM eligible | 显式受控 tool-only；mixed 保留既有 eager 并注明原因 | 需要把 startup service/hook/context/memory/MCP 混合插件一并 lazy |
| Cold 如何表达 | 纯 ProviderState 增 Cold，PluginStatus/wire 精确投影 | 协议消费方无法表达且要求无兼容变更；不能默默映射 Loaded |
| 停止期间资源 | 复用已有 guard/Arc，先证明安全，再卸载 | 需要强制终止 blocking guest、立即物理卸载或无法证明 lifetime |
| 启动失败是否自动重试 | Failed + 明确错误，仅显式新请求/操作重试 | 要求后台自动策略、supervisor/scheduler（超出 A） |
| saved desired 与 actual 失败 | 保留 durable intent、结构化 partial outcome | 产品要求自动撤销 durable intent；先澄清崩溃/回滚语义 |
| config 读取损坏 | 基线 load 会 warn 后 origin fallback；本次记录风险，不暗改全局策略 | 新需求要求 fail-closed config corruption；需独立安全/兼容决策 |
| projection 缺失通知 | 接真实状态通知，lag 从 authority 重建 | 必须伪造 revision 才能工作；说明架构缺口，不伪造完成 |
| 全局 registry gating | 不把 `aleph/tools` 用作插件 handle | 发现实现只能停全局才能停插件，退回 per-entry B 设计 |
| 共享工作区并发修改 | 主 agent 协调文件独占，保留他人 diff | 同文件有未知 writer/不兼容改动、基线漂移改变契约 |
| 资源/fixture 缺失 | 无 cargo，推进只读；真实 fixture 缺失就 blocked | 长期 <4 GiB、工具缺失或全部合法路径均无法验证 |

已识别设计气味：`set_plugin_enabled` bool 吞 persistence/runtime failure；`unmount_inner` 的 Disabled 无法表达停止失败；`ensure_loaded` 对管理读侧仍 eager；metadata tool index 容易被误称为 callable。A 针对边界修复，不顺便重写所有 trust/settings writer、MCP lifecycle 或全部模块化路线。

## 13. 下一会话执行顺序 / Next-session Execution Order

1. 使用配套 prompt，核实工作目录、HEAD、status、共享 writer 与可用 subagent；先读项目约束和本计划，不直接开始写 Rust。
2. 主 agent 做 plan review；安排只读调查 worker 提交真实调用图、A eligibility、锁/取消风险、B 缺口，自己审核关键接口与 diff。
3. 冻结 P0 契约并建立 P0→P1→P2→P3→P4→GA→P6 任务 DAG，明确 P5/GB 未授权；若用户要完整 callable 则先调整依赖并确认。
4. 依次下发实现；接口稳定后才给 caller 适配和独立测试 worker，不能并发修改同文件。测试执行单一队列，每次 cargo 前测内存。
5. 审阅实际 diff，重点确认 candidate save→live publish→reconcile、Ready 时点、在途 lifetime、旧 identity 清理与共享全局 registry 未被插件状态控制。
6. 真实 fixture/barrier 验证 A，逐项记录 PASS/FAIL/BLOCKED/UNRUN；失败给出具体修复任务，不让低成本模型无新信息重复三次以上。
7. GA 通过后只做事实文档更新和报告，默认结束。B 不因“顺手很方便”自动开始；不自动 push，提交也须遵守用户当前授权。

## 14. 完成定义与负面清单 / Definition of Done and Negative Checklist

### A 验收清单 / A Acceptance

- [ ] HEAD 与 shared diff 已复核，第一阶段已提交的事实未被改写。
- [ ] discovery 与 runtime activation 确实分开；eligible cold WASM 在 boot/查询时无 VM。
- [ ] durable desired 唯一；candidate 保存成功后才替换 live；toggle/reconcile 不越序；失败到达 RPC/Hub/builtin。
- [ ] Cold/Starting/Ready/Stopping/Failed/Stopped 语义可观测，不用 Pending/Loaded 伪装。
- [ ] 明确调用通过 runtime owner 激活，单飞、身份与 generation 绑定、mount 完成才 Ready；权限不放宽。
- [ ] 在途 guest 存活与停止清理有独立证据；失败不报 Stopped；迟到 completion/disposer 不影响新实例。
- [ ] 真实通知/重建到达已存在的 observer；不伪造 canonical revision；broadcast 不被当 durable source。
- [ ] 实际执行了相应真实 WASM/barrier 测试和编译检查；未执行项不冒充通过。
- [ ] `aleph/tools` 全局 provider 保持 builtin 语义；producer 未挂载错误未被假成功替代。
- [ ] A/B 分开报告，B 是否授权/实施有明确记录；文档更新到实际状态。

### 必须随最终报告声明 / Required Negative Report

列出未做 B 的 canonical plugin producer/per-entry/long-lived projection、MCP lazy、Gateway/Terminal/Memory backend、sidecar、Rust dylib、Safe Replay、scheduler、跨 kinds；列出未覆盖的 mixed WASM、未运行测试/资源阻塞、未验证平台与物理立即卸载保证。不得宣称完整插件工具接入或生产就绪超出证据。默认不提交、不 push；若以后用户另行授权提交，报告实际 commit，不自动 push。

**本次文档交付状态 / This documentation delivery**：仅创建实施计划和续跑 prompt；未修改 Rust、未运行 cargo、未实施 A/B、未提交或 push。验证证据由交付报告单列，不预填测试通过。
