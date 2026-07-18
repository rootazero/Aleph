# Matrix Channel SDK 重构设计文档

> 日期：2026-04-16  
> 目标：引入 `matrix-sdk`，彻底重构 Aleph 的 Matrix channel，清理旧代码，提升功能完整性和可维护性。

---

## 1. 背景与现状

### 1.1 现状问题

Aleph 当前的 Matrix channel 位于 `src/gateway/interfaces/matrix/`，由以下三个文件构成：

- `mod.rs` — `MatrixChannel` 结构体与 `Channel` trait 实现
- `config.rs` — `MatrixConfig` 配置结构
- `message_ops.rs` — 600+ 行的 HTTP 操作大杂烩（发送、接收、媒体、sync 循环、事件转换）

**核心缺陷：**

| 缺陷 | 说明 |
|------|------|
| 无官方 SDK | 手写 reqwest HTTP，JSON 解析松散，类型安全差 |
| 无 E2EE | 完全不支持 Matrix 端到端加密 |
| 无 sync 持久化 | `since_token` 仅存内存，进程重启后丢失或重放旧消息 |
| 无事件去重 | 没有 inbound deduper，重启或网络抖动时可能重复处理 |
| 事件覆盖窄 | 仅处理 `m.room.message`，不支持 poll/location/reaction/encrypted |
| 模块边界混乱 | `message_ops.rs` 同时承担 HTTP 客户端、事件解析、sync 循环、媒体操作，严重违反单一职责 |
| 可扩展性差 | 每增加一个 Matrix 特性都要手写 HTTP 和 JSON 解析 |

### 1.2 参考基准

OpenClaw 的 Matrix 实现（`/extensions/matrix/`）基于 `matrix-js-sdk`，具备以下能力：

- 完整的 E2EE 加密（Rust Crypto / vodozemac）
- 文件级 sync store 持久化
- Inbound event deduper
- Poll / Location / Reaction / Encrypted 事件全覆盖
- Reply + Thread 上下文解析
- MSC4357 draft streaming 支持
- 房间级策略（autoReply、requireMention、allowBots）
- Probe / Doctor / Startup maintenance 等运维能力

---

## 2. 设计目标

1. **引入 `matrix-sdk`**：用官方 Rust SDK 替换手写 reqwest，获得类型安全、自动重试、sync 持久化。
2. **彻底清理旧代码**：删除 `message_ops.rs`，按 domain 重新划分模块。
3. **Phase 1 先重构后加密**：本次先完成无 E2EE 的 SDK 重构，确保核心路径稳定；E2EE 作为 Phase 2 在 `client.rs` 上开启。
4. **补齐功能缺失**：sync 持久化、事件去重、poll/location/reaction 支持、thread/reply 上下文。
5. **融合 Aleph 架构**：不照搬 OpenClaw，而是将 SDK 能力封装在 Aleph 的 `Channel` trait 边界内。

---

## 3. 新架构设计

### 3.1 目录结构

```
src/gateway/interfaces/matrix/
├── mod.rs              # MatrixChannel + Channel trait impl + Factory
├── config.rs           # MatrixConfig（保留并增强）
├── client.rs           # matrix-sdk Client 封装（登录、sync、状态持久化）
├── sync.rs             # sync loop + 事件分发
├── events.rs           # MatrixEvent -> InboundMessage 转换
├── outbound.rs         # 发送消息/编辑/反应/删除
├── media.rs            # 上传/下载/ MXC 解析
└── dedupe.rs           # inbound event deduper
```

**删除：** `message_ops.rs`（旧代码全部移除）

### 3.2 模块职责

| 模块 | 职责 | 对应旧代码 |
|------|------|-----------|
| `mod.rs` | `MatrixChannel` 结构体、`Channel` trait 实现、`MatrixChannelFactory` | 原 `mod.rs` |
| `config.rs` | 配置定义、验证、房间/用户白名单、mention gating | 原 `config.rs`（增强字段） |
| `client.rs` | 封装 `matrix_sdk::Client`：构建、登录、启动 sync、状态存取 | 原 `message_ops.rs` 上半部分 |
| `sync.rs` | 使用 SDK 的 `sync_with_callback` 或 `sync_stream`，接收 `SyncResponse`，分发给事件处理器 | 原 `message_ops.rs::run_sync_loop` |
| `events.rs` | 将 SDK 的 `TimelineEvent` / `AnyTimelineEvent` 转换为 `InboundMessage` | 原 `message_ops.rs::convert_room_event` |
| `outbound.rs` | 发送文本、附件、编辑、反应、删除；处理 reply/thread relation | 原 `message_ops.rs` 发送相关 |
| `media.rs` | 媒体上传（`m.image/audio/video/file`）、mxc 下载、MIME 推断 | 原 `message_ops.rs` 媒体相关 |
| `dedupe.rs` | 基于 `(room_id, event_id)` 的内存+SQLite 去重器 | 新增 |

