# 阶段 4 Spec — 能力 (Capabilities) 右栏 Slot

> **任务边界**:本文件是**探索 + 方案**,**不**写实际重构代码。落地属于阶段 4 实施,待用户独立批准。
>
> **系列位置**:[阶段 0 总览(2026-10-15-rightside-optimization-plan.md)](2026-10-15-rightside-optimization-plan.md) · [阶段 1 Slot 接口(2026-10-15-rightpane-stage1-slot-spec.md)](2026-10-15-rightpane-stage1-slot-spec.md) · [阶段 2 宿主切换(2026-10-15-rightpane-stage2-host-switching-spec.md)](2026-10-15-rightpane-stage2-host-switching-spec.md) · [阶段 3 子代理 slot(2026-10-15-rightpane-stage3-subagent-slot-spec.md)](2026-10-15-rightpane-stage3-subagent-slot-spec.md) · **阶段 4 本文件**

---

## §0 TL;DR

- **后端完整就绪,UI 完全空缺**:`ProjectionHost` 通过 `mount_with_diagnostics(registry, tree, true)` 在 `src/bin/aleph-server/commands/start/mod.rs:269` 挂载;`wait_until_ready()` + `set_projection_host(...)` + `startup_diagnostics_control(...)` 三步就位。`grep -rn 'ProjectionHost\|ZahirFacade\|DiagnosticControl' interfaces/` 全仓 0 处真使用(只有 LLM provider 的同名 `capabilities` 字段,不同事实)。
- **9 种 capability kind,只 Tool 可执行**:其余 8 种 deferred;`EffectClaim` reducer 已 committed,**producer integration 明确 deferred**(AGENTS.md Tier 1 入口标注),右栏本阶段**只能做只读面板**,不能挂写入路径。
- **3 个本阶段事实源**:
  - 同步快路径:`host.current_snapshot()` → `Option<HostSnapshot>` (`projection_host.rs:737`),含 entries / cursor / closed / owner_generations
  - 同步诊断:`host.diagnostic_status()` → `DiagnosticStatus` (`projection_host.rs:844`),含 lifecycle / pending_depth / replacement_count / lag_count / applied_* 等
  - 异步流:`host.attach(scope) → ProjectionHandle` (`projection_host.rs:781`),`recv() → Option<ProjectionEvent>`(`Snapshot | Change | Invalidated`)
- **本阶段核心动作**:在右栏注册 `rps.capabilities` slot(见阶段 1 spec `§5`),消费 `current_snapshot()` + `attach(scope)` 双源,渲染**只读 tool 列表 + 选中后 metadata drawer**;diagnostic 计数面板放在 drawer 内 advanced 区,默认折叠。
- **3 个待拍板决策** S4-A / S4-B / S4-C,见 §3.1。

---

## §1 Aleph 现状审计(已读 8 个 capability/ 文件 + mount 点)

### §1.1 capability 子系统文件清单(全仓单一真源)

| 文件 | 行数 | 角色 | 关键导出 |
|---|---|---|---|
| `src/capability/descriptor.rs` | 105 | 数据 | `CapabilityDescriptor`, `CapabilityKind`(9 variants), `CapabilityId`, `ToolDescriptor`, `Owner` |
| `src/capability/backend.rs` | 226 | 数据 | `ToolBackendAdapter`(包 `ToolHandlerRegistry`), `TOOL_NAMESPACE="aleph/tools"`, `snapshot_capabilities` |
| `src/capability/facade.rs` | 440 | 数据 | `Zahir` trait, `CapabilitySnapshot`, `BackendLease`, `ResolveError`(闭集), `CapabilityChange`, `CapabilityChangeStream`(纯值状态机) |
| `src/capability/ownership.rs` | 1896 | 数据 | `OwnershipTree`, `OwnerGeneration`, reconcile `register_if_absent` |
| `src/capability/effect_claim.rs` | 469 | 数据 | 6 状态 `Prepared/Claimed/Invoking/Succeeded/Failed/Unknown`,reducer committed,**producer deferred** |
| `src/capability/projection_host.rs` | 3575 | host | `ProjectionHost`, `HostSnapshot`, `ProjectionEvent`, `ProjectionHandle`, `mount`, `mount_with_diagnostics`, `attach`, `current_snapshot`, `diagnostic_status` |
| `src/capability/zahir_facade.rs` | 971 | 唯一 concrete facade | `ZahirFacade::new(registry, tree)`, `reconcile_registry_bindings`, `map_registry_change` |
| `src/capability/diagnostic_control.rs` | 615 | narrow read-only | `DiagnosticControl`, `DiagnosticStatus`, `DiagnosticError`, `DiagnosticLifecycle` |
| `src/capability/mod.rs` | 517 | 注册 | `CapabilitySlot` 模式, `ALL_SLOTS`(53 entries), `projection_host_slot()` 在最末 |
| `src/capability/census.rs` | 3546 | census | (本次未读) |

