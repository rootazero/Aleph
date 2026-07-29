# Webhook 动态挂载表 + 保留路由守卫退休 (Webhook Dynamic Mount Table)

- 日期 / Date: 2026-07-30
- 状态 / Status: 设计已确认，待实现
- 来源 / Origin: 2026-07-29/30 webhook 入站接线轮（`c78b4e6ec`）的四项遗留
- 前置 / Predecessor: [2026-07-29-webhook-inbound-wiring-design.md](2026-07-29-webhook-inbound-wiring-design.md)

## 1. 背景 (Background)

上一轮把 channel webhook 入站接到了 gateway 的 HTTP 面上，并在终审时发现「接上一条断线会把同一个病搬到生命周期路径」——路由表是**启动期快照**，两个方向都会失同步。那一轮只做了「最小诚实」：

- 非 `Connected` 的通道回 503（端点还在，只是拒绝服务）；
- 运行时 `channel.start` / `channel.create` 回 `"restart_required"`（如实告诉调用方它还收不到）。

本轮把「最小诚实」换成**真正的动态挂载**，并顺手清掉它连带的三项遗留。四项一起做的理由是**它们是同一根因的不同外显**，分开做会互相打脸（见 §3 决策 D1）。

## 2. 现状核实 (Verified Findings)

以下全部经读码验证，非推测。

### A. 挂载点是一次性快照，注册表变化后无人重建

`src/bin/aleph-server/commands/start/builder/subsystems.rs:516-559` 在 `initialize_channels`
尾部遍历已启动通道、造 `Router`、`server.set_webhook_routes(...)`。
`GatewayServer::build_router()`（`src/gateway/server/mod.rs:735`）把它 `merge` 进去，
之后 `serve()` 期间 router **不可变**。

后果（上一轮终审记录）：

- `channel.stop` / `channel.delete` 回 `{"status":"stopped"}`，端点却照旧 200、照旧驱动 agent run
  ——路由持有自己那份 `Arc<GenericWebhookHandler>` 克隆（与 `WebhookChannel.handler` 无关），
  mount 也持有自己的 `broadcast::Sender` 克隆，于是 forwarder 唯一的退出条件
  `RecvError::Closed` **永远不可能触发**。`delete` 更糟：留下一个操作者以为已删除、
  却仍带密钥可用的认证端点。
- 运行时 `channel.create` 没有 HTTP 面。

### B. `RESERVED_ROUTE_PREFIXES` 的唯一消费者是「路径进路由表」这件事本身

`src/gateway/server/mod.rs:398`（清单）+ `:413`（`is_reserved_route`）。
全仓消费者只有一个：`webhook_receiver.rs:140` 的「配置 path 撞了网关路由 → skip」分支，
外加同文件的一条针对它自己的单测（`:842`）。

它存在的**唯一**理由是 operator 可写的 `path` 会进 axum 路由表，而 `Router::merge`
撞名即 panic。清单没有防漏守卫——将来往 `build_router()` 加路由却忘了登记，
症状是操作者机器上的 boot panic。

### C. webhook `path` 可遮蔽 Panel SPA 路径

Panel 是 `fallback_service(control_plane)`（`server/mod.rs:713`），
`control_plane` 只注册 `GET /` 与 `GET /{*path}`（`control_plane/server.rs:19-20`）。
真实路由优先于 fallback，故 `path = "/settings"` 会让 `POST /settings` 成为真实路由，
而 axum 0.8 的方法不匹配**不落回 Router fallback**（那要显式
`Router::method_not_allowed_fallback`）⇒ `GET /settings` 从「SPA 空壳」变 405。
`path = "/"` 同理。

`WebhookChannelConfig::validate()`（`interfaces/webhook/config.rs:57`）今天只查
`starts_with('/')`，拦不住这一类。

### D. 503 检查排在签名校验之前

`webhook_receiver.rs:216-237`：Step 0 查通道状态回 503，Step 1 才验签回 403。
未认证方由此能区分「通道未连接」与「签名错」——极小的状态 oracle。

### E. `restart_channel` 不走 `stop_channel` / `start_channel`

`channel_registry.rs:749-760` 直接调 `channel.stop()` + `channel.start()`。
任何「在 start/stop 上挂钩」的方案漏掉它，就会留一份**过期的 handler 克隆**在表里
——正是 A 里那个病的形状。这是本轮最容易漏的挂钩点。

