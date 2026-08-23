# 队列可见性统一 + 错误码跨端收口 (Queue Visibility & Receipt-Code Convergence)

- **日期**：2026-08-23
- **分支**：`worktree-queue-visibility-round11`（基线 `064d036fc`）
- **覆盖章节**：FEATURE_LOCATOR §4.7（消息流与最终答案汇总）· §4.8（消息排队与改需求打断）· §6.1（流式回显与工作区面板）；顺带触及 §6.9（重连与状态重建）
- **对标**：codex `core/src/session/{input_queue,inject,turn_suspension}.rs` · pi `packages/agent/src/agent.ts`（`steeringQueue`/`followUpQueue`）· kimi-cli `wire/root_hub.py`

---

## 0. 一句话

**「等待中」在 Aleph 的整条 wire 上没有表示，于是三个面各自拿「运行中」冒充它**；而当等待最终失败时，Panel 又把服务端已经算好的错误码扔掉、用关键词猜了一个。本轮给「等待」一个一等表示，并把错误分类收口到跨 crate 单一源。

---

## 1. 问题陈述

### 1.1 排队期是一段完全静默的时间

`RunAccepted` 由 `execution_engine/execute.rs` 发出，即**准入之后**。排队期一帧都没有：

```
chat.send  ──►  返回 {run_id}                     客户端拿到 id，画「思考中」
                     ⋮  0 … max_wait_secs 完全静默 ⋮
try_claim ──► mark_admitted ──► RunAccepted{run_id, session_key}
```

后果有两层：

1. **用户层**：客户端握着一个**引擎从没听说过**的 run 显示「思考中」。`event_visibility.rs:1050` 自己写着「A run that never reached the engine has no `RunAccepted`」。终局回执齐备（`Rejected` / `TimedOut` / `Purged`），**正向确认为零**——没有位置、不可枚举、不可单条撤回。唯一能看见真相的是 operator 的 `gateway.metrics`（`busy_queue::snapshot()` 全仓唯一读者）。

2. **架构层**：`EventVisibilityIndex` 的 run→session 种子**只来自 `RunAccepted`**，所以排队期的任何帧都解析不出归属。§4.8 Round-8 ② 是靠给 `RunError` 单独加 `session_key` 打的补丁——**逐帧补齐，不是修根因**；下一个排队期帧会再撞一次（判据：「列举法只覆盖立法当天的世界」）。

这同时是 §4.7 那句「Aleph 有两个队列，混淆它们是这一段存在的唯一原因」的**成因**：两个队列里有一个是隐形的。

### 1.2 Panel 把服务端算好的错误码扔掉，重新猜了一遍

`i18n::ReceiptKind` 是服务端的 8 桶分类，`code()` 的 doc 逐字写着「**Clients may switch on it, so these strings are API — do not rename**」，并一直在 `StreamEvent::RunError.error_code` 上发。

而 `platform/wide/views/chat/state.rs::ChatSendError::classify` **不读 `error_code`**，拿 `error` 字符串 `to_lowercase().contains()` 重新分类：

```rust
let code = if l.contains("disconnect") || l.contains("not connected") || l.contains("websocket") { … }
           else if l.contains("usage limit") || l.contains("quota") || l.contains("rate limit") { … }
           else if l.contains("timed out") || l.contains("cloud") || l.contains("http") || l.contains("provider") { … }
           else { ChatSendErrorCode::Unknown };
```

这正是 §4.7 Round-5 ⑤ 在 `inbound_router/executor.rs` 上**删掉**的那句话（「把已经是 typed 的东西先 `to_string()` 再按字符串重新分类」），只是这次跨了 crate。两张表根本不重合：

