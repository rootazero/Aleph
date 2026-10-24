# Aleph Phase 2B B8 — B8.0 只读审计记录

> 状态：B8.0 完成，等待 B8-A..B8-D 确认。**本文件为只读审计记录，未修改任何生产实现。**
> 起点：`git rev-parse --short HEAD` = `77938dc47`，`git status --short --branch` = `## main...origin/main`（工作树干净）。
> 历史确认：B7 = `cf5b94481`、B6 = `c67b351a6`、B5 = `6a2c77f3f` 均在当前 `main` 历史中。

---

## 0. 结论摘要

1. **B7 的 Feishu slice 是一个 provider 生命周期切片，不是 Capability。** 它没有 descriptor / lease / subscribe / project，Zahir facade 里没有 Feishu。
2. **B7 存在 3 个实证缺陷（不是"缺测试"，是行为错误）**：
   - `api_handle::publish` 无 generation/instance 栅栏 → 旧代 late completion 可覆盖继任者发布（`withdraw` 有栅栏，`publish` 没有，栅栏不对称）。
   - `ChannelRegistry::register` 替换时**不停止**旧实例（只 `unmount_channel`），旧实例被 forwarder 的 `Arc` 持有 → 旧 webhook 端口仍被占用 → 新实例 bind 失败。
   - **配置层没有 enabled 判定**：`[channels.feishu] enabled = false` 被静默忽略，通道照常启动。
3. **Gateway 侧没有 channel-state observer。** `ChannelRegistry` 对注册变化不发布任何事件；provider transition broadcast（容量 16）**没有任何 Feishu 消费者**。
4. **Feishu 没有任何 durable recovery 路径。** Feishu 目录内无 `SessionEventStore` / `append_batch` 写入；provider 广播与 api_handle 都自述"notification only"。
5. **AGENTS.md 中"全仓无生产 `ZahirFacade::new` 构造/挂载调用点"已过期。** 生产确实挂了：`src/bin/aleph-server/commands/start/mod.rs:269` → `ProjectionHost::mount_with_diagnostics` → `src/capability/projection_host.rs:545` `ZahirFacade::new(...)`。挂载面只含 Tool namespace，Feishu 不在其中。

---

## 1. 起点核对

```
pwd                                -> .
git rev-parse --show-toplevel      -> .
git rev-parse --short HEAD         -> 77938dc47
git status --short --branch        -> ## main...origin/main   (无其他输出 = 工作树干净)
git log --oneline --decorate -8
  77938dc47 (HEAD -> main, origin/main, origin/HEAD) merge: complete phase 2b B7 Feishu provider slice
  cf5b94481 feat: wire Feishu gateway provider lifecycle slice
  e1e25fd3f docs: strip machine-specific absolute paths from historical records
  57f1b7ef5 docs: drop machine-specific checkout paths from project guidance
  06e0bc027 docs: plan phase 2b B7 gateway provider slice
  c67b351a6 merge: complete phase 2b B6 boot activation
  3c3f60890 feat: activate eligible plugins during boot
  6a2c77f3f merge: complete phase 2b B5 capability lifecycle
```

B7 / B6 / B5 均在历史中；未执行 reset / pull / push / rebase / stash。

---

## 2. Feishu slice 调用图

### 2.1 关键文件

| 位置 | 角色 |
|---|---|
| `src/gateway/interfaces/feishu/mod.rs` | `FeishuChannel`（实现 `Channel`）、`FeishuChannelFactory`、生命周期、生命周期测试 |
| `src/gateway/interfaces/feishu/api.rs` | `FeishuApi`：`admit` / `is_admitted` / `drain`、入站准入（`calls` RwLock） |
| `src/gateway/interfaces/feishu/api_handle.rs` | 发布缓存：`publish` / `withdraw` / `get`（**派生发布，非 authority**） |
| `src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs` | 独立 axum server + `handle_webhook` |
| `src/gateway/interfaces/feishu/feishu_runtime/mod.rs` | ws 模式 runtime（`Drop` 在此） |
| `src/capability/provider.rs` | `ProviderLifecycle` / `ProviderHandle`（状态 + generation + transition 广播） |
| `src/gateway/channel_registry.rs` | `ChannelRegistry`：`register` / `start_channel` / `stop_channel` / `DesiredChannelState` / forwarder |
| `src/gateway/handlers/channel.rs` | `create_channel_from_config` / `handle_start` / `handle_stop` / `handle_list` / `handle_status` |

