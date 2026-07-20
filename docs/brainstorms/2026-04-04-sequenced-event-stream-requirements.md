---
date: 2026-04-04
topic: sequenced-event-stream
---

# Sequenced Event Stream with Gap Detection & Server-Side Backfill

## Problem Frame

WebChat 用户观看 agent 运行 trace 时，如果 WebSocket 广播滞后（tokio `broadcast::channel(1024)` 溢出），事件被静默丢弃，用户看到残缺的输出 —— 中间缺少 token、工具调用消失、trace 不完整。当前唯一处理是 `handler.rs:606` 的 `warn!` 日志，无任何恢复机制。

### 实际事件流（经代码验证）

```
┌─────────────┐    ┌─────────────────────┐    ┌──────────────┐    ┌───────────┐
│ Agent Loop   │───▶│ GatewayEventEmitter │───▶│ GatewayEvent │───▶│ WebSocket │
│ (produces    │    │ (has own seq_counter│    │ Bus (broadcast│    │ Handler   │
│  StreamEvent)│    │  serializes to JSON │    │ channel<String>│   │ (receives │
│              │    │  with run_id + seq) │    │ cap=1024)    │    │  String)  │
└──────────────┘    └─────────┬───────────┘    └──────────────┘    └─────┬─────┘
                              │                                         │
                     ┌─ NEW ──┤                                ┌─ NEW ──┤
                     ▼        │                                ▼        │
              ┌──────────────┐│                       ┌─────────────────┤
              │ agent_events ││                       │ On Lagged(n):  │
              │ (SQLite)     │◀───────────────────────│ parse run_id+  │
              │ seq-indexed  │        backfill        │ seq from JSON, │
              │              │───────────────────────▶│ query DB,      │
              └──────────────┘                        │ push to client │
                                                      └────────────────┘
```

**关键架构事实（review 发现的 P0 修正）：**
- WebSocket handler 订阅的是 `GatewayEventBus`，广播的是**序列化后的 JSON String**，不是 `RunEvent`
- `GatewayEventEmitter` 有自己的 `seq_counter: AtomicU64`，独立于 `ActiveRunHandle.seq_counter`
- **StreamEvent JSON 中已包含 `run_id` 和 `seq` 字段** —— handler 需要从 JSON 中解析这些字段来做 seq 追踪
- `GatewayEventEmitter` 是持久化的正确注入点 —— 它同时拥有 `seq_counter` 和 `event_bus` 引用

现有可复用基础设施：
- `GatewayEventEmitter.seq_counter` (AtomicU64) 已为每个 StreamEvent 分配单调递增序列号
- `agent_events` 表已有 `seq` 字段和 `get_events_since_seq()`、`get_events_in_range()` 查询方法（`(task_id, seq)` 上有复合索引）
- `SubscriptionManager` 已有 per-connection topic 过滤

## Requirements

**Event Persistence**

- R1. `GatewayEventEmitter.emit()` 在将 StreamEvent 序列化并发布到 `GatewayEventBus` 的同时，将事件异步写入 `agent_events` 表，复用已有的 `bulk_insert_events()` 批量接口
- R2. 写入时 seq 字段使用 `GatewayEventEmitter.next_seq()` 的返回值（即序列化到 JSON 中的同一个 seq），保证广播 seq 和持久化 seq 一致
- R3. 持久化采用异步批量写入（先到先触发：50ms 超时或 32 条积累），不阻塞事件发射的热路径。回填查询前必须先 force-flush 当前 batch buffer，确保最近事件已落盘

**Gap Detection & Backfill**

- R4. WebSocket handler 在 `RecvError::Lagged(n)` 时，从 JSON String 中解析 `run_id` 和 `seq`（利用已有的序列化格式），确定每个活跃 run 的 `last_delivered_seq`
- R5. 服务端从 `agent_events` 表查询 `get_events_since_seq(run_id, last_delivered_seq)` 获取遗漏事件（`run_id` 直接作为 `task_id` 参数传入，两者是同一概念的不同命名）
- R6. 回填事件通过 WebSocket 以 JSON-RPC notification 形式推送，附带特殊 topic `event.backfill` 以便客户端区分实时事件和回填事件
- R7. 回填期间暂停该连接的实时广播转发（暂时不从 broadcast receiver 消费），回填完成后重新订阅广播，从最新 seq 开始继续。这保证客户端收到的事件严格有序，不会出现回填事件和实时事件交错

**Per-Connection Tracking**

