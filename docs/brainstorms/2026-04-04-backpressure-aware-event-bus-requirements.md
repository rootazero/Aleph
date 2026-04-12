---
date: 2026-04-04
topic: backpressure-aware-event-bus
---

# Backpressure-Aware Event Bus

## Problem Frame

GatewayEventBus 使用 `tokio::broadcast` (容量1024) 向所有 WebSocket 客户端广播事件。当前实现是 **fire-and-forget**：如果某个客户端网络慢或处理速度跟不上，服务端继续发送事件，客户端的 receiver buffer 溢出后事件被**丢弃**，且**服务端无感知**。

这导致：
- 慢客户端的事件丢失无反馈
- 无法监控客户端的消费能力
- 无法差异化服务（重要客户端 vs 普通客户端）

**受影响用户**：所有通过 WebSocket 订阅 GatewayEventBus 的客户端。

---

## Requirements

### 客户端缓冲

- **R1**: 每个 WebSocket 连接维护独立的 `mpsc::channel(256)` 缓冲，而非直接订阅 `GatewayEventBus`

- **R2**: 客户端缓冲采用 **Drop-head** 策略：buffer 满时自动丢弃最旧的事件，保留最新事件

- **R3**: 客户端从自己的 buffer 中消费事件并发送给 WebSocket peer；如果 buffer 为空则等待

### 事件流

- **R4**: `GatewayEventBus` 仍然是全局广播通道，所有事件首先进入总线

- **R5**: WebSocket handler 为每个连接创建一个 `PerClientBuffer`（mpsc::Sender 持有者），将总线事件转发到 per-client buffer

- **R6**: Per-client buffer 的 sender 端是 fire-and-forget（不阻塞事件总线）；receiver 端在连接循环中消费

### 监控指标

- **R7**: 暴露客户端缓冲指标到 metrics 系统：
  - `gateway_client_buffer_len`: 当前 buffer 长度
  - `gateway_client_buffer_overflow_total`: 溢出次数（丢弃事件计数）

- **R8**: 溢出指标应关联到具体连接（conn_id 或 client_id）以便追踪

### 向后兼容

- **R9**: 不修改 `GatewayEventBus` 公开接口
- **R10**: 不修改 `EventEmitter` trait 接口
- **R11**: 现有 `SubscriptionManager` 过滤逻辑保持不变（在转发到 per-client buffer 前过滤）

### 错误处理

- **R12**: 如果 WebSocket 发送失败（client 断连），正常关闭连接，不 panic
- **R13**: 如果 per-client buffer 发送失败（receiver 丢弃），计数溢出事件，继续处理

---

## Success Criteria

- **SC1**: 慢客户端不会导致服务端阻塞或报错
- **SC2**: 事件丢失时服务端有指标可查（溢出计数）
- **SC3**: 客户端断开连接时服务器资源正常释放
- **SC4**: 性能回归：p99 延迟 < 现有架构的 p99 延迟

---

## Scope Boundaries

**In Scope:**
- WebSocket handler (`gateway/server/handler.rs`) 的连接管理
- Per-client buffer 的创建和生命周期管理
- Metrics 指标暴露

**Out of Scope:**
- 不修改 GatewayEventBus 内部实现（broadcast channel 容量保持 1024）
- 不实现 producer 端背压（通知生产者减速）
- 不实现 per-client 限流（Qos/优先级）
- 不修改 RunEventBus

---

## Key Decisions

- **Drop-head vs Drop-tail**: 选择 Drop-head（丢弃最旧）因为 streaming 场景下保留最新 chunks 更合理，client 丢几个中间 chunks 比丢最终结果影响小
- **Buffer 容量 256**: 略高于 RunEventBus 的 256，兼顾典型客户端消费速度

---

## Dependencies / Assumptions

- **D1**: WebSocket handler 现有架构可注入 per-client buffer 层（已验证：handler.rs 中连接循环是 `tokio::select!` 模式）
- **D2**: Metrics 系统存在（`tracing` 或自定义 metrics），可直接复用

---

## Outstanding Questions

### Resolve Before Planning
- **Q1** (Implementation): `PerClientBuffer` 应该内嵌在 `ConnectionState` 中还是独立 struct？建议在规划阶段看 `ConnectionState` 定义后决定

### Deferred to Planning
- **Q2** (Metrics): Metrics 命名空间和暴露方式（Prometheus？OpenTelemetry？）
- **Q3** (Testing): 如何构造"慢客户端"场景进行测试？

---

## Next Steps

→ `/ce:plan` for structured implementation planning