### 2.2 启动 / 停止 / 替换

**启动（boot）**
```
src/bin/aleph-server/commands/start/builder/subsystems.rs:456  create_channel_from_config
  -> src/gateway/handlers/channel.rs:469 create_channel_from_config
       -> ChannelConfig { id, channel_type, enabled: true, config }   // enabled 硬编码 true
       -> plugin::create(channel_type, channel_config) -> FeishuChannelFactory
  -> src/gateway/handlers/channel.rs:554 registry.register(channel)
       -> src/gateway/channel_registry.rs:320 register()
            -> webhook_mounts.unmount_channel(&channel_id)   // 只卸载路由，不停旧实例
            -> channels.insert(channel_id, Arc<RwLock<Box<dyn Channel>>>)
  -> src/gateway/channel_registry.rs:539 start_all -> :480 start_channel
       -> FeishuChannel::start()  src/gateway/interfaces/feishu/mod.rs:275
```

**FeishuChannel::start 的唯一 disabled 判定**（`src/gateway/interfaces/feishu/mod.rs:275-284`）：
```rust
async fn start(&mut self) -> ChannelResult<()> {
    match self.provider.state() {
        ProviderState::Ready => return Ok(()),
        ProviderState::Disabled => {
            return Err(ChannelError::NotConnected(
                "Feishu provider disabled/retired".into(),
            ))
        }
        _ => {}
    }
```
> `ProviderState::Disabled` 的**生产写入者不存在**——只有测试 `mod.rs:472 disabled_provider_refuses_start` 手工 `transition(ProviderState::Disabled)`。

**停止**（`src/gateway/interfaces/feishu/mod.rs:317-330`）：
```rust
async fn stop(&mut self) -> ChannelResult<()> {
    self.revoke();        // :95  先阻止新准入
    self.cleanup().await; // :109 abort webhook_handle / refresh_handle (:113)
    if matches!(self.provider.state(), ProviderState::Stopping | ProviderState::Failed) {
        self.provider.transition(ProviderState::Stopped)...
    }
    Ok(())
}
```
> 顺序正确（revoke → cleanup → dispose），但 `cleanup` 是 **abort**，不是 join（见 §4）。

### 2.3 发布缓存与 generation 栅栏

`src/gateway/interfaces/feishu/api_handle.rs`（表头自述："This cache cannot activate a provider. Live notifications are not a durable recovery source."）：

```rust
// :34  Called only inside ProviderHandle::with_ready_commit (provider -> cache).
pub(crate) fn publish(channel_id, api, config, provider, generation) {
    lock().insert(channel_id.to_owned(), Live { api, config, provider, generation });   // :43 无任何栅栏
}

/// An outgoing instance/generation cannot withdraw its replacement.
pub(crate) fn withdraw(channel_id, provider, generation) {          // :53
    if table.get(channel_id).is_some_and(|live|
        live.provider.same_instance(provider) && live.generation == generation) {       // :57 栅栏在此
        table.remove(channel_id);
    }
}
```
`get`（`:61`）经 `live.provider.acquire_call(live.generation)` 二次栅栏。

`ProviderHandle::with_ready_commit`（`src/capability/provider.rs:275-304`）只校验**该 provider 自身**的 `state == Starting && generation == expected_generation`，不感知发布表。

> **不对称栅栏 = 缺陷 D1**：旧实例（未被 stop）的 `start()` 迟到完成时，`with_ready_commit` 在旧 provider 上成功 → `publish` 无条件 `insert` → **覆盖继任者的发布条目**。`withdraw` 有栅栏而 `publish` 没有，说明作者只防了一半。

### 2.4 替换（replacement）

