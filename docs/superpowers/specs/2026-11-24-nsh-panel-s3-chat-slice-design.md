# NSH Panel S3：会话纵切与 v1 实时事件 / Chat Vertical Slice and v1 Live Events

> 状态 / Status: Approved 2026-11-24（含 §7a）。Worktree `.worktrees/nsh-panel-s3`，branch `nsh-panel-s3-chat`，基线 `b2c00e624`。
> 路线图 / Roadmap: [S3 会话纵切](../plans/2026-11-22-nsh-panel-migration-roadmap.md)。前置：S0、S1-A/B/C、S2 已合入 main。

## 1. 意图 / Intent

S2 交付了一个可定制但不能工作的 TSX 壳：两端（NSH 桌面宿主经 Desktop IPC，浏览器经 Web `/v1/ws`）只能读 `health`、`sessions.list` 和面板快照。业务 UI 仍只有旧 Leptos 一份，Tauri 退场门槛因此无法推进。

S3 让同一份 TSX 源码在两端完成一条真实业务路径：**选会话 → 读历史 → 发送 → 流式看到回复 → 取消**，结果来自真实 Core，权限与旧 `/ws` 完全一致。

S2 shipped a customizable shell that cannot do work yet. S3 makes one real business path work end to end on both hosts, from one TSX source, with the same authorization as legacy `/ws`.

## 2. 用户已作的决定 / Decisions already made

| # | 决定 | 来源 |
|---|------|------|
| D1 | 在两个 v1 端点开放 `chat.history`、`chat.send`、`chat.abort`，沿用现有 `dispatch_for_peer` 鉴权、幂等、速率路径（选项 A） | 本次会话 |
| D2 | 流式输出走**服务端事件推送**：把旧 `/ws` 内联的事件准入判定抽成共享函数，旧 `/ws` 改为调用它并以等价测试锁定；两个 v1 端点用同一函数推送（方案 1） | 本次会话 |
| D3 | 断线/溢出后只**重取快照**（`chat.history`），标记 stale；不实现 `events.subscribe` 补读，不宣告 durable replay，不暗定 ADR-007 | 路线图 S3/S4 分界 + 本次会话 |

## 3. 目标 / 非目标 / 反目标

**Goals**
- G1 两端真实发送消息、流式显示回复、取消运行；结果落库可在旧 Panel 看到。
- G2 事件准入只有一份判定；旧 `/ws` 行为不变（等价测试证明）。
- G3 v1 事件推送经 `system.hello` 特性协商开启；旧客户端（S1/S2 版本）不受影响。
- G4 普通成员经 Web v1 只能读写自己的会话，不被提升为 owner；收到的事件与旧 `/ws` 对同一主体的过滤结果一致。
- G5 模块卸载或连接代际变化后，事件回调不再到达旧模块（SDK 代际语义扩展到事件）。

**Non-goals**
- `events.subscribe` / 补读 / 去重游标（S4）。
- 附件、语音、agent/model/exec-tier 选择器、workspace pane、`/btw`、计划面板等旧 ChatView 子功能（S5 逐项迁移）。
- Linux/Windows 上的 `tsx-panel` 宿主（S2 仅 macOS；IPC 服务端改动在三平台编译与单测，但真机只做 macOS）。
- 默认渲染器切换、删除旧 Leptos ChatView、Tauri 退场。
- S2 遗留项（生产面三次挂载归零、真实 Cookie 300 s 过期）——本轮不做，回执中保持"未验证"。

**Anti-goals（做到也算失败）**
- 为 v1 另写一份事件过滤、会话归属或权限判定。
- v1 端点在未协商时推送任何 id-less 帧（会打断旧客户端：`shared/client/src/connection.rs` 的 local IPC 读循环把 notification 当协议错误断开）。
- 写操作在 v1 上失去幂等保护（见 §5.3）。
- 把"重连后重取历史"写成 replay 或 durable recovery。

## 4. 现状 / Current state（代码事实）

