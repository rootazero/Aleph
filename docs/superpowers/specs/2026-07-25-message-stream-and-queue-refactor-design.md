# §4.7 消息流与最终答案汇总 / §4.8 消息排队与改需求打断 —— 深度重构设计

- **日期**：2026-07-25
- **范围**：FEATURE_LOCATOR §4.7、§4.8
- **分支**：`feat-msgstream-queue-refactor`
- **参考项目**：`T:\Github\{codex, hermes-agent, kimi-cli}`

---

## 1. 背景与对照结论 (Gap Analysis)

### §4.7 消息流与最终答案汇总

| 维度 | codex | hermes | kimi | Aleph 现状 | 判定 |
|---|---|---|---|---|---|
| 终局文本单一源 | `TurnOutput` 结构化 | `turn_finalizer.py` | wire hub | `sanitize_final_response` 原子 + 4 条投递路径全接 | ✅ 领先 |
| 流式 / instant 双模 | 仅流式 | 仅流式 | 仅流式 | `plan_instant` sink-无关状态机 | ✅ 领先 |
| 错误呈递本地化 | 单套 i18n | 单套 i18n | 单套 i18n | **两套并存且分类不一致** | ❌ 缺陷 |
| fan-out 不阻塞主流 | 事件总线异步 | 异步 | 异步 | `deliver_final().await` 排在 `inner.emit` **之前** | ❌ 缺陷 |

### §4.8 消息排队与改需求打断

| 维度 | codex | hermes | kimi | Aleph 现状 | 判定 |
|---|---|---|---|---|---|
| 三态 busy 策略 | Interrupt / Steer | steer / queue / interrupt | — | Steer / Interrupt / Queue + per-channel | ✅ 领先 |
| 排队唤醒机制 | `watch::Sender<InputQueueActivity>` 事件驱动 | 事件驱动 | `asyncio` 队列订阅 | **2 秒轮询**，最长 30 min | ❌ 唯一轮询实现 |
| FIFO 到达序 | 到达即同步入队 | 同步 | 同步 | `register()` 在 `tokio::spawn` **内部** | ❌ 缺陷 |
| 显式停止清空队列 | `InputQueue::clear_pending` | 有 | 有 | `/stop` 只取消 run，队列残留继续放炮 | ❌ 缺陷 |
| 排队可见性 | TUI `pending_input_preview` | 有 | 有 | 零反馈（无 metric / UI / 计数日志） | ❌ 缺失 |
| 覆盖面 | 全 surface | 全 surface | 全 surface | 仅 channel，Panel / CLI 无车道 | ❌ 缺失 |
| RAII 泄漏防护 | — | — | — | `TicketGuard` + `RunSlot` 双 RAII | ✅ 领先 |

---

## 2. 缺陷清单（定位到行，均为改前锚点）

### §4.7

**F1 · origin fan-out 阻塞主流**
`event_emitter/origin_fanout.rs:100-117` —— `deliver_final().await` 排在 `inner.emit(event)` 之前。
Telegram / Slack 一次慢投递把 Panel 的 `RunComplete` 帧连同其后所有事件一起卡住。
装饰器契约本应是「主流优先、镜像尽力」。

**F2 · `InstantState` 终局不复位**
`event_emitter/instant_buffer.rs:147-181` —— `final_emitted` 在 `RunComplete` 后永不复位，
`buffer` 也不按 run 归零。emitter 一旦跨 run 复用（`InstantBufferingEmitter` 是 `pub` 装饰器），
第二个 run 的 summary 兜底被静默吞掉，或上一个 run 的残余文本拼进下一个 run 的终局块。
状态机应当按 run 自洽，而不是依赖「调用方保证每 run 新建 emitter」这一未被类型系统表达的约定。

**F3 · 终局事件补发裸吞失败**
`execution_engine/helpers.rs:318-329` —— drain 漏发时的 `RunComplete` 补发用 `let _ =`。
与 2026-07-04 定的分档（Run\* 骨架事件 warn / 装饰性事件 debug）矛盾：同一个事件，
两个产地两种待遇。

**F4 · 终局兜底混入中间进度块**
`reply_emitter/extract.rs:69-76` —— 兜底拼接把 `is_intermediate` 进度块也串进「最终答案」。
`approval/operator_requester.rs:127` 是真实生产者（审批提示以 standalone intermediate chunk 推送），
于是群聊 transcript / cron 结果里会混进审批文案。