| 服务端 `ReceiptKind::code()` | Panel `ChatSendErrorCode` | 用户实际看到 |
|---|---|---|
| `CANCELLED` | **无桶** | 按 Stop → `Unknown` → 红横幅「UNKNOWN 任务已取消」（§4.7 QA(c) 登记项） |
| `AGENT_BUSY` | **无桶** | **排队被拒 / 等待超时 → `Unknown` 红横幅** |
| `AUTH`（doc：「retrying will not help」） | **无桶** | key 过期落 `Unknown`，或被 `l.contains("http")` 误抓成 `CloudSendFailed` ＝「去重试」 |
| `SPEND_EXHAUSTED{limit, reset_ms}` | **无桶** | 花费上限 → `Unknown` |
| `TIMEOUT` / `RATE_LIMITED` / `UNREACHABLE` / `FAILED` | 关键词近似 | 会漂（`l.contains("http")` 是 `contains_phrase` doc 明写要避免的坑） |

**这两件事是同一件事的两半**：`RunQueued` 的两条终局边之一就是 `RunError{error_code: "AGENT_BUSY"}`。不修 1.2，做完 1.1 之后「排队等了 5 分钟然后失败」在 Panel 上仍然是一句 `UNKNOWN`。

---

## 2. 对标结论（先说不抄什么）

| 维度 | codex | pi | kimi-cli | Aleph 处置 |
|---|---|---|---|---|
| 队列拓扑 | 单 `InputQueue`（steer + mailbox） | **双队列** `steeringQueue` / `followUpQueue` | RootWireHub 广播 | **不移植**。Aleph 的 `BusyInputMode` 三态语义等价，且模式是 per-channel 配置而非每消息参数（既定裁定） |
| 排空粒度 | 全量 drain | `QueueMode = "all" \| "one-at-a-time"` | — | **不移植**。零消费者通道（R10） |
| 等待态对客户端可见 | `subscribe_activity` 回 `pending_activity`；TUI 可 `edit_queued_message` | `AgentState.steering: string[]` 进 UI 快照 | 广播 hub | **采纳理念，自研形状**（见 §3） |
| 排队消息归属 | `TurnInput::UserInput{client_id}` | — | — | **记账不做**（见 §7） |
| 跨进程交接 | `SuspendTurnOutcome`，显式声明「pending input 故意丢弃」 | — | — | 与 Aleph 已记的 P3 同一件事，**本轮不做**（见 §7） |
| 流式提交策略 | 两档齿轮 + 4 阈值 + 2 hold 计时器 + 迟滞 | — | — | **已裁定不抄**（2026-08-19）：那套复杂度是它的**单位**（渲染行）造成的；Aleph 单位是字符 ⇒ 策略塌成一个 `max` |

**架构映射的结论**：三个参考实现都是**单客户端**的（CLI / TUI），它们的队列全在客户端，所以「可见」是免费的。Aleph 是一核多端（R6），队列必须在服务端，于是「可见」需要一条 wire 表示——这是 Aleph 的问题，参考实现没有对应答案可抄。本轮不是移植，是**用 Aleph 已有的骨架事件机制回答一个参考实现不需要回答的问题**。

---

## 3. 设计

### 3.1 状态机：只加一个帧

`RunAccepted` **就是**准入边。它今天的名字之所以读起来像撒谎，是因为它前面缺一段——补上之后名字就是真的：

```
register_run 成功
   └─► RunQueued{run_id, session_key, ahead}           ← 唯一新帧
         ⋮ 每次醒来发现自己还不是队首且 ahead 变了 → 重发
   ├─► RunAccepted{run_id, session_key}                ← 既有帧，含义不变，从此是"准入"边
   │      └─► Reasoning / Tool* / … / RunComplete
   └─► RunError{..., error_code, session_key}          ← 既有帧（Rejected / TimedOut / Purged）
```

**关键性质：空闲会话上一帧都不多。** `register_run` 立刻拿到队首 ⇒ `is_front()` 为真 ⇒ `attempt()` 直接跑 ⇒ 从不发 `RunQueued`，Panel 照旧直接进 Thinking。**常见路径逐字节不变**——这是回归风险的天花板。

### 3.2 帧的形状（刻意窄）

```rust
StreamEvent::RunQueued {
    run_id: String,
    session_key: String,
    ahead: u16,          // 前面还有几条
}
```

砍掉的字段与理由（判据：「一个『展示用』字段在提交前必须能指出渲染它的那一行代码，指不出就是 CUT」）：