### §1.2 9 种 capability kind(`descriptor.rs:31-72`)

```
Tool, Storage, Network, Memory, Process, Channel, Secret, Llm, Plugin
```

**`Tool` 是唯一带 concrete backend 的 kind**(`ToolBackendAdapter` + `ToolHandlerRegistry`),其它 8 种在 `descriptor.rs` 占位但 deferred。右栏本阶段**只**展示 Tool entries;其余 8 种在 `CapabilityKind` 出现但不渲染(`filter` 掉)。

### §1.3 后端 mount 时序(start/mod.rs:265-290)

```
1. ALEPH_CAPABILITY_DIAGNOSTICS env var → diagnostics_enabled_from_env
2. OwnershipTree::new() → Arc
3. ProjectionHost::mount_with_diagnostics(registry, Arc::clone(&tree), diagnostics_enabled)
4. alephcore::capability::projection_host::set_projection_host(Arc::clone(&host))
5. host.wait_until_ready().await  ← 阻塞直到 default consumer 真正 applied 一个 snapshot
6. startup_diagnostics_control(host, tree, diagnostics_enabled)?
```

**关键事实**:
- mount 是**懒**的 — `ZahirFacade::new` 不扫 registry;首次 `current_snapshot()` / `attach()` 触发 facade lazy snapshot
- 同一份 `(registry, tree)` 共享给 `ZahirFacade` 和 `ProjectionHost`,**无第二份 owner map**(per memory + spec 防判据 §10)
- `set_projection_host` 是 process-global slot(`projection_host.rs:73-76`),写一次,`projection_host()` 全局读
- `startup_diagnostics_control` 仅在 `diagnostics_enabled` 为真时返回 `Some`,诊断面板**默认开**(由环境变量决定)

### §1.4 3 个事实源详表

#### §1.4.1 同步快路径:`host.current_snapshot() -> Option<HostSnapshot>`

`projection_host.rs:737`,签名:`#[must_use]`,**无副作用读**。

```rust
pub struct HostSnapshot {                              // projection_host.rs:99
    pub entries: Arc<HashMap<String, RegistryEntry>>,  // tool 名 → 描述符
    pub registry_cursor: Cursor,                       // 同步冻结的 registry cut
    pub registry_closed: bool,                         // 同 cut 的 close 标志
    pub owner_generations: HashMap<CapabilityId, OwnerGeneration>,
}
```

- 读**真实 applied state**(post applier drain),不读 publisher seed
- closed source 返回 `None`(fail-closed)
- 两个 cut(registry + owner)独立,不联合 atomic — 跨 authority 顺序由 `subscribe-before-snapshot` 路径保证

#### §1.4.2 同步诊断:`host.diagnostic_status() -> DiagnosticStatus`

`projection_host.rs:844`,**无副作用**。

```rust
pub struct DiagnosticStatus {                          // diagnostic_control.rs
    pub lifecycle: DiagnosticLifecycle,                // Active / Closing / Closed
    pub registry_cursor: Cursor,
    pub pending_depth: usize,                          // default consumer 待处理事件数
    pub pending_capacity: usize,                       // 同上容量
    pub replacement_count: u64,                        // 累计 tool 替换数
    pub lag_count: u64,                                // 累计 broadcast lag 数
    pub applied_tool_ids: Vec<CapabilityId>,
    pub applied_owner_generations: HashMap<CapabilityId, OwnerGeneration>,
    pub applied_invalidation_count: u64,
    pub applied_replacement_count: u64,
    pub last_replacement_registry_cursor: Option<Cursor>,
    pub last_replacement_tool_ids: Vec<String>,
}
```

- `DiagnosticLifecycle` 由 host-local `closed` bit + shared completion outcome 共同推导
- `pending_depth` 在 UI 端可显示"事件积压 X/Y"
- `lag_count` 在生产环境是有用诊断:非 0 = 慢消费方

#### §1.4.3 异步流:`host.attach(scope) -> ProjectionHandle`

`projection_host.rs:781`,**有副作用**(在 host 内部注册 consumer)。

```rust
pub struct ProjectionHandle {                          // projection_host.rs:1093
    host: Arc<HostInner>,
    id: u64,
    consumer: Arc<ConsumerState>,
    cancel: CancellationToken,
}

impl ProjectionHandle {
    pub async fn recv(&mut self) -> Option<ProjectionEvent>  // :1104
    pub fn cancel(&self)                                     // :1122
    pub async fn close(self)                                 // :1130
}

pub enum ProjectionEvent {                            // projection_host.rs:130
    Snapshot(HostSnapshot),
    Change(CapabilityChange),                          // {Registered, Replaced, Unregistered, Invalidated}
    Invalidated,                                       // "下一个 event 是新 Snapshot"
}
```