### F. 收窄路径不会破坏任何 operator 的现存配置

`VERSION` = `26.7.21`，上一轮（`c78b4e6ec`）合入于 2026-07-30，**从未随版本发布**。
接线之前 `path` 字段对入站**零作用**（`WebhookReceiver` 在 `src/` 里零构造点），
出站走 `callback_url`。故不存在「今天能工作的非 `/webhook/` 路径」。

### G. matchit 允许静态路由与 catch-all 并存，且不 panic

`/webhook/foo` 与 `/webhook/{*rest}` 可同时注册，静态更具体者胜。
⇒ 收窄之后，**唯一**残留漏点是「将来有人在 `/webhook/` 下加网关路由」，
而它是**静默**偷走一个 webhook 通道的路径，不是 panic。只能用字面量扫描钉。

## 3. 决策 (Decisions)

| # | 决策 | 选择 | 理由 |
|---|------|------|------|
| D1 | 四项遗留分几轮做 | **一轮** | 它们是同一根因的不同外显：动态表让 §2.B 的 panic 失败模式**整体消失**（清单随之退休）、让 §2.C 的遮蔽**不可表达**、让 §2.D 的 503 分支退居纵深防御。分轮做会先写一个守卫再删掉它 |
| D2 | 挂载形状 | **`POST /webhook/{*rest}` 单条常量路由 + 共享 map** | 路由表变成**常量**（不随配置变），这正是运行时创建能生效的前提；operator 的 path 再不进 axum 路由表 ⇒ boot panic 失败模式消失 |
| D3 | 路径是否收窄到 `/webhook/` | **收窄，`validate()` 强制** | §2.F 证明零破坏。换来 §2.C 由构造解决 + §2.B 清单退休。代价是 `GET /webhook/x` 从 SPA 空壳变 405（无人依赖） |
| D4 | 表的所有者 | **`ChannelRegistry`** | 「挂载跟随注册表」的字面实现。挂钩点全在一个文件内，避免 `register_channel_plugins` 那种「十个副本＝十个可忘处」 |
| D5 | 重复 path 的裁决 | **`channel_id` 字典序小者胜；同 id 必刷新** | 与 main 上「排序后先到者胜」（`43fc94b36`）逐字等价，但不再依赖 `HashMap` 迭代序 ⇒ 路由归属跨重启确定，不是每次开机抛硬币 |
| D6 | `RESERVED_ROUTE_PREFIXES` | **删除**（含 `is_reserved_route` 与其单测） | D2 之后零消费者。R10「零消费者立即撤回，绝不为未来留口」 |
| D7 | 检查顺序 | **查表 → 验签 → 查状态 → 解析转发** | 未认证方拿到的恒是 403 / 404，区分不出通道死活 |

## 4. 设计 (Design)

### 4.1 `WebhookMountTable` —— 表本身

```rust
// src/gateway/webhook_receiver.rs
pub const WEBHOOK_ROUTE_PREFIX: &str = "/webhook";

pub struct WebhookMountTable {
    mounts: RwLock<HashMap<String, WebhookMount>>,   // key = 完整 path，精确匹配
}

impl WebhookMountTable {
    pub fn new() -> Self;
    /// false = 被拒（已 warn!，绝不 panic）
    pub async fn mount(&self, mount: WebhookMount) -> bool;
    /// 返回摘掉的条数
    pub async fn unmount_channel(&self, channel_id: &ChannelId) -> usize;
    pub async fn len(&self) -> usize;               // boot 日志用
}

// 构造器签名随之变化：
// 前: WebhookReceiver::router(mounts: Vec<WebhookMount>) -> Router
// 后: WebhookReceiver::router(table: Arc<WebhookMountTable>) -> Router
```

`WebhookMount` 字段不变（`handler` / `inbound` / `status` / `channel_id`），
但它的 doc comment 要改：`status` 不再是「唯一能阻止已停通道回话的东西」
（现在 `stop` 真的把路由摘了），降级为纵深防御，见 §4.2。

`mount()` 的拒绝条件，两条都 `warn!` + 返回 `false`：

