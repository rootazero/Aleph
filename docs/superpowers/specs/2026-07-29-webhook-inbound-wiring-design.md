# Webhook 入站接线 + /stop 回执真机验证 (Webhook Inbound Wiring & /stop Receipt QA)

- 日期 / Date: 2026-07-29
- 状态 / Status: 设计已确认，待实现
- 来源 / Origin: 2026-07-29 busy-queue 车道轮的两项遗留

## 1. 背景 (Background)

上一轮（busy-queue 车道准入修复，`e81e5281b`）留下两项：

1. **`/stop` 回执数字没能真机验证。** `handle_stop` 里 `busy_queue::purge()` 的返回值拼进
   回执文案（`Msg::QueuedMessagesDropped { count }`），但这条路径只存在于 channel 入站
   （`src/gateway/inbound_router/command_handler.rs:373`），`chat.send` 到不了。它与该轮
   测试 2 读同一份车道快照（`purge()` 返回的就是 `total_waiting` 里那些），同源同因，
   但**只有单测覆盖**。
2. **`WebhookReceiver` 与 `src/gateway/webhooks/` 在 `src/` 里零构造点。** generic webhook
   channel 只能发不能收 —— 这也是本机没有任何可模拟 channel 入站路径的原因，即 (1) 无法
   真机验证的根本原因。

**关键因果**：(2) 是 (1) 的唯一前置。接通 webhook 入站后，`curl` 打入站 + 本地 sink 收出站
即可让整条 `/stop` 回执链在本机完全可观测。

## 2. 现状核实 (Verified Findings)

以下全部经 grep / 读码验证，非推测。

### A. webhook channel 报 Connected 但耳朵是聋的

`WebhookChannel` **在工厂表里**（`src/gateway/interfaces/mod.rs:129`，2026-07-26 那轮补的），
`start()` 成功、状态置 `Connected`、造出 `GenericWebhookHandler` 存进 `self.handler`
（`src/gateway/interfaces/webhook/mod.rs:178-190`）。但：

- `WebhookChannel::webhook_handler()`（`mod.rs:146`）与 `inbound_sender()`（`mod.rs:138`）
  两个 getter 在 `src/` 里**零消费者**。
- `WebhookReceiver::new` 只在自己文件的 test 里出现过一次（`webhook_receiver.rs:539`）。

这是「能力真有、状态却在说另一件事」的镜像版 —— 比缺失更难发现，因为它**报告自己已连接**。
与 2026-07-26 的「adapter 完整但工厂表没登记」同族，但断点更靠后一层。

### B. 同一根断线还挂着 msteams（本轮不做）

`impl WebhookHandler for MsTeamsChannel` 存在（`src/gateway/interfaces/msteams/mod.rs:475`），
但 msteams **连 `ChannelFactory` 都没有**，也不在 `register_channel_plugins` 里 —— 它比
webhook 断得还靠前一层。

### C. 另有一个完全独立的死子系统（本轮不做）

`src/gateway/webhooks/`（`config.rs` / `handler.rs` / `hmac.rs` / `template.rs`，约 46KB）是给
GitHub / Stripe 那种「外部服务触发 agent」用的，与 channel 层无关。`create_router` /
`WebhookProcessor` / `WebhooksConfig` **只在 `src/gateway/mod.rs:188-190` 被 `pub use`，别处
零引用**；`[webhooks]` 配置段在 `src/config/` 里**根本不存在**（`Config` 只有
`channels: HashMap<String, serde_json::Value>`，`src/config/structs.rs:208`）。

这块**从来没接过**，不是「接了又断」—— 定性上属于 severed-wire 三分法里的 CUT/DECIDE，
不是 CONNECT。

### D. Gateway 已有一台成熟的 axum server

`src/gateway/server/mod.rs:670` 构建 `/ws` `/health` `/ready` `/metrics` + artifact 路由 +
a2a 路由 + admin 路由，带 `SecurityHeadersLayer`、Origin 策略、TLS。

而 `WebhookReceiver::start` 自己 `bind(([0,0,0,0], port))`（`webhook_receiver.rs:140`）——
**另起一个 0.0.0.0 监听**，与「默认只绑 `127.0.0.1`，`[gateway] host = "0.0.0.0"` 才显式
开放局域网」的信任模型正面冲突。这是接线时必须一并消掉的真实安全缺陷。