- 每个 attach **独立有界队列**(`DEFAULT_PENDING_CAPACITY=64`)
- 满队列**丢弃最旧 + enqueue `Invalidated` + 新 `Snapshot`**(replaceable,不是 `tokio::mpsc` 丢最新)
- 同一 scope 可多 attach(每 attach 独立 cursor)
- handle 必须显式 `close()` 或 `cancel()`(在 Leptos `on_cleanup` / TUI drop 时)

### §1.5 UI 现状 — **0 处真使用**

```bash
grep -rn "ProjectionHost\|ZahirFacade\|DiagnosticControl\|DiagnosticStatus" \
     interfaces/webchat/src interfaces/tui/src
# → 0 hits

grep -rn "capabilities" interfaces/webchat/src interfaces/tui/src
# → 9 hits,全部为 LLM provider 的 GenerationType / ModelCapabilities
#   (interfaces/webchat/src/preset_providers.rs:34,65,70,78
#    interfaces/webchat/src/api/providers.rs:14 注释
#    interfaces/webchat/src/platform/wide/views/settings/generation_providers/{picker,detail_view,add_custom,preset_setup,model_ladder}.rs
#    interfaces/tui/src/tui/widgets/provider_picker.rs:250,364)
# → 与 capability 子系统**无关**,是同名不同物
```

**结论**:整条 capability 子系统(后端 mount 已就绪)对用户**完全不可见**。这是右栏可以低成本接入的核心理由 — 不需要"打破已有 UI 习惯",只需要把已有的真源**接一根线**到右栏。

### §1.6 EffectClaim producer 的实际边界(关键红线)

- `effect_claim.rs` 6 状态 + reducer `reconcile_session_events` 已 committed
- **无 `SessionEvent::EffectClaim*` 构造点**(全仓 grep 确认)
- AGENTS.md Tier 1 入口明示:EffectClaim producer integration deferred
- `DiagnosticStatus` 没有 effect_claim 字段(`replacement_count`/`lag_count` 是 host 层 counter,**不**含 effect claim)
- **右栏本阶段不能挂任何 effect-claim 写入路径**(既没有也无意义)

### §1.7 痛点(为什么进右栏)

1. **后端真源对用户不可见**:用户看到的是"MCP 服务器接好了"(`tool_registry_phase2.subscribe()` 日志),看不到"当前 MCP 工具列表 / owner generation / 健康度"。
2. **MCP 调试靠 stdin/stdout**:`src/bin/aleph-server/commands/start/mod.rs:295-304` 注释明示"Logs every MCP-driven register/unregister so operators can see exactly when remote tools enter or leave the LLM's surface" — 这是 operator-level 诊断,**用户**也需要看一眼。
3. **Owner generation 概念用户不感知**:`apply()` / `migrate()` 触发的 `OwnerGeneration` 递增是后端概念,但 ownership conflict 排查时必须能看见"工具 X 现在 owner 是第几代"。
4. **Diagnostic counters 零可视化**:`pending_depth` / `lag_count` 在 `DiagnosticStatus` 里但 UI 无出口 — host 闭锁时,用户没有"为什么慢"的线索。

---

## §2 右栏 Capabilities Slot — 目标与不变量

### §2.1 目标(可测)

| # | 目标 | 验证方式 |
|---|---|---|
| 2.1.1 | 用户在右栏 1 次点击内看到当前 `HostSnapshot` 全 tool 列表 | 视觉:启动后 5s 内 `current_snapshot` 触发首次 paint;count = `applied_tool_ids.len()` |
| 2.1.2 | 用户看到 tool 行的 4 元信息(name / schema kind / owner generation / fresh/stale) | 视觉:每行 4 字段;`stale` 通过 `lifecycle == Closing/Closed` 触发 |
| 2.1.3 | Tool 新增 / 替换 / 注销在 1s 内反映在右栏 | E2E:MCP 端 `register_tool` → 1s 内右栏出现新行 |
| 2.1.4 | 选中 tool 后,右侧 metadata drawer 显示 schema + 选中时的 owner generation + 最近一次 replacement 时间 | 视觉:点击行 → drawer 滑出,字段齐全 |
| 2.1.5 | Drawer 底部 advanced 区默认折叠,展开后看到 `pending_depth` / `replacement_count` / `lag_count` / `last_replacement_tool_ids` | 视觉:折叠态只占 1 行;展开后 4 字段全显示 |
| 2.1.6 | TUI 端共享同一份事实源(同一 `ProjectionHost` 实例,通过 `projection_host()` 全局拿) | 编译:与 webchat 端共用 `alephcore::capability::projection_host` API,无第二份订阅 |