1. `path` 不以 `/webhook/` 开头（故 `/webhook` 与 `/webhookx` 都被拒），
   或恰好等于 `/webhook/`（无子路径 ⇒ `{*rest}` 匹配不到）。
   ——`validate()` 已在更早处拦住（§4.4），这里是兜底：防未来第二个
   `WebhookHandler` 实现绕过 validate。与上一轮保留「缺前导 `/` 也 skip 不 panic」
   的理由同源。
2. 该 path 已被**另一个** `channel_id` 占用，且占用者 id 字典序更小。
   同 id → 直接替换（restart 刷新）；来者 id 更小 → 替换并 warn 指名两个 id。

### 4.2 派发端点

```rust
async fn webhook_endpoint(
    State(table): State<Arc<WebhookMountTable>>,
    uri: Uri, headers: HeaderMap, body: Bytes,
) -> impl IntoResponse
```

- **查表用 `uri.path()`**，不用 `Path` 提取的 wildcard ——避开百分号解码与 key
  不一致（key 就是配置里那个字面量）。
- **查到即把 `Arc` / sender / status 克隆出读守卫、在 `.await` 前放锁**
  ——`handler.handle()` 是 async，跨 await 持 `RwLock` 读锁会把一个慢 handler
  变成全表的写锁饥饿。
- miss → **404**。这是 `stop` / `delete` 之后的新语义（旧语义是 503）。
- 命中后：`verify()` 失败 → 403；状态 ≠ `Connected` → 503；`handle()` → 转发。
  503 分支现在只剩「通道自己迁到 `Error` / `Connecting`」这类不经 RPC 的自转情形；
  `try_read` 竞争时 **fail open** 的理由原样成立（瞬时写锁持有者不是通道已死的证据），
  注释保留。

### 4.3 `ChannelRegistry` 成为唯一咽喉

`ChannelRegistry::new()` 恒建一张空表（无 `Option`，无配置）；
`pub fn webhook_mounts(&self) -> Arc<WebhookMountTable>` 交出 Arc。

| 生命周期方法 | 动作 |
|---|---|
| `start_channel` | `channel.start()` 成功后，`webhook_handler()` 为 `Some` → `mount()` |
| `restart_channel` | 同上（**刷新**；§2.E 说明它为何必须单独挂钩） |
| `stop_channel` | `channel.stop()` 后 → `unmount_channel(id)` |
| `unregister` | → `unmount_channel(id)`（`channel.delete` 走这条） |
| `register` / `create_channel` | 插入前先 `unmount_channel(id)` ——未启动的替身不得继承旧端点 |

6 处挂钩全在 `channel_registry.rs` 一个文件内。

### 4.4 配置校验

`WebhookChannelConfig::validate()`：把 `path must start with '/'` 换成
「必须以 `/webhook/` 开头且带子路径」。于是错配的 path 在 `channel.start()`
就**报错**，而不是「启动成功、状态 Connected、耳朵是聋的」——那正是本轮主题
（advertised-but-disabled）。默认值 `/webhook/generic` 天然合规。

### 4.5 `GatewayServer`

- 字段 `webhook_routes: Option<Router>` → `webhook_mounts: Arc<WebhookMountTable>`（默认空表）。
- `set_webhook_routes(Router)` → `set_webhook_mounts(Arc<WebhookMountTable>)`。
- `build_router()` 里**恒定**一条 `.merge(WebhookReceiver::router(self.webhook_mounts.clone()))`。
- 删 `RESERVED_ROUTE_PREFIXES` + `is_reserved_route`（D6）。

**顺带消失的约束**：`set_webhook_mounts` 何时调用不再有时序要求（Arc 而非快照）。
上一轮「必须在所有 channel `start()` 之后收集」那条脆弱前提整体作废。

### 4.6 装配点与回执

- `subsystems.rs:516-559` 那整块收集逻辑 → 一行 `server.set_webhook_mounts(channel_registry.webhook_mounts())`
  + 一行按 `len()` 的 boot 日志。
- `handlers/channel.rs`：删 `needs_webhook_restart`（`:369`）与两处 `"restart_required"`
  分支（`:444` `channel.start`、`:735` `channel.create`），回执回归 `"started"` / `"created"`
  ——现在这是真的。

### 4.7 防漏守卫（§2.B 的替代物）

两条，各自都有真实失败模式可防：