### E. 时序与先例都已具备

- `GatewayServer` 构造 @ `start/mod.rs:185` → `initialize_channels` @ `:2480` →
  `run_until_shutdown` @ `:2828`。收集 handler 的窗口充裕。
- `set_a2a_state` / `set_admin_router`（`server/mod.rs:519` / `:525`）正是「bootstrap 期设
  `Option` 字段、serve 时在 `build_router()` 里 merge」的现成形状，直接复刻。

### F. QA 所需的 busy_input_mode 会被真正读到

`subsystems.rs:710-752` 的通用循环覆盖 imessage / telegram 以外**所有** channel type，
把 `ChannelPolicyConfig`（含 `busy_input_mode`）注册进 `inbound_router`。所以
`[channels.webhook_qa] busy_input_mode = "queue"` 不是又一根断线，QA 场景成立。

### G. HTTP 路由没有速率闸（姿态基线）

`rate_limiter` 只活在 `MiddlewareChain`（JSON-RPC / WS 派发路径），**不是 HTTP layer**。
现有 `/health` `/metrics` artifact a2a 都无速率限制。新增 webhook 路由与它们同姿态。

## 3. 决策 (Decisions)

| # | 决策 | 选择 | 理由 |
|---|------|------|------|
| D1 | 本轮范围 | **A（接通入站）+ QA（/stop 真机红绿）** | A 是 QA 的唯一前置；B / C 与 `/stop` 无因果关系，混进来是把一轮变成三轮的风险叠加 |
| D2 | 入站 HTTP 面挂载点 | **并入现有 gateway router** | 一个端口、一份 TLS、一层 SecurityHeaders、自动尊重 `[gateway] host`；顺手消掉硬编码 `0.0.0.0`；有 `set_admin_router` 现成先例 |
| D3 | webhook 路由安全姿态 | **不加额外闸** | HMAC 即认证，与 `/health` `/metrics` `/a2a` 同层；`[channels.webhook]` 配置即开关，`validate()` 已强制 secret 非空；不为单一功能造新机制（P6） |

## 4. 设计 (Design)

### 4.1 `Channel` trait 上加一个默认 `None` 的能力问句

```rust
// src/gateway/channel.rs
fn webhook_handler(&self) -> Option<Arc<dyn WebhookHandler>> { None }
```

**为什么是 trait 方法而非在 `initialize_channels` 里特判 `WebhookChannel`**：registry 存的是
`Box<dyn Channel>`，没有 `Any` 就没法向下转型 —— 这是类型系统逼出来的。而它正好是
「声明即接线」的形状：下一个 webhook 型 channel（msteams）只要覆写这个方法就自动接上，
不会重演 `register_channel_plugins` 那种「十个副本＝十个可忘处」。

`WebhookChannel` 现有的固有方法 `webhook_handler()` 返回
`Option<Arc<GenericWebhookHandler>>`，改为 trait impl 并擦除为 `Arc<dyn WebhookHandler>`。

### 4.2 `WebhookReceiver`：自带 server → 造 Router

```
前: start(&mut self, handlers: Vec<Arc<dyn WebhookHandler>>, mpsc::Sender<InboundMessage>)
    → 自己 bind 0.0.0.0:port，自己持 shutdown_tx

后: WebhookReceiver::router(mounts: Vec<WebhookMount>) -> Router
    WebhookMount { handler: Arc<dyn WebhookHandler>, inbound: InboundMessageSender }
```

- `port` / `shutdown_tx` 两字段删除 —— listener 与 shutdown 归 gateway 拥有。
- HMAC 助手 `compute_signature` / `verify_signature` **原样保留**：`GenericWebhookHandler`
  与 `interfaces/webhook/message_ops.rs` 是它们的真实消费者，现有 HMAC 单测一并保留。