### §2.2 不变量(必须守住,R10 同判据)

| # | 不变量 | 来源 |
|---|---|---|
| 2.2.1 | **全仓单一真源** | `ProjectionHost` + `ZahirFacade` 已是单一;slot 不再新建 `ToolHandlerRegistry` 副本 |
| 2.2.2 | **只读面板,零写路径** | EffectClaim producer deferred;本阶段**不**新增 `bump_runtime` / `revoke_tool` / `dispose_runtime` / `hold` / `release` UI 入口 |
| 2.2.3 | **handle 生命周期必显式 close** | `ProjectionHandle::cancel()` 必在 Leptos `on_cleanup` / TUI `Drop` 触发,否则 consumer 在 host 内部泄漏 |
| 2.2.4 | **attach 一次,组件 re-mount 不重复 attach** | 阶段 1 spec §2 的 `RightPaneRegistry` 保证 slot 生命周期 = 组件 re-mount 周期(非 page navigation 周期) |
| 2.2.5 | **scope 单一**:slot 用 `Scope::Public`(`aleph_protocol::subagent_tree::Scope` 等价物,见阶段 1 spec §3) | 不混用 `Scope::Session`(本阶段 capability 不分用户/会话) |
| 2.2.6 | **TUI 端走和 webchat 完全不同的渲染层,但共享同一 `ProjectionHost` 实例** | TUI `projection_host()` 全局拿;不可在 TUI 端另起 facade |
| 2.2.7 | **diagnostic 字段**不**通过 snapshot 暴露** | `current_snapshot()` 走纯值 `HostSnapshot`;diagnostic 走 `diagnostic_status()`,两个 API 严格分;不在 snapshot 中混 |

### §2.3 与现有右栏 slot 的关系

| 现有 slot(阶段 1 spec §5) | 关系 |
|---|---|
| `rps.subagent_tree` | 互补;subagent slot 显示**正在跑的任务树**,capability slot 显示**这些任务能调用的工具** |
| `rps.artifacts` | 不重叠;artifact 是任务产物,capability 是能力声明 |
| `rps.browser` | 不重叠 |
| `rps.approvals`(未来) | **强相关** — 审批请求常带"调工具 X"上下文,审批 drawer 拟从 capability slot 借 schema 显示(见阶段 1 spec §5.5 placeholder) |

### §2.4 slot id 锁定(已用 阶段 1 spec §6 约定)

- `rps.capabilities` — 唯一 slot id
- 渲染位置:右栏 Tab 条中(默认**非**默认打开 — 见 §3.1 S4-A)
- 文件:`interfaces/webchat/src/components/right_pane/slots/capabilities.rs` + 同名 Leptos 组件,Panel 与 TUI 各一份(共享 `alephcore` 数据源,无第二份订阅)

---

## §3 决策点(已批,见决策日志 §9)

### §3.1 3 个已批决策

| ID | 决策 | 选项 | 推荐 |
|---|---|---|---|
| **S4-A** | Capabilities slot 位置 | ✅ **A2** 次级展开(已批) | m00491 |
| **S4-B** | Diagnostic 计数面板(advanced 区)可见性 | ✅ **B1** 默认折叠(已批) | m00491 |
| **S4-C** | Tool 详情 drawer 模式 | ✅ **C1** 内联 drawer(已批) | m00491 |

### §3.2 决策背后的 trade-off(供回退时查)

- **A1 vs A2**:A1 让 capability 跟 subagent 平起平坐,但**用户工作主线**(对话 + artifacts)被挤掉 1 个 Tab 视觉位置;A2 把诊断视图收进"次级",更符合"右栏第一屏是产出,第二屏是机制"的信息层级
- **B1 vs B2**:B2 持续占用屏高,且对**非 dev 用户**是噪声;B1 通过 1 行折叠 + 展开后 4 字段,实现"0 噪声 / 1 键直达"
- **C1 vs C2**:C2 制造新路由(`/capabilities/{name}`)与现有 `/dashboard/subagents` 平行,触发判据 §1(同一事实的两份表述)风险;C1 内联 drawer 统一在右栏

---

## §4 战术改动(本阶段,**不**实施代码,只列方向)

### §4.1 新增文件(2,符合 R10 ≤ 2 新文件预算)

```
interfaces/webchat/src/components/right_pane/slots/capabilities.rs  # Panel 渲染
interfaces/tui/src/tui/widgets/right_pane/capabilities.rs          # TUI 渲染
```

### §4.2 既有文件触碰(3 处,均为 render 层,不改 capability 核心)