**F5 · 错误呈递不是单一源**
- `execution_engine/mod.rs::user_receipt` —— 硬编码中文，自带 `classify_failed` / `is_rate_limited` / `is_unreachable`；
- `i18n.rs::format_execution_error` —— 另一套 `contains_phrase` 分类，双语。

后果三条：
1. Panel 在 `language = "en"` 下弹中文错误；
2. `user_receipt` **没有 AUTH 桶** —— API Key 失效时告诉用户「请重试」（i18n 侧有 `ErrAuth`）；
3. `is_unreachable` 用 `msg.contains("connection")`，正是 `contains_phrase` 的 doc comment 明写要避免的
   foot-gun（`disconnection_policy` 误命中）。

**F6 · 同一次失败报两遍**
执行阶段失败时，用户收到**两条**消息：一条来自 run 自己的 emitter
（`RunError` → `ReplyEmitter::send_error` / Panel 气泡），一条来自等待循环收尾的错误回执
（`executor.rs` 的 `final_err`、`server_init.rs` 的 `Err(e)` 分支）。
两个 surface 都中招。

### §4.8

**Q1 · FIFO 到达序不成立**
`inbound_router/executor.rs:403-430` —— `busy_queue::register` 在 `tokio::spawn` 内调用，
两条相隔 1 ms 的消息可能反序入队。模块 doc 承诺的「every message joins its session's FIFO lane
up front ... so a newcomer can never jump ahead of waiting siblings」在实现上不成立。

**Q2 · 2 秒轮询**
`inbound_router/executor.rs:405 / 475` —— N 条排队消息末条额外累计 N×2 s 延迟；
每次轮询还让 `try_inject_steering` 重读**整份** session 事件日志（`get_events(None, None)`）。
三个参考实现全部事件驱动。

**Q3 · `/stop` 不清队列**
`inbound_router/command_handler.rs::handle_stop` —— 只走 `cancel_session`，per-session 车道原封不动，
随后逐条放炮。codex `Op::Interrupt` 明确 `InputQueue::clear_pending`。
**注意**：清队列不能下沉到 `cancel_session` 本身 —— `execution_engine/gate.rs:170` 的 `Interrupt`
busy 模式正依赖车道重启消息。

**Q4 · 队列深度不可观测**
`gateway.metrics.run_concurrency` 有 `running_sessions` 与信号量快照，唯独没有 queued。

**Q5 · Panel / CLI 无 follow-up 车道**
`bin/aleph-server/server_init.rs` 的 `agent.run` / `chat.send` 直接 spawn `engine.execute`，
`AgentBusy` 直接变 `RunError` 气泡。Steer 注入失败的两个真实场景——带附件的消息
（`steering.rs:329` 主动 defer 到 busy 队列，而 Panel 根本没有队列）、steering burst 撞
`MAX_PENDING_STEERING` ——都导致 Panel 消息丢失。

---

## 3. 设计

### 3.1 架构：`busy_queue` 上提到 gateway 层（P2 高内聚 / P1 低耦合）

等待车道现有三个利益相关方：inbound_router（channel）、server_init（Panel）、
execution_engine（放槽信号）。留在 `inbound_router/` 下会让 Panel 路径反向依赖 channel 路由模块。

```
src/gateway/busy_queue/
├── mod.rs      车道原语：FIFO + TicketGuard(RAII) + per-session Notify + purge
├── deliver.rs  共享等待循环 deliver_with_ticket()（inbound_router 与 server_init 同走）
└── config.rs   BusyQueueConfig ← [execution]
```

原 `src/gateway/inbound_router/busy_queue.rs` **删除**（连根，不留 re-export 影子）。

### 3.2 唤醒改事件驱动（codex `watch::Sender<InputQueueActivity>` 对位）

- 每个 `Lane` 自带一个 `Arc<Notify>`（按 session 惰性建、随车道消亡回收）；
- `SessionRunRegistry::release()` 已是**唯一权威放槽点**（且已带 seq 广播）→ 增一行
  `busy_queue::notify_slot_free(&key)`；
- `TicketGuard::drop` 同样 notify（队首离队 → 晋升次位），`purge` / `cancel_queued_run` 亦然；
- 等待端 `timeout_at(wake_by, notified)` —— 关键是 **`Notified::enable()` 先注册、再检查车道
  状态、再 attempt**：注册早于 attempt，故 attempt 期间到达的放槽信号不会掉进缝里。
  **保留有界兜底 tick**（默认 30 s，原 2 s 轮询的 15× 稀释）只兜「我们根本无从观察到的」
  漏发；漏发信号仍 fail-open，「车道永不 wedge」的不变式不动。