**⚠️ 接线时一并修掉的静默坑**：原 `start()` 收的是 `mpsc::Sender<InboundMessage>`，而
channel 体系走的是 `ChannelState.inbound_broadcast`（`InboundMessageSender`，
`channel.rs:642`）。照原签名接上去，消息会**绕过 `ChannelRegistry::start_message_forwarder`**
（`channel_registry.rs:587`）—— 那正是给每条入站消息盖 `health.record_event()` 的唯一地方，
`ChannelHealthMonitor::is_stale` 读它。后果会是：webhook channel 能收消息，健康监控却认定
它一直是死的。故新接口收 `InboundMessageSender`，从 channel 自己的 broadcast 进，让
forwarder 正常工作。

### 4.3 装配点

```rust
// src/gateway/server/mod.rs —— 完全复刻 admin_router / a2a_state 的形状
webhook_routes: Option<Router>,
pub fn set_webhook_routes(&mut self, r: Router) { self.webhook_routes = Some(r); }
// build_router(): if let Some(w) = self.webhook_routes.clone() { router = router.merge(w) }
```

收集点在 `src/bin/aleph-server/commands/start/builder/subsystems.rs::initialize_channels`
尾部：遍历已 `start()` 的 channel，取 `webhook_handler()` 为 `Some` 者，配对该 channel 自己的
`state().sender()`，造 Router，`server.set_webhook_routes(...)`。没有任何 webhook 型 channel
时不调用 —— `webhook_routes` 保持 `None`，路由表与今天逐字节相同。

### 4.4 路径冲突守卫（必须有）

axum `Router::merge` 遇重复路由**直接 panic**，而 `path` 是 operator 可写的配置字段
（`WebhookChannelConfig.path`，默认 `/webhook/generic`）。`path = "/ws"` 会让 daemon 起不来。

挂载前按保留前缀去重：`/ws` `/health` `/ready` `/metrics` `/v1/admin` 以及 artifact / a2a
前缀，外加已挂 webhook 路径之间的互相去重。冲突者 **skip + `warn!`，不 panic**。保留前缀
清单与 `build_router()` 里的实际路由**同文件**，避免两份清单漂移。

## 5. 测试 (Tests — TDD 红先行)

| 测试 | 断言 | 今天的状态 |
|------|------|-----------|
| `webhook_channel_handler_reaches_router` | 配 webhook channel → 收集 → 造 Router → 签名 POST → `InboundMessage` 出现在 registry 入站流 | **必红**（收集这一步根本不存在） |
| `reserved_path_is_skipped_not_panicked` | `path = "/ws"` 不 panic、被跳过、留 warn | 必红 |
| `duplicate_webhook_paths_deduped` | 两个 webhook channel 配同一 path，第二个被跳过而非 panic | 必红 |
| `no_webhook_channel_means_no_route` | 未配置时 `webhook_routes == None`，路由表与今天相同 | 应绿（防回归） |
| 现有 `WebhookReceiver` HMAC 单测 | 原样 | 应绿（防回归） |

## 6. /stop 回执真机 QA

**配置**

```toml
[channels.webhook_qa]
type = "webhook"
secret = "<qa-secret>"
callback_url = "http://127.0.0.1:<sink_port>"
path = "/webhook/qa"
busy_input_mode = "queue"
```

**Sink**：本机一个记录 POST body 的极小 HTTP server（出站回执的观测点 ——
`WebhookChannel::send` 是 POST 到 `callback_url`）。

**红（接线前）**：签名 POST 打进 `/webhook/qa`，sink 一个字节都收不到 —— 这本身就是断线的
运行时证据，正是本轮的红。

**绿（接线后）**

1. POST msg1 → 起一个长跑 run。
2. POST msg2 / msg3 → 因 `busy_input_mode = "queue"` 进 per-session FIFO 车道。
3. POST `/stop`。
4. 断言 sink 收到的回执文案里计数 **= 2**。

**负例**：无排队时 `/stop` 的回执**不带**计数后缀。

## 7. 本轮不做 —— 记账 (Out of Scope, Recorded)

- **B · msteams**：`impl WebhookHandler` 有，但连 `ChannelFactory` 都没有、不在工厂表 ——
  比 webhook 断得靠前一层。§4.1 的 trait 方法为它预留了接线口，但补工厂 + 补配置类型是
  独立一轮的工作量。