| 路径 | 改动 | 风险 |
|---|---|---|
| `interfaces/webchat/src/app.rs`(预计,阶段 2 已确定 mount 点迁此) | mount `<RightPaneSlot id="rps.capabilities" />` | 低;slot 自身惰性 |
| `interfaces/webchat/src/components/right_pane/mod.rs` | `registry.register(capabilities_slot())` 启动时调用 | 低;阶段 1 已设计 register API |
| `interfaces/tui/src/tui/regions.rs` | `RegionKind::RightPane(Capabilities)` 添加 variant(若阶段 7 决定 TUI 加右列) | 中;本阶段不实施,只留位 |

### §4.3 关键 API 调用模板(规格,非代码)

```
Panel 启动:
  let host = projection_host()             // 全局 Arc<ProjectionHost>
  let handle = host.attach(Scope::Public)  // 独立有界队列
  on_cleanup(|| handle.cancel())           // 显式 cancel

Panel 渲染:
  let snapshot = host.current_snapshot()   // 同步,首次 mount 后稳定
  for entry in snapshot.entries.values() {
    let name = entry.tool_descriptor.name
    let kind = entry.tool_descriptor.kind  // "Tool" 唯一可见
    let gen = snapshot.owner_generations[id_of(entry)]
    render_row(name, kind, gen, fresh_by_lifecycle)
  }

Panel 异步:
  while let Some(event) = handle.recv().await {
    match event {
      Snapshot(s) => state.replace(s.entries, s.owner_generations),
      Change(Registered { id, revision }) => add_or_replace_row(id, revision),
      Change(Replaced { id, new_revision, old_revision }) => update_row(id, new_revision),
      Change(Unregistered { id }) => remove_row(id),
      Invalidated => reload_snapshot(),  // 下一条一定是 Snapshot
    }
  }

Drawer advanced 区(展开时):
  let diag = host.diagnostic_status()
  render("Pending: {diag.pending_depth}/{diag.pending_capacity}")
  render("Replacements: {diag.replacement_count}")
  render("Lags: {diag.lag_count}")
  render("Last replacement: {diag.last_replacement_tool_ids.join(', ')}")
```

### §4.4 与 effect_claim 的边界

- **不读** `EffectClaim` 任何字段(producer deferred,无数据)
- **不写** 任何 effect_claim 相关 SessionEvent(本阶段**零**写路径)
- `DiagnosticStatus` 暴露的 `applied_*` / `replacement_*` 全部是 **host 内部 counter**,与 effect claim 概念**正交** — 用 `DiagnosticStatus` 不会"提前消费"未实现 effect_claim 状态

### §4.5 TUI 端简化(若阶段 7 决定 TUI 加右列)

- TUI `RegionKind::RightPane(Capabilities)` 仅显示 tool 名 + kind 缩略字符 + owner generation 数字
- Drawer advanced 区降级为 `pending_depth` / `lag_count` 2 行(`replacement_count` / `last_replacement_*` 太宽,屏外)
- handle `recv()` 走 `tokio::select!` + `tokio::sync::Notify`(Ratatui 主 loop tick 驱动)

---

## §5 验收(对齐阶段 1 spec §10 / 阶段 2 spec §8 / 阶段 3 spec §10)

### §5.1 单元测试(`alephcore` 端,`capability::projection_host::tests` 已存在,本阶段追加)

- UT-1:`rps.capabilities` slot 注册后,`host.attach(Scope::Public)` 返回的 handle 立刻可 `recv()` 到 `ProjectionEvent::Snapshot`
- UT-2:`handle.cancel()` 后 `recv()` 返回 `None`,且 host consumer map 中该 id 已被移除
- UT-3:同一 scope 两次 attach 收到**独立** cursor(互不污染)
- UT-4:`current_snapshot()` 在 `mount_with_diagnostics(..., false)` 路径下与 `mount(registry, tree)` 路径**完全等价**(DiagnosticControl 不影响)
- UT-5:`HostSnapshot` 两个 cut 独立:替换 owner 不动 `entries`,新 register 不动 `owner_generations`,两者在 `subscribe-before-snapshot` 路径下顺序稳定
- UT-6:同一 `DiagnosticStatus` 字段在 `apply` / `replacement` 序列下单调性:`replacement_count` 不减,`lag_count` 不减
- UT-7:`EffectClaim` reducer 与 `DiagnosticStatus` 字段**零交叉**(grep 验证 + 编译零导入)

### §5.2 视觉回归(Playwright on webchat build)

- VS-1:启动后右栏打开 Capabilities tab,5s 内看到 ≥0 tool 行(数量 = `applied_tool_ids.len()`)
- VS-2:选中 tool → drawer 滑出 ≤ 200ms,4 字段齐(name / kind / gen / fresh)
- VS-3:advanced 区折叠态只占 1 行,展开后 4 字段无截断
- VS-4:Tool 替换序列(MCP `replace_tool`)在 ≤ 1s 内反映在行上,generation 数字 +1
- VS-5:host 关闭(单元测试触发 `close_and_await`)后,drawer 显示 `Lifecycle: Closing → Closed`,**不**再渲染 entry