- **方法放行**：`src/gateway/server/connection/mod.rs:103` `V1_METHODS = ["health", "sessions.list", PANEL_UI_CAPABILITY_SNAPSHOT_METHOD]`；`local_ipc.rs:283`、`web_v1.rs:479` 调 `v1_mounted`。
- **会话归属已在共享分发中**：`src/gateway/method_visibility.rs:563-565` 把 `chat.send/abort/history` 标为 `Treatment::KeyChecked`，由 `dispatch_for_peer` 依调用方身份校验 session key——v1 放行后自动复用。
- **连接行已共用**：Web v1（`web_v1.rs:443-455`）与 Desktop IPC（`local_ipc.rs:163-171`）都在 `GatewaySharedState.connections` 插入 `ConnectionState`（`first_message=false`、`caller_user`、`caller_role`），`cleanup::run_cleanup` 统一清理。旧 `/ws` 的事件判定读的正是这张表，所以可直接复用。
- **事件判定是内联的**：`mod.rs:1339-1588` 在 `client_event_rx.recv()` 分支里依次做 token-rotated / device-revoked 关闭、`pty::manager::frame_admits`、`ui_display_admits_connection`、登录墙 `wall_admits`、`event_scope_guard.can_receive`、`surface::delivery::audience_allows`、`subscription_manager.should_receive`、`event_visibility.event_admits_for`、capability/ui 事件的二次身份复核、`event_visibility.project_for` 投影、发送前 `frame_admits` 复核，最后 `event_wire_form` 生成线上字节；`Lagged` 时发 `overflow_warning_frame` 并以 1008 关闭。
- **v1 只有请求/响应**：Web v1 `serve`（`web_v1.rs:401-535`）是顺序的读→执行→写，`max_inflight_requests: 1`；Desktop IPC `serve_requests`（`local_ipc.rs:220-275`）已是 `select!`（读帧 + 完成的 job），无事件分支。
- **幂等被关掉**：两条 v1 路径都调 `plan.without_cache()`（`local_ipc.rs:299`、`web_v1.rs:505`）；旧 `/ws` 只对 off-loop 只读方法这样做（`mod.rs:693`），写方法保留缓存键。
- **协商设施已存在**：`shared/protocol/src/system_hello.rs` 的 `Feature`（`EventReplay/Snapshot/BinaryChannel/Unknown`）与 `negotiate_features`；服务端 `src/gateway/handlers/system_hello.rs:96` 目前以空实现集协商。客户端 `AlephClient::open_local` 要求 `result.features.is_empty()`；`interfaces/panel/web/host.js:59` 发送 `features: []`。
- **桌面宿主丢弃事件**：`desktop/nsh-shell/src/panel_host.rs` 在 `connect_local` 后注释 "The stream events are not used" 并丢弃 `_events`；`AlephClient` 已有 `StreamEvent` 与 `take_topic_events()` 两个接收面。
- **TSX SDK**：`interfaces/panel/shell/sdk.ts` 的 `createSdk` 已有 dispose/代际门控与 release 集合，只订阅连接状态。

## 5. 设计 / Design

### 5.1 共享事件准入 / One admission function（D2）

在 `src/gateway/server/connection/` 新增 `event_admission.rs`（或置于 `forward.rs`，由实现者按文件大小决定），把 `mod.rs:1339-1588` 的判定原样迁出为：

```rust
pub(super) enum EventVerdict {
    Skip,
    Send(String),          // exact wire bytes (after projection + event_wire_form)
    Close(CloseReason),    // token_rotated / device_revoked
}
pub(super) async fn admit_event(
    ctx: &ConnectionContext, conn_id: &str, client_is_local: bool,
    session_device_id: Option<&str>, event_json: String,
) -> EventVerdict;
```

- 旧 `/ws` 循环只保留：调用 `admit_event`、按 verdict 写/关闭、`Lagged`/`Closed` 处理。**语义零变化**。
- **等价测试**：迁移前先给内联逻辑写特征测试（至少覆盖：pty 帧、他人 session 的 `stream.*`、`running_set_changed` 投影、`audience` 不匹配、登录墙 guest、`pairing.` 权限、token-rotated loopback 吞掉/远程关闭、device-revoked 自己/他人、无法解析的帧默认转发），迁移后同一组测试不改一字通过。
- 清理：`mod.rs` 中被迁出的内联块删除，不留第二份。

### 5.2 v1 事件推送 / Live events on v1（G3）