`src/gateway/channel_registry.rs:320-332` `register()`：
```rust
// A replacement instance has not started, so it owns no handler. Drop
// whatever the outgoing instance left mounted, or the route keeps
// serving with the old secret until someone happens to start the new one.
self.webhook_mounts.unmount_channel(&channel_id).await;   // :326 只卸载共享 webhook 路由表
let mut channels = self.channels.write().await;
```
> **缺陷 D2**：只卸载**共享** `WebhookMountTable`。但 Feishu **不使用**共享表——它自己 `tokio::net::TcpListener::bind` 起独立 axum server（`feishu_inbound/webhook_server.rs:29-31` `run_webhook_server`）。旧实例被 forwarder 的 `Arc` 克隆持有（`channel_registry.rs:1137-1148`）→ 旧 `FeishuChannel` 永不 drop → 旧 `axum::serve` task 永不 abort → **旧端口仍被占用** → 新实例 bind 报 `Address already in use`。ws 模式同理（`FeishuRuntime` 的 `Drop` 永不运行）。

---

## 3. "enabled" 判定盘点（全部 4 处，无一来自配置）

| # | 判定 | 位置 | 生产写入者 |
|---|---|---|---|
| 1 | `ProviderState::Disabled` | `feishu/mod.rs:278` | **无**（仅测试） |
| 2 | `DesiredChannelState{Running,Stopped}` | `channel_registry.rs:97-105` | `start_channel` / `stop_channel`（registry 自己的运行时意图） |
| 3 | `FeishuConfig::is_account_enabled` | `feishu/config.rs:205-214` | **无生产调用点** |
| 4 | `ChannelConfig.enabled` | `crates/aleph-slot-api/src/channel.rs:1190-1191` | 被 `handlers/channel.rs:479` 硬编码为 `true`，**从不被读取** |

> **缺陷 D3**：`[channels.feishu] enabled = false` → `resolved_channels` 仍然包含该通道 → 照常启动。`FeishuConfig` 未 `deny_unknown_fields`，该键被静默吞掉。用户在配置里写的禁用意图**没有任何效果**，且无任何告警。

---

## 4. Webhook handler 生命周期（stop 边界）

`src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs:29-36`：
```rust
pub async fn run_webhook_server(state: WebhookState, listener: tokio::net::TcpListener) {
    let app = Router::new()
        .route(&state.config.webhook_path, post(handle_webhook))
        .with_state(state);
    if let Err(e) = axum::serve(listener, app).await { ... }
}
```
* 无 `JoinSet`、无 per-request 计数、无 graceful shutdown signal。
* `cleanup()`（`feishu/mod.rs:109-113`）abort 的是**顶层 accept task**。
* abort `axum::serve` ⇒ in-flight handler future 被 drop ⇒ 客户端连接被切断 ⇒ 飞书侧看到失败并**可能重投**（`feishu/dedup.rs` 负责去重，但去重 ≠ 不丢）。
* 目前被证明的只有 **enqueue quiescence**：`api.drain()` 等的是 `calls` RwLock（`api.rs:99-105`），即"已准入消息已入队且不再新增"。**HTTP response 尾（200 JSON 写回）没有被 join**。

> 这正是 B8 plan "当前已知风险"第 1 条：不得把顶层 accept task 的 abort+await 写成完整 handler drain。

---

## 5. Observer 与 durable recovery

### 5.1 ChannelRegistry 没有 observer

* `ChannelRegistry` 对**注册变化不发布任何事件**：channel create / delete / register / unregister 路径内无 `publish` / `emit` / `broadcast`（已 grep `src/gateway/handlers/channel.rs` 与 `channel_registry.rs`，无命中）。
* 现存广播只有三处，全部**不是** channel-state observer：
  * per-channel inbound broadcast：Feishu `ChannelState::new(100)`（容量 100，**lag 即丢，无 resync**）；
  * registry `inbound_tx`（容量 1000）+ forwarder（`channel_registry.rs:1171` 对 `Lagged` 直接跳过）；
  * provider transition broadcast（`provider.rs`，容量 16，**Feishu 侧无任何订阅者**；`ProviderHandle::subscribe_changes` 的唯一生产消费者是 tools registry `src/tools/registry.rs:2631,2736`）。