### §5.3 端到端 smoke(走 ACP JSON / `mcp_tools_list` 真实路径)

- E2E-1:启动 aleph-server,打开右栏,看到默认内置 tool(`fs_read` / `shell` / 等)
- E2E-2:挂载 1 个 MCP server,新 tool 出现在右栏,generation = 0
- E2E-3:重启 MCP server(`apply_tool` 触发),行更新,generation = 1,`replacement_count` 计数 +1
- E2E-4:卸载 MCP server,行消失,`applied_tool_ids.len()` -1
- E2E-5:同步触发 100 次 `register_tool` + 100 次 `unregister_tool`,无 panic,`lag_count` 在合理范围(测试 1 consumer)
- E2E-6:TUI 端 `right_pane/capabilities` 区域在 TUI 启动后 ≤ 3s 首次 paint

### §5.4 性能预算

- `current_snapshot()` 单次调用 ≤ 100µs(无锁竞争;applied mutex 是 Mutex,非 RwLock,本阶段不优化)
- `attach()` 单次调用 ≤ 1ms(注册 consumer + clone Arc)
- 100 tool 全列表首次 paint ≤ 50ms(每行 4 字段,纯文本)
- handle `recv()` 稳态无事件时 ≤ 1ms 唤醒(`tokio::sync::Notify`)

---

## §6 风险与红线

### §6.1 R1–R10 红线(本阶段全部守住)

| 红线 | 本阶段兑现 |
|---|---|
| R1 Core 不调平台 API | slot 只读 `alephcore` 公开 API,无 platform 调用 |
| R2 UI 逻辑唯一源 | Leptos 组件,所有判断在 view;无平台特定逻辑 |
| R3 核心轻量化 | 不引新依赖(阶段 1 spec 已限定 `right_pane/` 内) |
| R4 Interface 层禁止业务逻辑 | 右栏 slot 是 I/O 视图层,无业务判断 |
| R5 AI 主动到达 | N/A(本阶段不涉及 AI 主动触达) |
| R6 一核多端 | TUI + Panel 共享 `alephcore::capability::projection_host` 同一 Arc |
| R7 LLM 主权 | 不替代 LLM 决策,只展示已有真源 |
| R8 工具即一切 | slot 本身是右栏 slot,配置由 `RightPaneRegistry` 暴露 |
| R9 智慧在 Prompt 中 | N/A(本阶段无 prompt 改动) |
| R10 薄 Harness | 新增 2 文件(Panel + TUI 渲染),≤ 2 预算内 |

### §6.2 本阶段特有风险

| ID | 风险 | 缓解 |
|---|---|---|
| R-S4-1 | `current_snapshot()` 与 `recv()` 两条路径的 snapshot **可能不一致**(registry cut vs owner cut 独立) | slot 主渲染走 `current_snapshot()`,流事件用其自带 snapshot;不混 |
| R-S4-2 | `attach()` 不显式 close → consumer 在 host 内部泄漏 | 阶段 1 spec 已强制 slot 必走 `on_cleanup` cancel;本 spec 加 UT-2 |
| R-S4-3 | `pending_depth` 在面板渲染时拍快照,drawer 展开时已 stale | drawer 重新 `diagnostic_status()` 读,不带缓存 |
| R-S4-4 | `EffectClaim` producer 当前已延期，本阶段 drawer 不得预先加入 producer 分支 | 阶段 4 只渲染已存在的 diagnostic/tool 事实；EffectClaim producer 与对应 UI 另行批准后再扩展 spec |
| R-S4-5 | Tool schema 含 **大 JSON**(MCP 工具 schema 平均 5–20 KB),drawer 内渲染会卡 | drawer 走折叠渲染(默认 1 行 summary,展开后全 JSON,带 monospace + scroll) |
| R-S4-6 | 9 种 capability kind 中除 Tool 外 8 种 deferred → 列表实际只有 Tool,用户疑惑 | header 明示 "Tools (1 kind active, 8 deferred)" |

### §6.3 已知不做(显式 not-do)

1. **不**新增 effect claim 写入路径
2. **不**暴露 `bump_runtime` / `revoke_tool` / `dispose_runtime` UI 入口(本阶段纯只读)
3. **不**实现 Tool 详情"调一次试试"按钮(属阶段 1+ 写入路径)
4. **不**做 Tool schema 全文搜索(右栏不放搜索框;搜索是阶段 1 后引入的全局能力)
5. **不**做 per-user / per-session scope 隔离(capability 现阶段是 process-global)
6. **不**做 capability 反向链接到 message / subagent(避免 §1 判据 1 的两份表述)
7. **不**做 i18n 字符串硬编码(走 i18n 框架,与阶段 3 一致)
8. **不**复制 ChatGPT / Codex 任何视觉元素(仅功能性参考)
9. **不**在 TUI 端复刻 drawer advanced 区的 `last_replacement_tool_ids` 全列表(TUI 屏宽不够)