**协商**
- `Feature` 新增 `LiveEvents`（wire `"live_events"`），`is_known` 同步。
- v1 两端的 hello 以 `[LiveEvents]` 为实现集调用 `negotiate_features`；未请求则结果为空 → 完全不推送（旧客户端零变化）。
- 旧 `/ws` 的 `system.hello`（若有调用）不变：它本就推事件，不依赖此特性。

**服务端每连接**
- 协商成功后：`PerClientBuffer::new()` + `forward_bus_to_client(ctx.event_bus.subscribe(), buffer)`（与旧 `/ws` 同一份），在连接主循环里加事件分支调用 `admit_event`。
- **v1 事件主题白名单**：`V1_EVENT_TOPICS`（与 `V1_METHODS` 并列、同文件），初始精确列举 `stream.run_accepted`、`stream.response_chunk`、`stream.run_complete`、`stream.run_error` 与 `connection.warning`（见 §7a B6）；在 `admit_event` 之后作为额外的收窄条件（只收窄，不放宽）。理由：与方法放行对称，事件面也按需挂载。
- **Web v1 主循环改造**：`serve` 由顺序循环改为 `socket.split()` + `select!{ biased; stop; revoked; event; 进行中的请求完成; 读下一帧(仅当无进行中请求) }`，保持 `max_inflight_requests: 1`。
- **Desktop IPC**：`serve_requests` 的 `select!` 增加事件分支；写帧仍走 `send`（`WRITE_TIMEOUT`）。
- **超限**：事件字节超过端点帧上限 → 不发该帧，改发 `overflow_warning_frame(1, total)`，连接保留；`Lagged` → 与旧 `/ws` 相同：发 warning 后关闭（Web 1008；IPC 直接结束连接）。
- **撤销/到期**：现有 `revoked` 轮询与 `expires` 超时不变；`admit_event` 返回 `Close` 时 v1 同样结束连接。

**客户端**
- `shared/client`：`open_local` 请求 `LiveEvents`，接受 `result.features ⊆ requested`（替换 `is_empty()` 检查）；local IPC 读循环在已协商时接受 `jsonrpc:"2.0"` 且无 `id` 的 notification，交给与 WebSocket 路径相同的 `handle_message`/`classify_value` 分流到 `StreamEvent` 与 `TopicEvent` 接收面；未协商时保持"notification 即断开"。
- `desktop/nsh-shell/src/panel_host.rs`：保留 `connect_local` 返回的事件接收器与 `take_topic_events()`，以连接代际（`link.attempt`）绑定转发任务，向 TSX 发宿主事件 `aleph.event { topic, data }`；连接替换/关闭时任务随之结束。删除 "stream events are not used" 注释与 `_events` 丢弃。
- `interfaces/panel/web/host.js`：hello 请求 `["live_events"]`；`onmessage` 对无 `id` 帧发 `aleph.event`（与桌面同形）；未协商则忽略。`nsh.json` 的 host 能力清单加 `aleph.event`（如 NSH 需要声明事件名）。

### 5.3 写方法放行与幂等 / Write methods（D1）

- `V1_METHODS` 加入 `chat.history`、`chat.send`、`chat.abort`。
- v1 两端去掉无条件的 `plan.without_cache()`，改与旧 `/ws` 同一规则：`RpcExecutor::execute` 内部本就只对 `lane.needs_idempotency()` 的方法使用键，所以直接传 `plan`；若 `without_cache` 因此只剩旧 `/ws` off-loop 一处调用，保留，否则删除。
- Web v1 仍受 `gateway.require_idempotency_key`；TSX 每次发送尝试生成一个 UUID v4 `idempotency_key`，重试同一尝试复用同一键。
- 速率：Web v1 已有 `check_origin_rate`（`chat.send` → `RpcHeavy`）；Desktop 无网络来源，沿用现状。

### 5.4 TSX 会话模块 / Chat module