1. `validate()` 的前缀检查（§4.4）——防「启动成功但聋」。
2. **源码守卫**：`include_str!("mod.rs")` 断言除 §4.5 那条常量外，`build_router()`
   里不出现第二处 `/webhook` 的 `.route(` / `.nest(`。理由见 §2.G：matchit 不 panic，
   所以这个漏点是**静默**的，只能用字面量扫描钉。

## 5. 测试 (Tests — TDD 红先行)

| 测试 | 断言 | 今天 |
|------|------|------|
| `runtime_mount_becomes_reachable_without_restart` | 空表 POST → 404；`mount()` 后 POST → 200 | **红**（路由表是启动期快照） |
| `stopped_channel_route_disappears` | `unmount_channel` 后 POST → **404** | **红**（今天 503） |
| `signature_is_checked_before_channel_status` | 未连接通道 + 坏签名 → **403** | **红**（今天 503） |
| `duplicate_path_keeps_smaller_channel_id` | 两个方向都测（先大后小 / 先小后大） | 红 |
| `same_channel_remount_refreshes_handler` | 同 id 重挂 → 新 secret 生效 | 红 |
| `path_outside_webhook_prefix_is_refused` | `mount()` 回 false，表保持空 | 红 |
| `validate_rejects_path_outside_webhook_prefix` | `/settings` / `/` / `/webhook/` 均报错 | 红 |
| registry 四钩 | `start` 挂 / `stop` 摘 / `restart` 刷新 / `unregister` 摘 | 红 |
| `panel_spa_paths_are_untouched` | `build_router()` 后 `GET /`、`GET /settings` 仍落 SPA | 绿（防回归） |
| `webhook_prefix_has_exactly_one_route` | 源码守卫（§4.7-2） | 绿（防回归） |
| 现有 HMAC 单测 | 原样 | 绿（防回归） |

**被替换的旧测试**：`reserved_path_is_skipped_not_panicked` /
`path_missing_leading_slash_is_skipped_not_panicked` → 并入
`path_outside_webhook_prefix_is_refused`；`reserved_route_matches_prefix_segments_only`
→ 随 `is_reserved_route` 一起删。

## 6. 行为变化（面向 operator）

| 变化 | 说明 |
|------|------|
| `path` 必须以 `/webhook/` 开头 | 违者 `channel.start()` 报配置错。§2.F：无人受影响 |
| `GET /webhook/x` 由 SPA 空壳变 405 | 路由是 POST-only。无人依赖 |
| `stop` / `delete` 后打入站由 503 变 404 | 端点真的不存在了，这是修复本身 |
| `channel.start` / `channel.create` 不再回 `restart_required` | 运行时创建真的能收了 |

## 7. 本轮不做 —— 记账 (Out of Scope, Recorded)

- **签名无时间戳 / nonce**：重放保护仍是入站去重 5 分钟窗口（`inbound_router/dedup.rs`）的
  副产品。与本轮四项无因果关系。
- **重复 path 的输家只有 `warn!`**：`channels.list` 里两个通道都仍报 `Connected`，
  其中一个是聋的——本轮主题的小残留。做彻底需要一个新的通道状态
  （"started but unmounted"），属独立一轮；不在这轮造新机制（P6）。
- **msteams 的 `ChannelFactory`** / **`src/gateway/webhooks/`（~46 KB agent-trigger 子系统）**：
  上一轮已记账，定性 CUT/DECIDE，不是 CONNECT。

## 8. 风险 (Risks)

| 风险 | 缓解 |
|------|------|
| 跨 `.await` 持表锁 → 慢 handler 饿死全表 | §4.2 明确「克隆出守卫、放锁再 await」；code review 项 |
| 漏挂 `restart_channel` → 过期 handler 克隆留在表里 | §2.E 单列；§5 的 registry 四钩测试各一条 |
| 删 `pub` 常量/函数破坏外部调用者 | 已 grep 全仓：消费者只在 crate 内（`webhook_receiver.rs` 一处 + 一条自测） |
| 收窄路径破坏现存部署 | §2.F 证明：接线从未发布，`path` 此前对入站零作用 |
| 表与注册表的耦合越层 | 两者同在 `src/gateway/`；表是注册表的投影，属 I/O 记账而非业务逻辑（R4 不涉） |