> R10 合规：纯机械的到达序簿记 + 放槽信号，不做意图 / 完成度 / 相关性判断。
> 模型变强不会让这段脚手架变得不必要（Future-Proof ✓）。

### 3.3 停止语义（Q3）

取消标记落在 **ticket** 上（不是车道上），所以停止之后到达的消息不会继承它。两个粒度：

| 粒度 | API | 调用点 | 语义 |
|---|---|---|---|
| 会话级 | `purge(session_key) -> usize` | `command_handler::handle_stop`（`/stop`） | 丢弃该会话全部排队消息，条数追加进 `/stop` 回执 |
| run 级 | `cancel_queued_run(run_id) -> bool` | `AgentRunManager::cancel_run`（Panel `chat.abort`） | 丢弃指定的那一条 |

等待者观察到取消 → 返回 `DeliveryOutcome::Purged`，**走正常回执路径而非错误路径**
（会话级由 `/stop` 一次说清整批；run 级由客户端自己的 abort 响应交代）。

`gate.rs` 的 `Interrupt` 分支**不**调用任一者 —— 它依赖车道重启自己那条消息。

> run 级这条是 Q5（Panel 补车道）带出来的必然配套：排队消息已经把 `run_id` 回给了客户端，
> 却还没进引擎 `active_runs`，`ExecutionEngine::cancel` 看不见它 —— 不接这条，Panel 就会有一个
> 停不掉的 pending 气泡。

### 3.4 错误呈递真·单一源（F5）

```
ExecutionError::receipt_kind()       typed  ─┐
i18n::classify_error_text(&str)      string ─┴→ ReceiptKind ─→ i18n::Msg（双语文案）
```

- `ReceiptKind` 落在 `i18n.rs`（文案目录旁），7 桶：`Timeout / Cancelled / AgentBusy /
  RateLimited / Auth / Unreachable / Failed`。`ReceiptKind::code()` 是 wire code 的唯一源；
  除**新增**的 `AUTH` 外拼写全部不变（客户端可 switch，属 API）；
- `ExecutionError::user_receipt(locale) -> (&'static str, String)` 与
  `receipt_kind() -> ReceiptKind` 是仅有的两个出口，文案由 `i18n::t(Msg::ErrReceipt(kind))` 出；
- `i18n::format_execution_error` 的唯一消费方（`inbound_router/executor.rs`）改走 typed
  `user_receipt`，该函数与其专用的 `truncate_error` 随即**整体删除**——它在把 typed 错误
  `to_string()` 之后重新按字符串分类，还回显最多 200 字原始内部链；
- **删除** `execution_engine/mod.rs` 的 `classify_failed` / `is_rate_limited` / `is_unreachable`。

locale 来源：`execute.rs` 从 `request.metadata["locale"]`（inbound_router 已 stamp）取，
缺省 `Locale::from_config(None)` = Zh，与现状一致；bin crate 两个站点从 `app_config` 取。

### 3.5 配置（与 §4.5 `[team_broadcast]` → `BroadcastConfig` 同构）

`[execution]` 新增四项，全部 `#[serde(default = "…")]` 命名默认函数读现有 const（单一来源零漂移）：

| 键 | 默认 | 取代 |
|---|---|---|
| `busy_queue_max_per_session` | 32 | `MAX_QUEUED_PER_SESSION` |
| `busy_queue_max_wait_secs` | 1800 | `BUSY_QUEUE_MAX_WAIT_SECS` |
| `busy_queue_wake_fallback_secs` | 30 | `BUSY_POLL_MS`（2 s 轮询） |
| `max_pending_steering` | 16 | `MAX_PENDING_STEERING` |

前三项经 `BusyQueueConfig` 流入 `deliver_with_ticket`；`max_pending_steering` 走既有
`ExecutionEngineConfig` → `gate.rs` → `try_inject_steering` 通路。

### 3.6 可观测性（Q4）

`gateway.metrics.run_concurrency` 增 `busy_queue` 字段：
`{ total_waiting: usize, per_session: Vec<{session_key, depth}> }`（按深度降序、空车道省略），
与既有 `ConcurrencySnapshot::per_agent` 呈现语言一致；Panel `RunSlotsCard` 加一行徽标。

### 3.7 数据流（改后）