- **SDK**：`createSdk` 增 `on(topicPrefix, listener): () => void`，与 `subscribe` 共用 `releases` 集合与 `current()` 门控（disposed 或代际变化后不再回调）；`ShellAccess` 增 `onEvent`。`sdk.test.ts` 加对应用例（dispose 后不到达、旧代际不到达）。
- **模块**：新增 `interfaces/panel/modules/Chat.tsx`，稳定 ID 走 S2 布局（新模块按 S2 默认规则出现）。`Sessions.tsx` 行可点击 → 打开该会话（`navigate` + 选中 key）；Chat 也提供"新会话"（不传 `session_key`，由服务端返回的 `session_key` 定位）。
- **流程**：进入 → `chat.history{session_key, limit}`；若 `active_run` 非空则把它当作当前运行接续显示。发送 → `chat.send{message, session_key, channel:"gui:chat", stream:true, idempotency_key}` → 记录 `run_id`；`stream.response_chunk`（按 `run_id` 过滤）追加到流式气泡；`run_complete`/`run_error` → 重取 `chat.history` 以快照为准。Stop → `chat.abort{run_id, session_key}`。
- **连接变化**：代际变化或收到 `connection.warning` → 显示 stale 标记并重取 `chat.history`。不声称补齐中间事件。
- **长列表与输入法**：历史用 `List`（`keyExtractor` = 消息 id/序号）；输入框必须在组合输入期间不提交（Enter 只在非 composing 时发送），中文/日文 IME 两端实测。

### 5.5 被清理的旧代码 / Cleanup

| 位置 | 处理 |
|---|---|
| `src/gateway/server/connection/mod.rs:1339-1588` 内联准入 | 迁出为 `admit_event`，原处只留调用 |
| `src/gateway/server/connection/web_v1.rs:401-535` 顺序循环 | 替换为 split + select |
| `local_ipc.rs:299`、`web_v1.rs:505` 无条件 `without_cache()` | 删除；`RpcPlan::without_cache` 若无剩余调用者一并删除 |
| `shared/client/src/connection.rs` "S1-B admits only responses" 分支与 `features.is_empty()` 检查 | 改为协商感知 |
| `desktop/nsh-shell/src/panel_host.rs` 丢弃 `_events` | 改为转发 |
| `docs/research/NSH_PROTOCOL_V1.md` 与参考文档中"v1 只读"表述 | 更新为当前事实 |

## 6. 方案对比 / Approaches

| 方案 | 做法 | 取舍 | 结论 |
|---|---|---|---|
| 1 事件推送（选中） | 抽取共享准入 → v1 协商后推送；断线重取快照 | 真流式，满足 S3 验收；动旧 `/ws` 热路径，需等价测试 | 用户选择 |
| 2 快照轮询 | 发送后轮询 `chat.history` | 不碰事件面；无流式、增负载，S3 流式验收不过 | 否 |
| 3 订阅 + 补读 | 实现协议 §9 `events.subscribe` | 提前做 S4，依赖未批准 ADR-007 | 否 |

参考：t3code `apps/server/src/orchestration-v2/threadHistoryPaging.ts` 用 `snapshotSequence` 衔接快照与事件——属于 S4 形态，本轮只借鉴"快照为准、事件为增量"的客户端纪律；deepseek-harness 的 keyed 贡献与 disposer 已在 S2 SDK 中体现，本轮扩展到事件订阅。

## 7. 验收 / Acceptance

自动化：
- `admit_event` 等价测试（§5.1）迁移前后同一组通过。
- v1 两端：未协商无推送；协商后他人会话的 `stream.*` 不到达普通成员、自己的到达；超限帧变 warning；`Lagged` 关闭；撤销关闭。
- 旧客户端兼容：不请求 `live_events` 的 `AlephClient::open_local` 与 S2 `host.js` 行为不变（现有测试保持绿）。
- `chat.send` 经 Web v1 普通成员：写入自己的会话；对他人 session key 被 `KeyChecked` 拒绝；同一 `idempotency_key` 重放不产生第二次运行。
- `cargo test`（gateway connection、client、nsh-shell `--features tsx-panel`）、`just panel-tsx-test`、`node --test interfaces/panel/web/host.test.mjs`、`cargo clippy`。

真机（macOS，隔离 Gateway，复用 `qa/nsh_s2_shell/` 装置扩展）：
- 桌面与 Chrome 各自：选会话 → 发送 → 流式出现 → 完成后历史一致；长回复中 Stop 生效且历史显示取消；中文与日文 IME 组合输入不误提交；重启 Gateway 后 stale 标记 + 重取。
- 同一条消息在旧 Leptos Panel 中可见（证明同一 Core 事实）。
- 回执：`docs/superpowers/runs/2026-11-24-nsh-panel-s3-chat-receipt.{md,json}`，逐项列出已验证 / 未验证。