* `ChannelLifecycleSnapshot`（`channel_registry.rs:1507`）**无 version / epoch / cursor**。
* `ChannelHealthMonitor`（`src/gateway/channel_health_monitor.rs`）是 **300s 轮询**，无 revision、无 live invalidation。
* `DesiredChannelState`（`:97-105`）是唯一的运行时"该不该跑"判定（`start_channel`/`stop_channel` 写，`health_states` 读）。

### 5.2 仓库里已存在的两个正确范式（可直接抄）

| 范式 | 位置 | 机制 |
|---|---|---|
| subscribe-before-snapshot + lag→invalidation | `src/capability/projection_host.rs:546-549`；`src/capability/facade.rs` `CapabilityChangeStream::invalidate_and_resync` | 先订阅再取快照；`Lagged` 只发 invalidation，**不重放** |
| durable SSOT + 落后方从日志 heal | `src/gateway/session_projector.rs` `MessageProjector`（`SessionEventObserver`）；`src/session/store.rs` `append_batch` | 广播满则**延后**，恢复时**从 `session_events` 重新读**（seq 缺口用 in-memory missed set 记录 + heal pass） |

> B8 plan 说"复用 `ChannelRegistry` 的 subscribe-before-snapshot 语义"——**该语义在 `ChannelRegistry` 里不存在**，它只存在于 capability facade。措辞需在 reference 文档里更正。

### 5.3 Feishu 没有 durable recovery

* `src/gateway/interfaces/feishu/` 全目录**无** `SessionEventStore` / `append_batch` / `SessionEvent` 写入点。
* `provider.rs` 自述 broadcast 是 "a notification tap, not a recovery source"；`api_handle.rs:3-8` 自述 "Live notifications are not a durable recovery source."
* 入站消息最终经 inbound router → 执行引擎 → `SessionEventStore` 落库，那是**会话内容**的事实源，**不是** Feishu provider/channel 生命周期的事实源。重启后 provider 状态仅由 config 重建，**不重放**错过的 Feishu 事件。
* Gateway 另有 `src/gateway/delivery_queue.rs` `DeliveryStore`（SQLite，出站重试），是**第三个**独立 store，与 provider 生命周期无关。
* `SessionService::emit_batch` 默认 `Err`，唯一生产 override 是 `InProcessActorSessionService`（`src/session/in_process.rs`）。

---

## 6. Zahir facade：Feishu 若要成为 live Capability，缺什么

### 6.1 现有契约（已核实）

| 契约 | 位置 | 对 Feishu 的含义 |
|---|---|---|
| `CapabilityKind` 9 变体 | `src/capability/descriptor.rs:24-34` | 只有 `Tool` 可执行（`descriptor.rs:5-12` 文档） |
| `CapabilityDescriptor` 9 必填字段 | `descriptor.rs:95-105` | **没有 route / endpoint / handler 字段** → 入站 webhook 无 descriptor 表达 |
| `to_descriptor` | `src/capability/backend.rs:104-129` | 硬编码 `namespace = TOOL_NAMESPACE`、`kind = Tool`、`lifetime/owner = Runtime` → 非 Tool 无法经它产出 |
| `BackendLease` | `src/capability/facade.rs:60-71` | `canonical_entry: pub(crate) crate::tools::registry::CanonicalEntryLease` → **lease 类型绑定 tools registry** |
| `Zahir::resolve` | `src/capability/zahir_facade.rs:344` | 硬编码 `namespace != TOOL_NAMESPACE` → 非 Tool namespace 直接不可 resolve |
| `OwnershipTree` | `src/capability/ownership.rs:253` | 生产中**唯一** owner map：plugin owner / hook registry / dangerous_tools / builtin_registry / tools registry 共用 → 用它**不会**产生第二份 owner map |
| `EffectClaim` | `src/capability/effect_claim.rs` | 无生产 producer（仅 `effect_claim.rs:43,56,69,82` 的 adapter match arm + 测试夹具）。Feishu 入站 POST 是外部 effect → 需要 producer，而 B8 明令禁止 |

### 6.2 三个硬阻塞