| 砍掉 | 为什么 |
|---|---|
| `seq` | **随兄弟**：`StreamEvent::RunAccepted`（`types.rs:53`）也没有 seq，而 §4.7 的打磨话术逐字写着「别指望 seq 字段做排序/去重，没人读它」 |
| `depth`（车道总深度） | 无渲染者。`ahead` 已回答用户唯一会问的那一问 |
| `enqueued_at` | 无渲染者。「已等 2 分钟」没有设计面 |
| `author` / `client_id` | `Ticket` 没有作者字段；房间里「队友排了一条」需要它 → 记账（§7） |

**`session_key` 取被寻址的那个**（不是 `btw::execution_session` 派生的执行车道键）。`spawn.rs` 自己的注释已把理由写清楚：「the `RunError` receipt for a run that never reached the engine has to resolve on the client that is attached to it — the derived session may have no row at all yet」。同一句话对 `RunQueued` 逐字成立。

### 3.3 顺带修一个根因（不是新功能）

`RunQueued` 携带 `session_key`，且 `note_frame` 跑在过滤器之前 ⇒ **`EventVisibilityIndex` 的种子从「准入」前移到「到达」**，排队期整段进入可解析区。

⚠️ **`RunError.session_key`（Round-8 ②）刻意保留。** 判据：「一条腿倒下之后，别默认另一条撑得住」。`frame.rs` 那句「修法是让帧自解析，而不是加宽解析器的喂养条件」描述的是**更强的形状**——索引可用不等于该退回去依赖索引。文档必须写明它现在是冗余的、以及为何冗余仍要留着，否则下一个读者会把它当残留清掉（判据：「一个机制的存在理由如果写在另一个文件里，删它的人不会读到那里」）。

### 3.4 产地：等待者在已有唤醒点自报位置

**为什么不是车道自己广播**：那需要把 `Arc<dyn EventEmitter>` 塞进 `static Mutex<HashMap>` 里的 `Ticket`，且要在锁外补发；并且会给车道长出「发事件」这个第二职责——车道是候车室，不是运行登记簿。

**为什么等待者自报是对的**：`notify_slot_free` → `wake.notify_waiters()` 本来就唤醒**全车道**（`busy_queue/mod.rs:253`），每个等待者本来就要重跑 `is_front()`。判据两条：

- 「一条唤醒边只回答它自己那个问题」——位置变化这件事**已经有一条边在描述它**，再造一条是浪费；
- 「产地要选那件事变成事实的唯一接缝」——对**我这条消息**而言，「我的位置变了」变成事实的时刻，正是我醒来发现自己还不是队首的那一刻。

**零新增唤醒边、零新增 tick、零轮询。**

> ❌ **明确否决查询式**（Panel 轮询 `chat.pending`）：§4.8 Round-5 ② 已经把轮询→事件驱动做过一轮，重新引入等于把那一轮作废。

### 3.5 服务端改动点

```rust
// busy_queue/mod.rs —— 车道仍是纯候车室，只新增一个同步只读查询
impl TicketGuard {
    /// 前面还有几条。ticket 不在车道里（已撤票/被清）读作 0 —— 与
    /// `is_front` / `is_cancelled` 同一个 fail-open 姿态。
    pub fn ahead(&self) -> u16;
}

// busy_queue/deliver.rs —— 唯一新增参数，形状与既有的 `attempt` 逐字同构
pub async fn deliver_with_ticket<F, Fut, R, RFut>(
    ticket: TicketGuard,
    cfg: BusyQueueConfig,
    attempt: &mut F,
    report: &mut R,                 // 位置变了就调一次
) -> DeliveryOutcome
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), ExecutionError>>,
    R: FnMut(u16) -> RFut,
    RFut: Future<Output = ()>,
```

⚠️ **`report` 必须是 async 且被内联 await，不能是 `FnMut(u16)` + `tokio::spawn`。** 发帧是 async；spawn 出去就放弃了顺序，而 `ahead` 是单调下降的——乱序到达等于界面闪回。内联 await 按构造保序，且与 `attempt` 用的是同一个 `FnMut() -> Fut` 惯用法。