- R8. 每个 WebSocket 连接维护 `last_delivered_seq: HashMap<String, u64>`（key 为 run_id），记录每个活跃 run 的最后成功投递序列号。run 对应的 `RunComplete`/`RunFailed`/`RunCancelled` 事件投递后，清除该 run_id 的条目
- R9. 正常事件转发时，从序列化 JSON 中提取 `run_id` + `seq`（轻量 JSON 解析或结构化信封），更新 `last_delivered_seq`

**Event Envelope**

- R10. WebSocket 推送的事件 JSON 中已包含 `run_id` 和 `seq` 字段（StreamEvent 序列化自带），无需额外修改。客户端可选地利用这些字段做前端排序或去重

**Infrastructure Change**

- R11. `GatewaySharedState` 新增 `Arc<StateDatabase>` 字段，使 WebSocket handler 可以访问数据库进行回填查询。相应修改 server builder 和 ConnectionContext 构造路径

## Success Criteria

- WebChat 观看 agent 运行时，即使 broadcast channel 发生 lag，trace 输出最终完整无缺失
- 回填过程对客户端透明（不需要客户端发起任何请求）
- 正常场景（无 lag）零额外延迟 —— 持久化是异步的，不在事件发射的热路径上
- 回填期间事件严格有序 —— 不出现回填事件和实时事件交错
- 与已有的 trace replay（`agent_trace_replay.rs`）共享同一数据源（agent_events 表）

## Scope Boundaries

- **不包含**: 跨设备断线重连恢复（未来可扩展，但本次不做）
- **不包含**: 全局 gateway seq（本次只做 per-emitter seq，与 GatewayEventEmitter 的 seq_counter 一致）
- **不包含**: 客户端侧 gap 检测逻辑（服务端全权负责回填）
- **不包含**: 新的数据库表或 schema 变更（复用 agent_events）
- **不包含**: 事件压缩或去重（直接推送原始事件）
- **不包含**: 非 StreamEvent 的事件回填（config 变更、presence 等 gateway 级事件不在范围内）

## Key Decisions

- **GatewayEventEmitter seq（非 ActiveRunHandle seq）**: GatewayEventEmitter 有自己的 seq_counter，这是实际序列化到 JSON 中的 seq，也是 handler 能从广播字符串中解析出的 seq。使用这个 seq 作为持久化和回填的基准
- **服务端主动推送（非客户端请求）**: 客户端零改动，服务端检测到 lag 后自动回填，降低前端复杂度
- **复用 agent_events 表**: 零新表，已有 `get_events_since_seq()` 直接可用，与 trace replay 共享数据源
- **异步批量持久化 + 回填前 force-flush**: 正常路径不阻塞，但回填查询前强制刷新 buffer 确保数据完整性
- **回填期间暂停实时转发**: 保证严格有序，避免回填和实时事件交错。代价是回填期间可能积累更多 lag，但 SQLite 查询足够快（有索引），回填窗口极短

## Dependencies / Assumptions

- `agent_events` 表的 seq 字段和 `get_events_since_seq()` 已正确实现（已确认存在，`(task_id, seq)` 上有复合索引）
- `GatewayEventEmitter.seq_counter` 为 per-emitter 单调递增（已确认 AtomicU64，SeqCst ordering）
- `agent_events` 表使用 `task_id` 作为键，StreamEvent 使用 `run_id` —— 两者是同一概念的不同命名
- `GatewaySharedState` 当前不持有 `StateDatabase` 引用 —— R11 需要添加

## Outstanding Questions

### Deferred to Planning

- [Affects R3][Technical] 异步批量写入的实现方式：tokio::spawn 后台 task + mpsc channel 收集事件，还是在 emit() 内部用 buffer + timer
- [Affects R9][Technical] 从序列化 JSON String 中提取 run_id + seq 的最轻量方式 —— 全量 serde_json::from_str 还是用 simd-json 部分解析还是在发布时附带结构化信封
- [Affects R1][Technical] StreamEvent → AgentEvent 的映射：StreamEvent 枚举变体（ResponseChunk, ToolStart, ToolEnd, RunComplete 等）如何映射到 agent_events 的 `event_type` (String) 和 `payload_json` (String)，以及 `is_structural` 分类（ToolStart/ToolEnd/RunComplete = structural, ResponseChunk/ReasoningDelta = pulse）
- [Affects R7][Technical] "暂停实时转发"的具体实现 —— 是暂停从 broadcast receiver 消费（可能导致 receiver 也 lag），还是消费但丢弃（需要追踪丢弃范围），还是消费并追加到回填队列

## Next Steps

→ `/ce:plan` for structured implementation planning