---

## §7 阶段路线(本 spec 自身)

| 步骤 | 范围 | 验收 |
|---|---|---|
| 7.1 决策 S4-A/B/C 用户拍板 | 3 决策 | 用户回复 |
| 7.2 写 `RightPaneSlot` for capabilities 骨架(纯渲染,stub 数据) | 2 文件 | `cargo check` |
| 7.3 接 `current_snapshot()` 同步快路径 | 1 文件 | 启动后 5s 内 paint |
| 7.4 接 `attach()` 异步流 | 1 文件 | MCP register/unregister 1s 内反映 |
| 7.5 drawer 内联 + advanced 折叠 | 1 文件 | VS-2 / VS-3 通过 |
| 7.6 TUI 端简化版(若阶段 7 决定) | 1 文件 | E2E-6 通过 |
| 7.7 验证矩阵 5.1–5.4 全跑 | 全套 | UT/VS/E2E/性能 100% |

**不**实施任何代码 — 上述步骤是**实施 spec**,待用户独立批准 §3 决策后启动。

---

## §8 file:line 速查(本阶段)

### §8.1 capability 子系统(全仓单一真源)

| 路径 | 角色 |
|---|---|
| `src/capability/descriptor.rs:31-72` | 9 capability kind 枚举 |
| `src/capability/descriptor.rs:74-105` | `CapabilityDescriptor` 结构 |
| `src/capability/backend.rs:34` | `TOOL_NAMESPACE="aleph/tools"` |
| `src/capability/backend.rs:130-180` | `ToolBackendAdapter::snapshot_capabilities` |
| `src/capability/facade.rs:42-87` | `Zahir` trait(5 必填方法) |
| `src/capability/facade.rs:147-185` | `BackendLease { descriptor, owner_generation }` |
| `src/capability/facade.rs:190-220` | `ResolveError` 闭集 |
| `src/capability/facade.rs:230-310` | `CapabilityChange` 4 变体 |
| `src/capability/facade.rs:340-380` | `CapabilityChangeStream` 纯值状态机 |
| `src/capability/ownership.rs:180-280` | `OwnershipTree` 构造 + reconcile |
| `src/capability/ownership.rs:1200-1500` | `register_if_absent` + `apply` + `migrate` |
| `src/capability/effect_claim.rs:80-140` | 6 状态 enum |
| `src/capability/effect_claim.rs:340-380` | reducer `reconcile_session_events` |
| `src/capability/projection_host.rs:70-80` | `set_projection_host` / `projection_host()` |
| `src/capability/projection_host.rs:99-127` | `HostSnapshot` 结构 |
| `src/capability/projection_host.rs:130-160` | `ProjectionEvent` 3 变体 |
| `src/capability/projection_host.rs:393-410` | `DiagnosticPlane` |
| `src/capability/projection_host.rs:509-517` | `ProjectionHost` 结构 |
| `src/capability/projection_host.rs:519-525` | `mount(registry, tree)` |
| `src/capability/projection_host.rs:527-625` | `mount_with_diagnostics(...)` |
| `src/capability/projection_host.rs:700-735` | `wait_until_ready` |
| `src/capability/projection_host.rs:737-762` | `current_snapshot` |
| `src/capability/projection_host.rs:781-810` | `attach(scope)` |
| `src/capability/projection_host.rs:844-1000` | `diagnostic_status` |
| `src/capability/projection_host.rs:1005-1090` | `diagnostic_release` |
| `src/capability/projection_host.rs:1093-1100` | `ProjectionHandle` 结构 |
| `src/capability/projection_host.rs:1100-1120` | `recv()` |
| `src/capability/projection_host.rs:1122-1130` | `cancel()` |
| `src/capability/projection_host.rs:1130-1145` | `close(self)` |
| `src/capability/diagnostic_control.rs:60-180` | `DiagnosticStatus` 字段 |
| `src/capability/diagnostic_control.rs:200-300` | `status()` 读法 |
| `src/capability/diagnostic_control.rs:320-400` | `bump_runtime` / `revoke_tool` / `dispose_runtime` |
| `src/capability/diagnostic_control.rs:420-520` | `hold` / `release` / `close` |
| `src/capability/zahir_facade.rs:120-200` | `ZahirFacade::new` + `reconcile_registry_bindings` |
| `src/capability/zahir_facade.rs:380-460` | `snapshot_entries_in_scope(scope)` |
| `src/capability/zahir_facade.rs:520-600` | `map_registry_change` |
| `src/capability/mod.rs:380-440` | `ALL_SLOTS` 53 entries |
| `src/capability/mod.rs:480-510` | `projection_host_slot()` |