## 7a. 预审后的补充决定 / Decisions after builder check

| # | 问题 | 决定 | 理由 |
|---|------|------|------|
| B1 | 无模型配置时 `chat.send` 走模拟执行（`chat_handlers::handle_send` → `AgentRunManager::start_run`），该路径不检查会话归属（`src/gateway/method_visibility.rs:156-164` 的 carve-out），旧 `/ws` 同样有此缺口 | **在 Core 补归属检查**：模拟路径在启动运行前对调用方提供的 `session_key` 走与真实路径相同的 `visibility::existing_session_is_visible`（会话不存在不算拒绝）；旧 `/ws` 与 v1 一并修复，删除 `method_visibility.rs` 中的 carve-out 段落 | Core 是唯一授权权威（R2/R6）；接口层可用性判断会造成两份事实。代价：旧 `/ws` 上此前能写他人会话的调用会被拒——这是缺陷修复，用测试锁定 |
| B2 | Gateway 重启后 Chrome 的 Cookie 绑定丢失（`web_v1.rs:58,107,310-317`，内存态） | **允许重新配对**；配对后 TSX 按 S2 布局恢复所选会话并重取 `chat.history`。桌面经新 socket 自动重连。不新增凭据持久化 | Cookie 内存态是 S1-C 有意的设计；持久化属于凭据存储变更，超出 S3 |
| B3 | 发送已生效但应答丢失 | 标记"结果未知"，**不自动重试**；重连后重取 `sessions.list` + `chat.history` 以快照为准。用户手动重试复用同一 `idempotency_key`，只在进程内缓存 TTL 内去重（`src/gateway/idempotency.rs:115-133`），回执如实写明 | 不承诺跨重启 exactly-once（与 Capability 入口"external-effect exactly-once deferred"一致） |
| B4 | 同键不同参数应返回 `idempotency_conflict`（协议 §7.2） | **S3 不实现**，记为已知限制；旧 `/ws` 与 v1 共用同一执行器，行为一致 | 改共享幂等契约是独立变更，不应夹带进纵切 |
| B5 | 桌面客户端有界队列可能丢流帧甚至丢 warning（`shared/client/src/connection.rs:786-808`） | 复用已有 `AlephClient::stream_continuity()`（`StreamContinuity{generation, dropped_events}`，计数在队列之外）：`panel_host` 在转发时比较计数，增长即向 TSX 发 `aleph.event{topic:"connection.stale"}`，TSX 与 warning 同样处理（stale + 重取历史） | 计数器本就为"满队列不能隐藏需要恢复"而设；不另造信号 |
| B6 | `live_events` 承诺通用 `stream.*` 还是聊天所需 | **窄契约**：S3 只承诺 `stream.run_accepted/response_chunk/run_complete/run_error` 与 `connection.warning`（及 B5 的宿主合成 `connection.stale`）。`V1_EVENT_TOPICS` 按这几个主题精确列举，而非 `stream.` 前缀；两宿主各有测试锁定同一集合 | 与分类器现状（忽略 `session_updated`/`running_set_changed`）一致，deny-by-default |

## 8. 留给实现者 / Left to the builder

- `admit_event` 落在新文件还是 `forward.rs`；签名中借用/所有权的具体形状。
- `aleph.event` 在 NSH host 事件声明中的确切写法。
- Chat 模块的视觉细节（沿用 S2 的 class 体系）。
- `history` 的 `limit` 默认值与长列表虚拟化是否需要（以 NSH `List` 实测为准）。

## 9. 风险 / What this makes harder

- 旧 `/ws` 事件路径被重构：等价测试是唯一护栏，覆盖不到的分支会漂移（判据 3）。
- `V1_EVENT_TOPICS` 是列举法（判据 5）：新模块需要新主题时必须显式加；这是有意的 deny-by-default。
- v1 写方法开放后，Web 端 Cookie 会话首次可产生业务效果；撤销/到期必须在事件与请求两个方向都关闭（判据 14）。