- **C · `src/gateway/webhooks/`**：~46KB 的 GitHub / Stripe agent-trigger 子系统，从未接过、
  `[webhooks]` 配置段不存在。大概率 CUT，但那是 DECIDE 级判断（要先确认没有产品意图），
  单开一轮。

## 8. 风险 (Risks)

| 风险 | 缓解 |
|------|------|
| 路径冲突让 daemon boot panic | §4.4 保留前缀守卫 + 专项测试；保留清单与实际路由同文件 |
| 新 HTTP 路由扩大攻击面 | D3 已定姿态：与 `/health` `/metrics` `/a2a` 同层；配置即开关，secret 强制非空 |
| trait 加方法波及全部 channel 实现 | 默认 `None`，现有实现零改动 |
| QA 场景依赖长跑 run 的时序 | msg1 用一个足够长的任务；断言前轮询 sink 而非固定 sleep |

## 9. QA 结果 (Real-Machine QA Results, 2026-07-29)

真机执行，macOS 27 / Darwin 27.0.0。RED = `2b822e187`（Task 1 之前），GREEN = `409dc6eb8`（Task 5 之后）。
两个 binary 必须各自独占 `CARGO_TARGET_DIR`：仓库根 `.cargo/config.toml` 把所有 worktree 钉到
同一个 `target/`，两次 build 互相覆盖，且 cargo 对另一个 worktree 报 `Fresh alephcore` —— 共享
target 结构上装不下两个 binary。

| 场景 | 期望 | 实测 |
|------|------|------|
| RED · 接线前签名 POST | 连不上 / 404，sink 空 | **PASS**。`HTTP/1.1 405 Method Not Allowed` + `allow: GET,HEAD`，body 空（SPA 兜底只注册 GET/HEAD，POST 打到未匹配路径由 axum MethodRouter 回 405）。`sink.log` 0 行。boot 日志有 `Registered channel: webhook (webhook)` / `✓ Channel webhook started`，**没有** `webhook ingestion route(s) mounted` —— channel 报 Connected 而聋，正是断线形状 |
| GREEN · 入站到达 | POST 200，sink 收到 agent 回复 | **PASS**。boot 增出 `Gateway: 1 webhook ingestion route(s) mounted`。`[post] HTTP 200 body='ok'`，~14s 后 sink 收到真实 agent 回复：`{"conversation_id":"qa-conv",…,"message":"QA-HELLO-OK",…}` |
| GREEN · /stop 回执计数 | 回执含计数 = 2 | **PASS**。sink 实测：`⏹ 已停止当前任务。 已随本次停止取消 2 条排队中的消息。`（`Msg::RunStopped` + `Msg::QueuedMessagesDropped{count:2}`，Locale::Zh）。服务端日志同刻：`/stop: cancelled running run, run_id=cc4b658a…` + `/stop: dropped queued messages…, dropped=2`，且被取消的正是 msg1 自己的 run —— 证明 msg1 当时确实在飞，两条后继确实还在等 |
| GREEN · 无排队时 /stop | 回执不带计数子句 | **PASS**。先经 `gateway.metrics.run_concurrency` 确认 `running_sessions=[] / total_waiting=0`，再 POST `/stop`，sink 实测：`当前没有正在执行的任务。`（`Msg::NoActiveRun`），**无**计数子句 —— 计数是实算不是常量 |

### 9.1 脚本必须对着代码改，不能照抄草稿

- **payload 文本字段是 `message` 不是 `text`**（`message_ops.rs::WebhookPayload`）。写 `text` 会
  反序列化失败、端点回 400。
- **签名头 / 格式与草稿一致**：`X-Webhook-Signature: sha256=<hex hmac-sha256>`。
- **必须自带 `message_id`**：缺省时 `derive_message_id` 退化成 **内容指纹**（sender + conversation
  + thread + message 的 SHA-256），于是跨场景重发的第二条 `/stop` 与第一条撞 id、被入站 dedup
  静默丢弃。QA 每条消息带唯一 `message_id`。
- **回执断言必须用中文**：真机 `[general]` 无 `language` 键 ⇒ `Locale::from_config(None)` ⇒ `Zh`。

### 9.2 QA 时序的两个真陷阱（都不是产品 bug）