```
inbound / Panel 消息到达
  → busy_queue::register(session, cap, run_id)  ← 同步、在 spawn 之前（修 Q1 到达序）
  │    └ None（车道满）                          → Rejected（立即回执，语义不变）
  → spawn { deliver_with_ticket(guard, cfg, attempt_fn) }
       ├ 已被取消                                → Purged
       ├ is_front → attempt → 跑过了              → Executed(_)（emitter 已报，调用方不再报）
       ├ 超 max_wait                             → TimedOut（回执，非静默丢弃，语义不变）
       └ 否则 park on Notify ⟵ SessionRunRegistry::release / TicketGuard::drop
                                                 / purge / cancel_queued_run
/stop        → busy_queue::purge(session)          → 整批一条回执
chat.abort   → busy_queue::cancel_queued_run(run)  → Panel 收 RunError{CANCELLED} 关气泡
metrics      → gateway.metrics.run_concurrency.busy_queue → Panel RunSlotsCard
```

---

## 4. 错误处理

- **谁报错**：`DeliveryOutcome` 是单一源 —— `Executed(_)` 已由 run 的 emitter 报过，
  调用方只记日志；`Rejected` / `TimedOut` 归调用方。
- **车道满 / 超时**：行为与现状一致（用户可见回执，非静默丢弃），只是阈值改为可配。
- **purge / abort**：独立 `Purged` 出口，回执而非错误。channel 侧静默（`/stop` 整批一条回执），
  Panel 侧额外发一帧 `RunError{CANCELLED}` —— 它的气泡按 `run_id` 记账，没有终局帧就一直转。
- **Notify 漏发**：兜底 tick 兜住；`is_front` 的 fail-open 语义不变（未知票 → 允许投递）。
- **panic**：`TicketGuard::Drop` 保持唯一出口，尸票不可能留在车道（既有回归测试保留）。
- **locale 缺失**：回退 Zh，与现状字节一致。

---

## 5. 测试计划

| 用例 | 覆盖 |
|---|---|
| `register` 先于 spawn → 到达序 | Q1 |
| notify 唤醒（不等兜底 tick） | Q2 |
| `purge` 清空车道并唤醒全部等待者 | Q3 |
| `cancel_queued_run` 只命中目标 ticket，siblings 不动 | Q3 |
| `panic_while_holding_ticket_releases_the_lane`（保留） | 回归 |
| cap 拒最新 / 释放后重新准入（保留，改用配置值） | 3.5 |
| `busy_queue` 深度快照 | Q4 |
| `max_pending_steering` 从配置生效 | 3.5 |
| `extract_final_response` 跳过 intermediate chunk | F4 |
| `plan_instant` 在 `RunComplete` 后复位（第二个 run 兜底可用） | F2 |
| origin fan-out：inner 先收到 `RunComplete` | F1 |
| `user_receipt`：401 → AUTH / En locale / `disconnection_policy` 不误命中 | F5 |
| `DeliveryOutcome::user_error`：只有 never-ran 的出口欠用户回执 | F6 |

---

## 6. 熵减清单（已同步删除）

- `src/gateway/inbound_router/busy_queue.rs`（整文件，上提后原地删，不留 re-export 影子）
- `execution_engine/mod.rs`：`classify_failed` / `is_rate_limited` / `is_unreachable`
  （含 `msg.contains("connection")` 的 `disconnection_policy` 误命中坑）
- `i18n.rs`：`format_execution_error`（typed 错误被 `to_string()` 后重新按字符串分类的
  第二套分类器，唯一消费方已改走 `user_receipt`）+ 其专用的 `truncate_error`
  （它把最多 200 字原始内部链回显给 channel）
- `i18n::Msg` 的 `ErrRateLimit` / `ErrAuth` / `ErrTimeout` / `ErrNetwork` /
  `ErrServiceUnavailable` / `ErrGeneric{detail}` 六个变体 → 收敛为单个
  `ErrReceipt(ReceiptKind)`
- `executor.rs`：`BUSY_POLL_MS` / `BUSY_QUEUE_MAX_WAIT_SECS` 常量与整个轮询循环

---

## 7. 红线自检

- **R1**：无平台 API。
- **R4**：车道与回执均为 gateway I/O 边界，不含业务推理。
- **R7 / R10**：三态策略仍由通道显式声明，网关不二次打分；新增代码全部是机械簿记，
  `src/harness/` 零触碰、12 文件与行数棘轮不变。
- **P1 / P2**：`busy_queue` 上提消除 Panel → inbound_router 的反向依赖，车道原语高内聚。
- **P6**：不为假想需求预留抽象；配置项全部对应已存在的硬编码常量。