循环里加**一处**上报，位置在「决定 park 之前」（覆盖队首被背压推回的情形）：

```rust
let ahead = ticket.ahead();
if last_reported != Some(ahead) {
    last_reported = Some(ahead);
    report(ahead).await;
}
```

| 改动点 | 性质 | 行数量级 |
|---|---|---|
| `busy_queue/mod.rs` `TicketGuard::ahead()` | 新增只读查询 | ~15 |
| `busy_queue/deliver.rs` 循环加一处 report + 去重 | 连线 | ~10 |
| `busy_queue/spawn.rs`（Panel/CLI）传闭包发 `RunQueued` | 连线，已有 `emitter` | ~10 |
| `inbound_router/executor.rs`（channel）传同样闭包 | 连线，已有 emitter | ~10 |
| `event_emitter/types.rs` + `events/frame.rs` + `frame_census` | 新 `StreamEvent` 变体 + 机械 match 臂 | ~40 |
| `handlers/chat.rs` `chat.history` + `pending[]` | 挂在既有快照上 | ~25 |

**服务端约 110 行，全部是连线**——零新模块、零新配置、零新 RPC、`src/harness/` 零触碰（R10）。

> **两条到达路径都无条件发帧。** 「只接 WS 面」的含义是**不往 Telegram 回一条「排队中」的消息**，不是「channel 来的 run 不发帧」。一条 Telegram 来的排队消息，它的 `RunQueued` 上 WS 总线是**对的**——同一会话开着 Panel 的人应该看见它。`OriginFanoutEmitter` 只扇出终局答案，骨架事件不经它。决定落在「谁渲染」，不落在「谁发」——避免按站点分叉（判据：「收敛写者时要数一遍写者」）。

### 3.6 Panel 状态机：让编译器点名每个读者

```rust
pub enum ChatPhase { Idle, Thinking, Streaming, Error, Queued { ahead: u16 } }
```

仍是 `Copy`/`PartialEq`/`Eq`。⚠️ **这里最初想按穷尽 `match` 设计，核实后发现前提不成立**：全 crate 只有 **12 个**读写站点（7 写 + 5 读），且**没有一处是穷尽 `match`**——每个读者都是 `==`、`matches!`，或干脆丢弃返回值。这意味着**加一个变体不是编译错误**：手机端 `composer.rs:66` 的 `matches!(…, Thinking | Streaming)` 正是这样漏掉 `Queued` 的——写下那天完全正确，`Queued` 这个第三种忙态出现的那一刻起悄悄错了，代价是排队期间手机 composer 又肯发一次。

真正让"是不是 busy"只有一处答案的，不是编译器而是**一个谓词 + 一条源码级守卫**：`ChatPhase::is_busy()`（`matches!(self, Queued{..} | Thinking | Streaming)`）是唯一允许判定忙态的地方，所有 surface 都必须问它；守卫 `no_surface_enumerates_the_busy_phases_by_hand` 按**规则而非名单**抓违规——一行 `matches!(` 里出现两个及以上 `ChatPhase::` 就判定为内联忙态集合，唯一豁免的正是 `is_busy` 自身，单变体的 `matches!`（问"是不是这一个特定阶段"，`==` 对 `Queued { .. }` 表达不了这一问）不算违规。**加一个变体不会让编译器点名任何读者**——读者要不要跟上新变体，靠这条守卫在下次改动时抓，不是靠类型系统。

渲染：占位气泡文案「排队中 · 前面还有 N 条」。

**`ahead == 0` 的含义是「前面没有别人了，但我还没开始」**，统一渲染成「即将开始」。它有两个来源，两个都为真、都该可见：

1. **队首被背压推回**——`attempt()` 回 `AgentBusy`（steering burst 到 `max_pending_steering`），我是队首但跑不了。这正是 §4.8 Round-9 处理的那类等待，**它今天完全不可见**（继续冒充 Thinking）。
2. `ahead()` 的 fail-open——ticket 已不在车道（撤票 / 被清 / 车道消失）。

两者接下来都会被 `RunAccepted` 或 `RunError` 覆盖，所以同形渲染无害。