1. **入站 coalescer 默认开着，`debounce_ms = 800`**（`coalescer.rs::CoalescingConfig::default`）。
   两条后继 POST 相隔 32ms 会被**合并成一条**，车道深度只到 1。后继消息必须相隔 > 800ms。
2. **`purge()` 只数「仍在等」的，不数已被提升为 running 的**。首轮 msg1 只跑了 11.4s，`/stop`
   到达前它已结束、队首被 `mark_admitted` 提升成 running，于是回执如实报 `dropped=1`。
   这是**正确行为**（CLAUDE.md §4.8：车道是候车室不是运行登记簿），不是缺陷 —— 断言 count=2
   必须让 msg1 在 `/stop` 落地时**仍在飞**。同理，中途插 `gateway.metrics` 轮询（每次 WS 往返
   1–2s）会把 `/stop` 推迟到 msg1 结束之后，反而测不到；改为紧凑发送、事后查日志对账。

### 9.3 QA 顺带发现的既有缺陷（非本轮 Task 1–5 引入）

**`WebhookChannelFactory::create` 硬编码 channel id `"webhook"`**（`interfaces/webhook/mod.rs:262`），
丢弃 `create_channel_from_config(id, …)` 传进来的实例 id。而 `subsystems.rs` 把 router 侧策略
注册在**配置段名**下（`register_channel_config(&inst.id, …)`），executor 又按**运行时 channel id**
查（`channel_run_identity` → `configs.get(channel_id)`）。两者只有在配置段恰好叫 `webhook` 时才对得上。

实测证据（配置段名为 `webhook_qa` 时的 boot 日志两行）：

```
Registered channel: webhook (webhook)
Inbound router: access tiering registered for 'webhook_qa' (webhook) [tier=guest]
```

后果：段名不等于 `webhook` 时，该 channel 的 `busy_input_mode` / `permission_level` /
`default_workspace` / `tool_permissions` / slash-access **全部静默失效**，回落默认值（busy 模式退回
`Steer`）。同族问题 whatsapp 已修 —— `subsystems.rs` 里那段注释写得很清楚：「The generic factory
hardcodes the id "whatsapp"; rebuild with the real instance id so the registry keys the channel
correctly and multi-instance configs are addressable」—— webhook 没有对应的重建分支。本轮 QA 的
绕法是把配置段命名为 `[channels.webhook]` 让两个 id 重合；**修复本身超出 Task 1–5 范围，单独记账**。

### 9.4 命令与脚本

RED / GREEN 各自独占 target dir 构建后，把 binary 拷出来再跑（共享 target 会互相覆盖）：

```bash
# RED（detached worktree @ 2b822e187）与 GREEN 各构建一次，binary 立刻拷走
CARGO_TARGET_DIR=<独立目录> cargo build --bin aleph-server

python3 sink.py 8788                                   # 出站观测点
./aleph-server-{RED,GREEN} --config aleph_qa.toml start # 注意 --config 是全局 flag，在子命令之前
python3 post.py "<text>" "<unique-message-id>"
```

首条消息会撞 pairing 墙（`dm_policy` 默认 `Pairing`，回执带配对码）。走产品自己的审批链，
**没有放宽任何访问控制**：

```bash
aleph-server gateway call --url ws://127.0.0.1:8787/ws channel.pairing.list    -p '{"channel":"webhook"}'
aleph-server gateway call --url ws://127.0.0.1:8787/ws channel.pairing.approve -p '{"channel":"webhook","code":"<code>"}'
```

车道深度用 `gateway.metrics.run_concurrency` 的 `busy_queue.total_waiting` / `per_session[].depth`
直读（实测到过 `{"depth": 2, "session_key": "agent:main:peer:dm-qa-user"}`）。

`sink.py` / `post.py` / `aleph_qa.toml` 均为一次性脚本，跑在 scratch 目录，**未入库**。QA 配置由
真实 `~/.aleph/config.toml` 复制而来（保留 provider），并**删掉 `[channels.AlephzBot]`** —— QA daemon
不得把用户的真 Telegram bot 拉上线。注意 daemon 启动时会**改写传入的配置文件**（把 `secret`
收进 vault 并按 channel id 归档），改段名后需重新写回 `secret`。