1. **route 无处安放**：Feishu 入站路由的事实源是 `FeishuConfig.webhook_path` + host/port（独立 axum server，`webhook_server.rs:29-31`），**不在**共享 `WebhookMountTable` 里。descriptor 里没有 route 字段 → 要放进去只能新增字段或新增一份**拷贝**（形状 #1「同一事实的两份表述」）。
2. **lease 类型绑定 tools registry**：`CanonicalEntryLease` 是 tool registry 的 capture，Feishu 无从产出 → 要么改契约，要么造第二套 lease。
3. **EffectClaim producer 缺失**：入站 webhook 投递是外部 effect；`reconcile_effect_claim` 是 fail-closed 的，没有 producer 就永远没有 claim。

### 6.3 一个可抄的"部分采纳"先例

`src/extension/hooks/registry/`：Hook kind（非 Tool）用**同一个** `OwnershipTree`，`register_if_absent` + `OwnerRef::Runtime` + `LifetimeScope::Runtime` + `VisibilityScope`，**但不发 descriptor / lease / publication**。
→ 这正是"有 owner authority、无第二份 owner map、但不进 Zahir 描述面"的现成模式。

### 6.4 结论

**GO/NO-GO = NO-GO（当前）**。缺 4 项契约：① 非 Tool 的 descriptor 产出路径；② 非 tool-registry 的 lease 类型；③ 入站 route 的单一事实源安放位置；④ EffectClaim producer（且 B8 禁止引入）。

---

## 7. 文档事实纠正

| 位置 | 陈述 | 实际 |
|---|---|---|
| `AGENTS.md` "Capability Phase 4 入口" | "**全仓无生产 `ZahirFacade::new` 构造/挂载调用点**（grep 确认仅 `src/capability/mod.rs` 模块声明）" | **已过期**。`src/bin/aleph-server/commands/start/mod.rs:269` 调 `ProjectionHost::mount_with_diagnostics` → `src/capability/projection_host.rs:545` `ZahirFacade::new(registry.clone(), Arc::clone(&tree))`；另 `src/executor/builtin_registry/mod.rs:410`。挂载面只含 Tool namespace（Feishu 不在其中），但"无生产挂载"这句必须改。 |
| `docs/research/aleph_modularization_phase2b_b8_plan.md` "Observer" | "复用 `ChannelRegistry` 的 subscribe-before-snapshot 语义" | `ChannelRegistry` **没有**该语义；它在 `capability/projection_host.rs:546-549` + `facade.rs` `invalidate_and_resync`。 |

---

## 8. GAPS（按 plan 四语义轴）

| 轴 | 现状 |
|---|---|
| outcome | 已有 provider 状态机与 `DesiredChannelState`；缺 disabled 真实来源、缺 boot/enable race 与 late-completion 防护 |
| publication | `publish` **无栅栏**（D1）；`withdraw`/`get` 有栅栏 → 栅栏不对称 |
| resource | 替换不停止旧实例（D2）→ 旧端口/旧 ws task 泄漏；stop 只 abort 不 join |
| observer | Feishu provider 状态**零订阅者**；broadcast 容量 16 落后即丢且无 resync 消费方；无注册变化事件 |
| durable | Feishu **无** durable 写入 → 无 provider 级恢复；`SessionEventStore` 仅为会话内容事实源 |
| 读侧隔离 | ✅ 未发现读路径隐式启动：`channels.list` / `channels.status` / `health_states` 只读；`api_handle::get` 走 `acquire_call`（非 Ready 即 None）；`send` 走 `admit_call`（非 Ready 即 Err） |

---

## 9. 本次审计未做

* 未运行任何 `cargo` 命令（memory gate `python qa/browser_live/check_memory.py` 尚未执行）。
* 未跑 `qa/channels/run.sh` 真实 Feishu QA，未构建 `aleph-server`。
* 未修改任何生产代码、测试或 reference 文档（本文件除外）。
* 未验证 `websockets` Python 依赖是否存在于 `qa/channels` 裸环境。
* 三个子代理均在写出终稿前超时；本文件的结论均已由主 Agent 逐条复验（§2/§3/§4/§5/§6 的引文行号已直接 grep 核对），子代理仅提供检索广度。
* 未审计 `src/harness/`（B8 禁止改动，且不在本次范围）。