---

## 4. 数据流

### 4.1 Inbound 数据流

```
Matrix Homeserver
      |
      v
[matrix-sdk Client] --sync_stream--> [sync.rs]
                                          |
                                          v
                              [dedupe.rs] 去重检查
                                          |
                                          v
                              [events.rs] 事件转换
                                          |
                                          v
                              [mod.rs] 通过 ChannelState 广播 InboundMessage
                                          |
                                          v
                              Gateway EventBus / Agent Loop
```

### 4.2 Outbound 数据流

```
Agent Loop -> OutboundMessage
                  |
                  v
          [mod.rs] MatrixChannel::send
                  |
                  v
          [outbound.rs] 构建 Matrix 消息内容
                  |
                  v
          [media.rs] 如有附件则上传获取 mxc:// URI
                  |
                  v
          [matrix-sdk Client] Room::send()
                  |
                  v
          Matrix Homeserver
```

---

## 5. 关键设计决策

### 5.1 使用 `matrix-sdk` 而非 `matrix-sdk-crypto`（Phase 1）

- `Cargo.toml` 仅引入 `matrix-sdk`（不带 `e2e-encryption` feature）
- 这样获得：类型安全的 API、自动 sync、状态持久化、房间对象管理
- 不引入：加密相关的 device verification、recovery key、cross-signing 复杂度
- Phase 2 时只需在 `client.rs` 中启用 crypto feature 并初始化 `CryptoStore`

### 5.2 Sync 持久化

- 利用 `matrix-sdk` 内置的 `StateStore`（基于 SQLite）
- 持久化 `sync_token`，进程重启后从上次位置继续
- 存储路径：`~/.aleph/state/matrix_sync.db`

### 5.3 事件去重

- `dedupe.rs` 维护一个基于 SQLite 的轻量表：
  ```sql
  CREATE TABLE IF NOT EXISTS matrix_seen_events (
      room_id TEXT NOT NULL,
      event_id TEXT NOT NULL,
      seen_at INTEGER NOT NULL,
      PRIMARY KEY (room_id, event_id)
  );
  ```
- 清理策略：启动时删除 7 天前的记录
- 内存缓存：启动时加载最近 1000 条到 `HashSet` 中，避免高频 SQLite 查询

### 5.4 事件覆盖扩展

`events.rs` 需要处理以下事件类型：

| 事件类型 | 转换行为 |
|----------|----------|
| `m.room.message` (m.text) | 标准文本消息 |
| `m.room.message` (m.image/audio/video/file) | 携带附件的 `InboundMessage` |
| `m.room.message` (m.location) | 文本 + location metadata |
| `m.poll.start` | 转换为 polls 文本描述 |
| `m.reaction` | `MessageMeta::Reaction` |
| `m.room.encrypted` | Phase 1 跳过并告警；Phase 2 解密后处理 |
| `m.room.redaction` | 可记录但暂不主动消费 |

### 5.5 Thread / Reply 上下文

- 从 SDK 的 `m.relates_to` 中提取：
  - `m.in_reply_to` -> `InboundMessage.reply_to`
  - `rel_type: m.thread` + `event_id` -> `MessageMeta::ThreadRoot`
- 支持 `reply_to` 在 outbound 时通过 `RoomMessageEventContent::set_relates_to` 设置

### 5.6 文本分块

- 保留现有 `MessageFormatter::split` 逻辑
- 在 `outbound.rs` 中对长文本分块发送，每块保持同一个 `thread` / `reply` relation（仅第一块）

---

## 6. 配置变更

`MatrixConfig` 新增字段（向后兼容，均有默认值）：