### §8.2 mount 时序

| 路径 | 行 | 角色 |
|---|---|---|
| `src/bin/aleph-server/commands/start/mod.rs:13` | import | `ProjectionHost, ProjectionShutdownOutcome` |
| `src/bin/aleph-server/commands/start/mod.rs:70-71` | struct 字段 | `host: Arc<ProjectionHost>`, `tree: Arc<OwnershipTree>` |
| `src/bin/aleph-server/commands/start/mod.rs:260-265` | env | `ALEPH_CAPABILITY_DIAGNOSTICS` → `diagnostics_enabled_from_env` |
| `src/bin/aleph-server/commands/start/mod.rs:266` | alloc | `OwnershipTree::new()` |
| `src/bin/aleph-server/commands/start/mod.rs:269` | mount | `ProjectionHost::mount_with_diagnostics(registry, tree, enabled)` |
| `src/bin/aleph-server/commands/start/mod.rs:272` | global | `set_projection_host(Arc::clone(&host))` |
| `src/bin/aleph-server/commands/start/mod.rs:273-275` | wait | `host.wait_until_ready().await` |
| `src/bin/aleph-server/commands/start/mod.rs:281-285` | diag | `startup_diagnostics_control(...)` |
| `src/bin/aleph-server/commands/start/mod.rs:295-304` | mcp log | `tool_registry_phase2.subscribe()` |
| `src/bin/aleph-server/commands/start/mod.rs:4011` | shutdown | `host.close_and_await()` |

### §8.3 UI 零使用证据(grep 命中点均为 LLM provider capabilities,非本 spec 真源)

| 路径 | 含义 |
|---|---|
| `interfaces/webchat/src/preset_providers.rs:34,65,70,78,109,251` | LLM provider GenerationType |
| `interfaces/webchat/src/api/providers.rs:14` | 注释,说明 provider schema |
| `interfaces/webchat/src/platform/wide/views/settings/generation_providers/*.rs` | 6 处 LLM provider 设置 |
| `interfaces/tui/src/tui/widgets/provider_picker.rs:250,364` | TUI provider 选择 |

---

## §9 决策日志(本 spec)

| ID | 决策 | 状态 | 来源 |
|---|---|---|---|
| S4-A | Capabilities slot 位置 = A2 次级展开 | ✅ **A2** | 用户批 m00491 |
| S4-B | Diagnostic 计数面板 = B1 默认折叠 | ✅ **B1** | 用户批 m00491 |
| S4-C | 详情 drawer = C1 内联 | ✅ **C1** | 用户批 m00491;与 S3-C 一致,避免路由分裂 |

**继承自上游 spec 的已批决策**(不重复拍板):
- 3.1.1 = A(flex 第三列)
- 3.1.2 = trait(可注册)
- 3.1.3 = subagent 默认 Tab,capability 次级
- S2-A/B/C = 阶段 2 容器过渡 + ChatOnly 分支 + tool API 直读
- S3-A/B/C = subagent 默认 Tab / 分 2 slot / 内联 drawer
- S4-A/B/C = capability 次级展开 / diagnostic 默认折叠 / 内联 drawer(本 spec,2026-10-15 拍板)

---

## §10 引用

- **上游 spec**:
  - [2026-10-15-rightside-optimization-plan.md](./2026-10-15-rightside-optimization-plan.md) — 阶段 0 总览
  - [2026-10-15-rightpane-stage1-slot-spec.md](./2026-10-15-rightpane-stage1-slot-spec.md) — 阶段 1 slot 接口
  - [2026-10-15-rightpane-stage2-host-switching-spec.md](./2026-10-15-rightpane-stage2-host-switching-spec.md) — 阶段 2 宿主切换
  - [2026-10-15-rightpane-stage3-subagent-slot-spec.md](./2026-10-15-rightpane-stage3-subagent-slot-spec.md) — 阶段 3 子代理 slot
- **AGENTS.md**:`Capability Phase 4 入口` 段(`AGENTS.md:155-200` 区域) — EffectClaim producer deferred 的事实源
- **FEATURE_LOCATOR.md**:§3.5d `docs/reference/FEATURE_LOCATOR.md:6080` — capability 机制详述
- **shared 真源**:`alephcore::capability::projection_host` (`src/capability/projection_host.rs`) — `ProjectionHost` / `HostSnapshot` / `ProjectionEvent` / `ProjectionHandle` 全部公开

---

*Last updated: 2026-10-15 · 阶段 4 spec v1.1 · S4-A=A2 / S4-B=B1 / S4-C=C1 已批*