⚠️ **上报点在「决定 park 之前」而不是在 `else` 臂里**——否则来源 1 漏掉。空闲会话仍然一帧不发：`is_front()` 为真且 `attempt()` 不回 `AgentBusy` ⇒ 函数在到达上报点之前就 `return` 了。

### 3.7 错误码跨端收口

```
shared/protocol/src/receipt.rs        ← 新增：ReceiptCode 枚举 + as_wire() / from_wire()
        ↑                                        ↑
  服务端 i18n::ReceiptKind::code()        Panel ChatSendError::from_wire_code()
  （改为读它，不再手写字面量）           （主路径；classify(msg) 降级为兜底）
```

`aleph-protocol` 已是两边共同依赖（`interfaces/webchat/Cargo.toml:110`）。判据：「跨 crate 的 wire 契约要么共用一个类型（重命名 ⇒ 编译错），要么在依赖两边的那一侧留一条真正对账的测试」——本轮**两者都做**：共用类型 + 两侧各一条对账。

- Panel 独有的 `Unsupported`（客户端拒发、从未上线）**保留**，不进共享表——它答的是另一问。
- ⚠️ **`classify` 刻意不删**：`error_code` 是 `Option<String>`，老 core 与非 `RunError` 路径（`ChatApi::send` 的传输层错误）没有码。但它从**第一分类器**降级为**兜底**，并留一条守卫断言「有 code 时永远不走 classify」。

### 3.8 attach 快照

`chat.history` 响应 `+ pending: [{run_id, ahead}]`（按位置排序），挂在 `active_run` / `plan` 已经在的那个快照上。

论证与 §6.9 ② 逐字相同：**一个快照分两次调用就开出一个「拿着 transcript 却拿着另一份状态」的窗口**。

这是 best-effort 直播帧的**权威那一半**——与 `agent_trace` 镜像 ↔ `RunSummary` 权威是同一个架构。刷新 / 第二个标签页 / 中途加入的客户端从这里重建排队相位。

`pending[]` 的两个渲染者：① 重建自己那条 run 的 `Queued{ahead}` 相位；② 显示车道深度。**不含正文预览**（那需要车道携带 payload ＝ 耐久化那件事，§7）。

---

## 4. 熵减与文档纠偏

### 4.1 代码

| 项 | 处置 | 依据 |
|---|---|---|
| `EventEmitter::emit_run_error`（`event_emitter/mod.rs:178`） | **CUT** | 零生产者（全仓所有 `RunError` 都直接构造 `StreamEvent`），P6 |
| `ChatSendError::classify` 的关键词表 | **降级为兜底**，不删 | §3.7 |
| `state.rs`（3088 行）切出两个文件 | 见 §5 | P2；只拆本轮真正改到的 |

### 4.2 FEATURE_LOCATOR 已过期的登记项（本轮逐条改正）

开工前按「开工修一条记录在案的 gap，第一步是去代码里确认它还成立」逐条证伪，**六条里有三条已经不成立**：

| 登记项 | 代码实况 | 处置 |
|---|---|---|
| §4.7 (ii) `[memory.assembler] render_style` 无人读（DECIDE） | `thinker/memory_context_provider/memory.rs:115` 已读；`DISCARD_TAG_PAIRS[1]` 已加 `<memory>` 围栏 + 漂移守卫 | **已 CONNECT，改文档** |
| §4.8 (ii) `render_user_session_text` CUT 候选 | 全仓已无此符号 | **已 CUT，改文档** |
| §6.9 已知边界「对端用户消息无实时回显」 | `stream.session_user_message` 帧已在 wire 上，Panel 有 `push_peer_user_message` | **已连线，改文档** |
| §4.7 `emit_run_error` 零生产者 | 仍成立 | 本轮 CUT |
| §4.7 QA(c) Panel 不读 `error_code` | 仍成立，且比登记的更大（§1.2） | 本轮修 |
| §4.8 P3 排队消息崩溃耐久化 | 仍成立 | 记账，见 §7 |

**这条本身是一条判据**：文档里的「已知缺口」记录会被后来的修复悄悄作废，而没有任何力让它变短。