```rust
pub struct MatrixConfig {
    // 原有字段
    pub homeserver_url: String,
    pub access_token: String,
    pub allowed_rooms: Vec<String>,
    pub sync_timeout_ms: u64,
    pub send_typing: bool,
    pub mention_gating: bool,
    pub allowed_users: Vec<String>,

    // 新增字段
    /// Sync 状态持久化数据库路径（默认 ~/.aleph/state/matrix_sync.db）
    pub state_store_path: Option<String>,
    /// 事件去重数据库路径（默认 ~/.aleph/state/matrix_dedupe.db）
    pub dedupe_store_path: Option<String>,
    /// 初始 sync 拉取的历史消息条数
    pub initial_sync_limit: u64,
    /// 是否自动加入邀请的房间
    pub auto_join_invites: bool,
    /// 房间级策略配置
    pub rooms: Option<HashMap<String, MatrixRoomPolicy>>,
}

pub struct MatrixRoomPolicy {
    pub require_mention: Option<bool>,
    pub allow_bots: Option<bool>,
    pub users: Option<Vec<String>>,
}
```

---

## 7. 错误处理

- `matrix-sdk` 的错误类型（`matrix_sdk::Error`）统一在 `client.rs` / `sync.rs` / `outbound.rs` 中映射为 `ChannelError`
- 新增映射：
  - `matrix_sdk::Error::Http` -> `ChannelError::SendFailed` / `ChannelError::ReceiveFailed`
  - `matrix_sdk::Error::Io` -> `ChannelError::Internal`
  - Sync 循环中的非致命错误（如超时）记录 warning 并继续，致命错误（如 auth 失败）转为 `ChannelError::AuthFailed`

---

## 8. 测试策略

| 层级 | 内容 |
|------|------|
| 单元测试 | `config.rs`、`events.rs`、`dedupe.rs` 的纯逻辑测试（无需网络） |
| 集成测试 | 使用 `matrix-sdk-test` 提供的 mock sync response 测试 `sync.rs` -> `events.rs` 整条 inbound 链 |
| 编译验证 | `cargo check -p alephcore`、`cargo clippy -p alephcore -- -D warnings` |
| 回归测试 | 保留并更新 `mod.rs` 中原有的 factory/create/start 测试 |

---

## 9. 变更清单（文件级）

### 删除
- `src/gateway/interfaces/matrix/message_ops.rs` ❌

### 新增
- `src/gateway/interfaces/matrix/client.rs` ✅
- `src/gateway/interfaces/matrix/sync.rs` ✅
- `src/gateway/interfaces/matrix/events.rs` ✅
- `src/gateway/interfaces/matrix/outbound.rs` ✅
- `src/gateway/interfaces/matrix/media.rs` ✅
- `src/gateway/interfaces/matrix/dedupe.rs` ✅

### 修改
- `src/gateway/interfaces/matrix/mod.rs` — 适配新模块，移除 `reqwest::Client`
- `src/gateway/interfaces/matrix/config.rs` — 新增字段与验证
- `Cargo.toml` — 添加 `matrix-sdk` 依赖

---

## 10. 风险与回滚

| 风险 | 缓解措施 |
|------|----------|
| `matrix-sdk` 编译体积增大 | 仅引入基础 feature，暂不带 crypto；评估后决定是否接受 |
| SDK API 与现有 trait 不匹配 | 在 `client.rs` 中做薄封装层，不暴露 SDK 类型到 `Channel` trait |
| 同步行为变化（long-poll vs sync_stream） | 使用 SDK 的 `sync_stream`，行为更标准；本地充分测试 |
| 旧 `message_ops.rs` 测试丢失 | 等效测试在 `events.rs` / `outbound.rs` / `media.rs` 中重写 |

**回滚策略：** 保留旧代码在一个临时 branch；若主分支出现问题，可 revert 单条 commit 恢复。

---

## 11. 实施阶段

本次设计对应 **Phase 1**：

1. 添加 `matrix-sdk` 依赖
2. 编写 `client.rs` → `sync.rs` → `events.rs` → `dedupe.rs` → `outbound.rs` → `media.rs`
3. 重构 `mod.rs` 和 `config.rs`
4. 删除 `message_ops.rs`
5. 运行编译检查、更新/补充单元测试
6. 提交 PR

**Phase 2（后续）**：在 `client.rs` 中启用 `e2e-encryption` feature，添加 device verification 和 recovery key 管理。

---

## 12. 结论

通过引入 `matrix-sdk` 并彻底重构模块边界，Aleph 的 Matrix channel 将从“手写 HTTP 的半成品”升级为“功能完整、结构清晰、具备 E2EE 扩展能力”的生产级实现。同时通过删除 `message_ops.rs` 避免代码债务继续堆积，充分体现 Rust 的类型安全和模块化优势。