---

## 5. 文件拆分（只拆本轮真正改到的）

`state.rs`（3088 行）是本轮改动最密的文件，只切出本轮真正碰的两块：

```
platform/wide/views/chat/state.rs (3088)
  ├─► state/send_error.rs   ← ChatSendErrorCode + ChatSendError + from_wire_code（~110 行，自带单测）
  └─► state/run_phase.rs    ← ChatPhase + active_run_id 生命周期（~200 行）
```

**不动** `execution_engine/execute.rs`(2516) / `chat/events.rs`(1916) / `steering.rs`(1379)——本轮对它们是加法，不满足「改到才拆」。它们的尺寸另记一笔账。

---

## 6. 验证

### 6.1 单测

- `TicketGuard::ahead()` 的 fail-open 姿态（ticket 不在车道 ⇒ 0），与 `is_front` / `is_cancelled` 同形
- `deliver_with_ticket` 只在 `ahead` **真变化**时上报（去重），且不因 `wake_fallback_secs` 兜底 tick 重复上报
- 空闲会话路径**一次 `report` 都不调**（守住「常见路径逐字节不变」）
- `ReceiptCode` 两侧对账：服务端 `ReceiptKind::code()` 的码集 == 协议表；Panel `from_wire_code` 对协议表**每一个**码都有非 `Unknown` 的映射（**期望值从协议类型派生，不写字面量清单**）
- `ChatPhase` 穷尽 match（编译期）

### 6.2 变异证伪（判据：没被证伪过的守卫不算守卫）

写完每条守卫**手动破坏一次**，确认它红且点得出文件行号：

| 破坏 | 应红的守卫 |
|---|---|
| `report()` 改成 no-op | 位置上报测试 |
| `from_wire_code` 改回 `classify` | 码表对账 |
| 从协议表删一个码 | 服务端码集相等测试 |
| 给 `ReceiptKind` 加一个桶而不加协议表 | 同上 |

⚠️ **分类要按四分法且顺序正确**：`running 0 tests` ⇒ VACUOUS → `test result: FAILED` ⇒ RED → `test result: ok` ⇒ GREEN → **剩下的（连 `test result:` 行都没有）才是 BUILD-ERROR**。cargo 对测试失败也打 `^error:`，别按它排序。

### 6.3 真机 QA

复用既有装置（隔离 `HOME` / `ALEPH_HOME` + 慢速 mock provider + Chrome）：

**GREEN**：一个会话连发两条 → 第二条显示「排队中 · 前面还有 1 条」→ **刷新页面仍在**（走 `pending[]`）→ 第一条完成 → 第二条转 Thinking → 完成。
**RED**（同场景、只剪断 `report` 那条线的对照二进制）：第二条全程显示「思考中」，刷新后回到空白。

外加一条终局面：把 `max_wait_secs` 调到 5s，让第二条超时 → Panel 应显示 `AGENT_BUSY` 对应的文案，**不是** `UNKNOWN` 红横幅。

⚠️ QA 的时钟必须是**会话日志**，不是墙钟。

### 6.4 构建（判据清单 §10 最小验证集）

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run        # check 看不见它的 #[cfg(test)]
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
cargo clippy --all-targets
```

第 4 条是**唯一编译出厂形态**的命令（`--lib` 测试构建里 `cfg(test)` 为真，看不见出厂形态的编译错误）。

---

## 7. 刻意不做（写进文档，防重提）

1. **不加 `busy_input` wire 参数 / Panel pill** —— 会话旋钮表既定裁定；三种处置在 Panel 上已各有手势（`＋`/Enter = 幽灵队列、轮边界自动 flush = Steer、`⚡`/Esc = abort 重排）。加参数得到零消费者通道（R10）。
2. **不移植 pi 的双队列 / 每消息 `streamingBehavior`** —— `BusyInputMode` 三态语义等价且是 per-channel 配置，同上。
3. **不移植 codex 的自适应分块** —— 2026-08-19 已裁定：那套复杂度是它的**单位**（渲染行）造成的。
4. **不做队列的崩溃耐久化（P3）** —— 但本轮**证实了接缝**：位置/成员变动恰好发生在 `mark_admitted` / `purge` / `cancel_queued_run` / `TicketGuard::Drop` **四臂**，与 §4.8 owed backlog 记的墓碑四臂**是同一组**。将来实施时写点落在这四臂，`ahead()` 的实现即现成的成员枚举。
5. **不给 `Ticket` 加作者**（codex `client_id`）—— 房间里「队友排了一条」需要它；`RunQueued` 加 `author` 是一行，但**没有渲染者**（判据：展示字段提交前必须指得出渲染它的那一行）。
6. **不做 `pending[]` 的正文预览 / 单条撤回（L3）** —— 预览需要车道携带 payload（＝第 4 条）。`busy_queue::cancel_queued_run` 已实现且**全仓零调用者**，接客户端入口是一条独立的账（属「一个决定有没有反悔的路」那条判据的正半边）。
7. **不做 channel 侧的「排队中」回执** —— 它与 §4.7 (i) 登记的「审批等待只到达授予面、不到达等待面」是**同一形状**（都是「等待发生在一个面，通知发在另一个面」），应当在 `src/approval/` 那一轮一起修，而不是在这里做半个。

---

## 8. 风险与边界

| 风险 | 缓解 |
|---|---|
| `ChatPhase` 加变体触及 6 个读者 | 编译器穷尽 match，无法静默漏掉；每处都要显式回答「Queued 算不算 busy」 |
| `deliver_with_ticket` 新增参数触及 2 个生产调用点 + 测试 | 必填参数（不给默认值）——编译错误强于登记表 |
| `RunQueued` 帧在 fan-out 装饰器里被误处理 | **已逐个核实（2026-08-23）**，四个装饰器无一需要新逻辑：<br>· `redacting.rs` **穷尽无通配**（line 81 的注释逐字说明这是有意的：「Every arm is explicit rather than a catch-all `_ => event`, so a NEW …」）⇒ 加变体是**编译错误**，逼一次显式决定；新变体进 line 241 的 pass-through 臂（排队帧无正文可脱敏）<br>· `origin_fanout.rs:111` `_ => None` —— 只从 `RunComplete` 抽终局答案，排队帧天然不扇出到 Telegram/Slack ✓<br>· `team_fanout.rs:202` `_ => {}` —— 排队 run 没有群聊气泡 ✓<br>· `instant_buffer.rs:192` `_ => InstantOutcome::Forward` —— 原样转发 ✓ |
| 位置上报刷屏（车道深 32、频繁变动） | 去重（只在真变化时报）+ 会话级车道上限 32 ⇒ 单会话上限 32 次；帧本身 best-effort |
| 协议 crate 加 `receipt.rs` 影响 `aleph-cli` / `aleph-tui` | 纯新增模块，不改既有导出；但按判据 §10 一并跑 `cargo test -p aleph-tui -p aleph-cli`（最小验证集不覆盖这两个 crate） |

**已知边界（本轮刻意不覆盖）**：
- **phone 形态的 `SessionMap` 不参与**（`app.rs` 的 `not_phone` 门）——`ChatPhase::Queued` 在手机上仍生效（相位是会话级的），但 `pending[]` 的深度显示无处可放。
- **`ahead` 是最后一次上报的值**，不是实时值；两次上报之间车道可能已变。权威值在 `chat.history.pending[]`。

---

## 9. 完成后的文档补充

1. **FEATURE_LOCATOR.md**：§4.7 / §4.8 / §6.1 各加本轮条目；§4.2 表逐条改正（三条已过期登记项）；§6.9 的「已知边界」删掉已连线那条。
2. **GATEWAY.md**：`busy_queue` 一节补「等待态的 wire 表示」与「为什么位置由等待者自报」。
3. **CLAUDE.md 判据清单**：拟新增两条（待实施后按实际收获定稿）——
   - 「一个状态如果只有终局回执没有正向确认，客户端只能拿相邻状态冒充它」
   - 「服务端已经算好并放上 wire 的分类，客户端再猜一遍就是同一个缺陷跨了 crate」
