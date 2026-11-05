# Aleph Protocol v1

-   文档状态：设计基线，实施前需与当前仓库接口盘点对齐
-   Protocol：`1.0`
-   传输：本地 IPC（Unix Domain Socket / Windows Named Pipe）
-   消息语义：JSON-RPC 2.0 + Aleph Event Envelope
-   目标：让 Server 与 Desktop 独立演进，并支持多个客户端

## 1. 范围与非目标

### 1.1 范围

Protocol v1 定义：

-   客户端/Server 握手和版本协商。
-   RPC 请求、响应和错误。
-   Server 主动事件。
-   Session 级事件顺序。
-   Snapshot + Replay。
-   请求幂等和取消。
-   本地 IPC framing。
-   认证、授权和连接生命周期的协议要求。
-   大型二进制数据的引用与传输协商。
-   向后兼容规则。

### 1.2 非目标

Protocol v1 不规定：

-   Agent 内部推理和 Prompt 格式。
-   模型供应商 API。
-   Server 内部 Crate 划分。
-   NSH、QuickJS 或 TSX 实现细节。
-   插件内部语言和 Runtime 实现。
-   具体数据库或 Event Store 产品。
-   远程公网 API 的完整安全部署规范。

## 2. 规范性用语

"必须""不得""应该""不应该""可以"分别表示 MUST、MUST NOT、SHOULD、SHOULD
NOT、MAY。

所有 JSON
对象应忽略未知可选字段。必需字段缺失、字段类型错误或违反约束时，应返回明确协议错误，不得静默猜测。

## 3. 传输与消息 framing

### 3.1 本地传输

  平台      传输
  --------- --------------------
  macOS     Unix Domain Socket
  Linux     Unix Domain Socket
  Windows   Named Pipe

默认不使用 localhost TCP 作为本地 IPC。

### 3.2 Frame 格式

``` text
+---------------------------+
| Length: uint32, big-endian|
+---------------------------+
| Payload: Length bytes     |
| UTF-8 JSON                |
+---------------------------+
```

-   Length 表示 Payload 字节数，不包含 4-byte Header。
-   每个 Frame 恰好承载一个 UTF-8 JSON 消息。
-   读取端必须处理 partial read、partial write 和多个 Frame 连续到达。
-   Length 为 0、超过协商上限或无法解码的 Frame 必须拒绝。
-   不得仅根据未经验证的 Length 进行无上限内存分配。
-   建议默认控制消息上限为 8 MiB；具体方法可以规定更小上限。
-   JSON 深度、数组长度、字符串长度和并发请求数也必须受限。

### 3.3 消息类别

连接建立后，Frame Payload 可以是：

1.  JSON-RPC Request。
2.  JSON-RPC Response。
3.  JSON-RPC Notification。
4.  Aleph Event Envelope。
5.  连接控制消息（仅限协议定义的控制流程）。

客户端和 Server
都必须能够识别未知消息类别并按协议规则拒绝或忽略；不得将任意 JSON
对象误判为事件。

## 4. 连接认证与握手

### 4.1 认证要求

IPC 建立前或握手过程中，连接必须绑定到当前 OS
用户，并验证客户端有权访问该 Server 实例。

-   UDS Socket 应位于用户专属目录，设置合理权限。
-   Windows Named Pipe 应设置 DACL，并核验客户端用户身份。
-   可使用短期随机连接凭据或等效机制防止同用户环境中的进程伪装。
-   凭据必须避免出现在命令行参数、普通日志和错误回显中。
-   握手成功不代表客户端拥有所有业务权限；RPC 仍须进行授权。

具体凭据派发方式由 Desktop Supervisor 与 Server
启动器确定。不得将固定静态 Token 写入仓库。

### 4.2 `system.hello`

客户端连接后必须首先调用 `system.hello`。在握手成功前，Server
不应接受业务 RPC。

请求：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "method": "system.hello",
  "params": {
    "protocol": {
      "major": 1,
      "minor": 0
    },
    "client": {
      "name": "aleph-desktop",
      "version": "0.1.0",
      "platform": "macos",
      "ui_runtime": "nsh"
    },
    "features": [
      "event_replay",
      "snapshot",
      "binary_channel"
    ]
  }
}
```

响应：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "result": {
    "protocol": {
      "major": 1,
      "minor": 0
    },
    "server": {
      "name": "aleph-server",
      "version": "0.1.0"
    },
    "connection": {
      "connection_id": "conn-001"
    },
    "features": [
      "event_replay",
      "snapshot",
      "binary_channel"
    ],
    "limits": {
      "max_message_bytes": 8388608,
      "max_inflight_requests": 128
    }
  }
}
```

### 4.3 版本协商

-   `major` 不同：拒绝连接，返回 `version_mismatch`。
-   `major` 相同：协商双方支持的兼容 `minor`。
-   客户端不得假设未协商的 Feature 可用。
-   新增可选字段和 Feature 应保持旧客户端可忽略。
-   改变字段含义、删除必需字段或改变消息语义属于破坏性变更。
-   Server 应返回实际采用的 Protocol 版本，而不是只回显客户端版本。

Feature 名称必须集中定义并维护注册表。未知 Feature
可忽略，不得因此导致整个握手失败，除非客户端将其声明为必需功能（如后续版本增加
`required_features`）。

## 5. JSON-RPC 规则

### 5.1 请求

``` json
{
  "jsonrpc": "2.0",
  "id": "req-1001",
  "method": "agent.run",
  "params": {
    "session_id": "session-001",
    "input": {
      "type": "text",
      "content": "分析当前项目结构"
    }
  }
}
```

-   `jsonrpc` 必须为 `"2.0"`。
-   `id` 必须是字符串或整数；Aleph 实现建议使用字符串。
-   `method` 必须是已注册方法。
-   `params` 必须符合对应方法 Schema。
-   通知（Notification）不带 `id`，且不返回 JSON-RPC Response。
-   业务请求应有超时或明确的长期任务语义。

### 5.2 响应

成功：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-1001",
  "result": {
    "task_id": "task-001",
    "status": "accepted"
  }
}
```

失败：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-1001",
  "error": {
    "code": -32010,
    "message": "Capability denied",
    "data": {
      "type": "capability_denied",
      "retryable": false
    }
  }
}
```

响应的 `id` 必须与请求对应。客户端必须支持多个 in-flight 请求，并按 ID
匹配响应；不得依赖响应顺序。

### 5.3 请求 ID

-   单个连接内，请求 ID 不得在未完成请求之间重复。
-   建议使用随机/递增组合的字符串 ID。
-   断线后客户端不得仅凭旧连接请求 ID 推断操作是否执行成功。
-   对具有副作用的请求必须使用幂等机制。

## 6. RPC Namespace 与方法基线

以下是 Protocol v1
的建议方法目录。实现时应先映射现有接口；不得为满足列表而制造无实际用途的
RPC。每个方法必须有稳定 Schema、权限要求、错误集合和测试。

### 6.1 `system.*`

  方法                       语义
  -------------------------- -----------------------------
  `system.hello`             握手与版本协商
  `system.health`            Server 健康状态
  `system.info`              Server 版本、构建和能力信息
  `system.shutdown`          经授权的优雅关闭请求
  `system.recovery_status`   查询恢复状态

`system.shutdown` 不应被普通 UI 模块开放；仅 Desktop Supervisor
或明确授权的管理客户端可调用。

### 6.2 `session.*`

  方法                     语义
  ------------------------ --------------------------------------
  `session.create`         创建 Session
  `session.list`           列出可访问 Session
  `session.get`            获取 Session 元数据
  `session.get_snapshot`   获取一致性快照
  `session.archive`        归档 Session（若产品支持）
  `session.delete`         删除 Session（若产品支持，需强授权）

### 6.3 `message.*`

  -----------------------------------------------------------------------
  方法                                语义
  ----------------------------------- -----------------------------------
  `message.send`                      向 Session 发送用户输入

  `message.get`                       获取消息

  `message.list`                      分页读取消息

  `message.cancel_generation`         取消当前生成（若语义与 Task cancel
                                      不同）
  -----------------------------------------------------------------------

消息编辑、删除、附件等接口只有在现有产品语义明确后才纳入 v1。

### 6.4 `agent.*`

  方法                 语义
  -------------------- -----------------------------------
  `agent.run`          接受 Agent Task，立即返回 task_id
  `agent.get_task`     获取 Task 状态
  `agent.list_tasks`   查询 Task
  `agent.cancel`       请求取消 Task
  `agent.pause`        暂停（仅当执行模型支持）
  `agent.resume`       恢复（仅当执行模型支持）

`agent.run` 不得保持 RPC 请求直到 Agent Loop 完成。它应返回
`task_id`，进度通过事件流提供。

### 6.5 `tool.*`

  方法                语义
  ------------------- ---------------------------------------
  `tool.list`         列出当前可用 Tool
  `tool.get_schema`   获取 Tool Schema
  `tool.invoke`       经授权调用 Tool（如确有独立调用需求）
  `tool.get_call`     获取 Tool Call 状态/结果

Agent 内部 Tool 调用与外部客户端主动 Tool
调用应有清晰区分，避免外部客户端绕过 Agent Policy。

### 6.6 `capability.*`

  ------------------------------------------------------------------------------
  方法                                语义
  ----------------------------------- ------------------------------------------
  `capability.list`                   查询当前客户端可见能力

  `capability.get`                    查询能力元数据

  `capability.request_approval`       请求用户审批（若产品流程需要）

  `capability.resolve_approval`       提交审批决定，需绑定审批对象和授权上下文
  ------------------------------------------------------------------------------

客户端不能通过 `capability.*` 自行授予权限。

### 6.7 `plugin.*`

  方法                 语义
  -------------------- ------------------------
  `plugin.list`        列出插件
  `plugin.get`         获取插件信息
  `plugin.enable`      启用插件
  `plugin.disable`     禁用插件
  `plugin.install`     安装插件（需管理权限）
  `plugin.uninstall`   卸载插件（需管理权限）

插件管理 RPC 必须校验权限、来源和状态。普通 Generated UI
不得调用安装、启用或禁用接口。

### 6.8 `browser.*`

Browser API 应通过 Server 侧 Browser Supervisor 暴露。具体方法按现有
Browser Sensor 接口确定。NSH WebView 只是 UI 展示宿主，不等同于 Browser
Automation Backend。

### 6.9 `ui.*`

`ui.*` 是 Desktop Host 内部 UI Bridge API
的逻辑命名，不意味着所有方法都必须经过 Server RPC。纯 UI
生命周期操作可留在 Desktop；涉及业务状态或执行能力的操作必须通过
`aleph-client` 到 Server。

## 7. 长任务、取消和幂等

### 7.1 长任务模型

`agent.run` 成功响应表示 Server 已接受任务，不表示任务完成。

``` json
{
  "jsonrpc": "2.0",
  "id": "req-run-1",
  "result": {
    "task_id": "task-001",
    "status": "accepted"
  }
}
```

后续状态通过 `agent.task.*` 事件提供，也可通过 `agent.get_task` 查询。

### 7.2 幂等键

具有副作用或可能因网络中断而重试的 RPC，应接受 `idempotency_key`。

示例：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-run-2",
  "method": "agent.run",
  "params": {
    "idempotency_key": "desktop-installation-42-op-0007",
    "session_id": "session-001",
    "input": {
      "type": "text",
      "content": "运行测试"
    }
  }
}
```

规则：

-   幂等键由客户端为一次逻辑操作生成。
-   Server 在定义的主体/方法/作用域内保存键和结果。
-   同一键、同一参数摘要：返回原操作结果或当前状态。
-   同一键、不同参数摘要：返回 `idempotency_conflict`。
-   幂等记录保留期限必须明确；过期后客户端不得假定仍可去重。
-   幂等键不是授权凭据。

### 7.3 取消

取消是有副作用的状态转换，必须是显式 RPC。

``` json
{
  "jsonrpc": "2.0",
  "id": "req-cancel-1",
  "method": "agent.cancel",
  "params": {
    "task_id": "task-001",
    "reason": "user_requested"
  }
}
```

取消响应只表示 Server 接受了取消请求；最终结果通过 Task
状态事件或查询确认。取消不保证已启动的外部副作用可以回滚。

## 8. Event Envelope

### 8.1 基本结构

``` json
{
  "type": "event",
  "event": {
    "event_id": "evt-10001",
    "scope": {
      "type": "session",
      "id": "session-001"
    },
    "sequence": 10001,
    "name": "agent.task.progress",
    "timestamp": "2026-11-05T02:00:00Z",
    "payload": {
      "task_id": "task-001",
      "status": "running"
    }
  }
}
```

字段：

  字段           类型              说明
  -------------- ----------------- ----------------------------------------
  `event_id`     string            全局唯一或在定义作用域内唯一的事件标识
  `scope.type`   string            `session`、`system` 等
  `scope.id`     string            作用域 ID
  `sequence`     integer           在该 scope 内单调递增
  `name`         string            稳定事件名
  `timestamp`    RFC 3339 string   事件产生时间，非排序依据
  `payload`      object            对应事件 Schema

`sequence` 只在同一 scope 内比较。不同 Session
的序号不可直接比较。`timestamp` 不可替代 sequence。

### 8.2 事件类别

``` text
system.*
session.*
message.*
agent.*
tool.*
capability.*
plugin.*
browser.*
```

事件名称和 payload 必须有
Schema。未知事件应允许客户端忽略，但客户端必须推进其已处理游标的规则应谨慎定义：不能因为忽略未知事件就破坏后续顺序。

### 8.3 Durable 与 Ephemeral

事件必须标记或由事件注册表定义持久化语义：

-   Durable：必须持久化或可确定性重建。
-   Ephemeral：允许丢失、合并或限流，不能承载唯一业务状态变更。

例如 token delta 可以是 Ephemeral，但最终 Message 内容必须通过 Durable
状态保存。Task 创建、终态、审批决定等通常应是
Durable。最终分类以现有状态模型和恢复需求为准。

## 9. Snapshot + Replay

### 9.1 Snapshot

请求：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-snapshot-1",
  "method": "session.get_snapshot",
  "params": {
    "session_id": "session-001"
  }
}
```

响应：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-snapshot-1",
  "result": {
    "session_id": "session-001",
    "snapshot_sequence": 10000,
    "state": {
      "status": "active",
      "messages": [],
      "tasks": []
    }
  }
}
```

Snapshot 的 `state` Schema 应独立版本化。为避免巨型
Snapshot，可按消息分页或返回资源引用；但必须明确 Snapshot
覆盖到哪个序号。

### 9.2 订阅

``` json
{
  "jsonrpc": "2.0",
  "id": "req-subscribe-1",
  "method": "events.subscribe",
  "params": {
    "scope": {
      "type": "session",
      "id": "session-001"
    },
    "after_sequence": 10000
  }
}
```

成功响应：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-subscribe-1",
  "result": {
    "subscription_id": "sub-001",
    "scope": {
      "type": "session",
      "id": "session-001"
    },
    "accepted_after_sequence": 10000
  }
}
```

Server 随后推送 sequence 大于 10000 的事件。

### 9.3 无丢失订阅要求

Snapshot 和 Replay 之间必须有一致性边界。实现可以采用：

-   原子地读取 Snapshot 游标并注册订阅；
-   先注册缓冲订阅，再读取 Snapshot，并丢弃不大于 Snapshot
    游标的缓冲事件；
-   或其他可证明无缺口的实现。

不得采用"先读取 Snapshot，过一段时间再订阅"的无保护流程。

### 9.4 历史不可用

如果 `after_sequence` 早于可用历史：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-subscribe-1",
  "error": {
    "code": -32030,
    "message": "Resynchronization required",
    "data": {
      "type": "resync_required",
      "scope": {
        "type": "session",
        "id": "session-001"
      },
      "earliest_available_sequence": 12000
    }
  }
}
```

客户端应重新获取 Snapshot，再重新订阅。

### 9.5 去重和游标

-   客户端按 `(scope, sequence)` 去重。
-   `event_id` 用于诊断和额外去重。
-   只有连续处理到某个序号后才能推进该 scope 的连续游标。
-   若允许非连续或过滤订阅，必须另行定义游标语义；v1 初期建议 Server 按
    scope 提供完整事件流，客户端在本地过滤。
-   不得把收到事件的最大 sequence 直接当作连续已处理游标。

## 10. 背压、流控和资源限制

-   每连接限制 in-flight RPC 数。
-   每订阅限制待发送事件数量和缓冲字节。
-   发送端必须具备背压策略。
-   Ephemeral 事件可合并或丢弃，但必须保证最终 Durable 状态可恢复。
-   Durable 事件不能静默丢弃；缓冲耗尽时应断开或要求 resync。
-   单个慢客户端不得无限占用 Server 内存。
-   事件订阅必须可取消，并在连接断开时释放。
-   对高频 `message.delta` 可按时间窗口或字节数批量发送；最终 Message
    必须可通过状态查询恢复。

## 11. 二进制数据通道

### 11.1 原则

控制通道使用
JSON；大文件、图片、截图、音频等使用数据通道或受控资源引用。

不要将大对象长期 Base64 嵌入事件。

### 11.2 资源引用

控制消息可返回：

``` json
{
  "resource_id": "resource-001",
  "content_type": "image/png",
  "size_bytes": 524288,
  "transfer_id": "transfer-001",
  "expires_at": "2026-11-05T03:00:00Z"
}
```

Resource ID 必须绑定连接/用户/授权范围，并设置过期时间。

### 11.3 数据传输要求

数据通道必须支持：

-   Transfer ID。
-   Content-Type。
-   总长度。
-   Chunk offset / index。
-   完整性校验。
-   超时、取消和清理。
-   单文件和总并发大小限制。
-   权限校验。

第一阶段可以在同一 IPC 连接中定义二进制
Frame；只有在实际性能数据证明必要时再引入独立 Socket 或共享内存。

## 12. 错误模型

错误：

``` json
{
  "jsonrpc": "2.0",
  "id": "req-err-1",
  "error": {
    "code": -32010,
    "message": "Capability denied",
    "data": {
      "type": "capability_denied",
      "capability": "filesystem.write",
      "resource": "workspace://output.txt",
      "retryable": false,
      "request_id": "req-err-1"
    }
  }
}
```

建议错误类别：

``` text
parse_error
invalid_request
method_not_found
invalid_params
internal_error
version_mismatch
authentication_failed
authorization_denied
capability_denied
resource_not_found
invalid_state
rate_limited
resource_exhausted
operation_cancelled
server_unavailable
resync_required
idempotency_conflict
plugin_unavailable
runtime_limit_exceeded
```

错误码应稳定；`message`
是面向人的文本，不可作为程序判断依据。`data.type`
是稳定机器可读分类。不得在错误中泄露
Token、密钥、完整环境变量或敏感本地路径。

建议保留 JSON-RPC 标准错误码，并为 Aleph 应用错误使用约定的 Server Error
范围；具体数字注册表应在实现时集中定义，不能由各模块随意分配。

## 13. 连接、订阅与关闭

-   连接关闭时，连接级订阅和临时资源必须释放。
-   连接关闭不得自动取消由 Server 接受的 Agent Task。
-   客户端重连后重新握手，再恢复 Snapshot/Replay。
-   `system.shutdown` 仅对具备管理权限的客户端开放。
-   Server
    优雅关闭应停止接受新任务、持久化必要状态、通知客户端并按策略处理现有任务。
-   连接心跳可作为传输健康检测，但不能替代业务状态查询。

## 14. 安全模型

1.  Transport Authentication：验证 OS 用户和连接身份。
2.  Protocol Validation：校验消息长度、Schema、版本和请求速率。
3.  Client Authorization：区分 Desktop、CLI、插件 Host 等客户端身份。
4.  Capability Authorization：对每个操作按主体、资源、动作和上下文授权。
5.  Approval Binding：审批必须绑定具体操作摘要、资源和有效期。
6.  Audit：记录安全相关操作，不记录秘密内容。
7.  Resource Isolation：限制连接、插件、VM 和数据传输资源。
8.  Replay Safety：事件恢复不能重放副作用。

UI 的按钮、隐藏控件或 TSX Capability 声明都不是授权边界。Server
必须独立验证。

## 15. Rust 类型建议

下列为概念模型，不要求逐字照搬。应遵循项目现有异步 Runtime
和序列化约定。

``` rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EventScope {
    #[serde(rename = "type")]
    pub scope_type: String,
    pub id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EventEnvelope<T> {
    pub event_id: String,
    pub scope: EventScope,
    pub sequence: u64,
    pub name: String,
    pub timestamp: String,
    pub payload: T,
}
```

实现注意：

-   内部事件应优先使用强类型 Enum。
-   网络边界 DTO 应与内部业务类型分离。
-   不要把 `serde_json::Value` 作为所有内部业务状态的默认表示。
-   对开放扩展字段可以使用 `Value`，但需限制大小和深度。
-   避免在热路径上重复序列化/反序列化或无必要 Clone。
-   `timestamp` 可在 Rust 内部使用统一时间类型，线上格式为 RFC 3339。
-   对事件名建议使用集中注册表或强类型枚举，兼容未知事件时保留原始字符串。

## 16. 建议 Cargo Crate 边界

``` text
aleph-protocol-types
  ├─ RPC DTO
  ├─ Error DTO
  ├─ Event DTO
  └─ Version / Feature Types

aleph-protocol
  ├─ JSON-RPC Envelope
  ├─ Frame Codec
  ├─ Schema Validation
  └─ Protocol Negotiation

aleph-server-transport
  ├─ UDS / Named Pipe Server
  ├─ Authentication
  ├─ Connection Lifecycle
  └─ Event Delivery

aleph-client
  ├─ Typed RPC Client
  ├─ Request Correlation
  ├─ Subscription API
  └─ Reconnect / Replay Coordination

aleph-client-ipc
  ├─ UDS Client
  └─ Named Pipe Client
```

不要为了这些名字强行拆现有
Crate。只有当模块边界、依赖隔离和测试收益明确时才新增独立 Crate。

## 17. 兼容性与演进

-   Protocol 主版本只在破坏性变更时升级。
-   次版本新增可选方法、字段或 Feature。
-   方法 Schema 必须版本化或遵循兼容扩展规则。
-   不复用已弃用字段表达新语义。
-   删除旧方法前必须经过弃用周期。
-   Server 应能明确报告支持的 Protocol 版本范围。
-   Desktop 应在不兼容时给出可操作错误，而不是循环重连。
-   Protocol Schema、Rust DTO、示例和兼容测试必须同一提交更新。

## 18. Protocol v1 验收测试

### Frame / Codec

-   partial header、partial payload。
-   多个 Frame 连续到达。
-   长度为 0、最大值、超限值。
-   无效 UTF-8、无效 JSON、超深 JSON。
-   并发请求与乱序响应。
-   连接突然断开。

### Handshake

-   同主版本、不同次版本。
-   主版本不兼容。
-   未知 Feature。
-   缺失必需字段。
-   无效凭据和错误 OS 用户。
-   连接握手前发送业务 RPC。

### RPC

-   未知方法、无效参数、权限拒绝。
-   请求超时和取消。
-   重复请求 ID。
-   幂等键重复、参数冲突和过期。
-   多个并发 Task。

### Events

-   同一 Scope 序号单调递增。
-   多 Scope 序号互不混淆。
-   重复事件去重。
-   慢客户端背压。
-   Ephemeral 合并。
-   Durable 事件恢复。
-   Snapshot / Subscribe 竞态无丢失。
-   历史清理后 `resync_required`。

### Recovery / Side Effects

-   Desktop 退出而 Task 继续。
-   Desktop 重连后恢复状态。
-   Server 崩溃后恢复。
-   外部副作用已发生但确认记录未完成时，不盲目重试。
-   取消请求和最终 Task 状态一致。

### Security

-   跨用户 IPC 访问被拒绝。
-   插件/TSX 无权调用未授权方法。
-   超大消息和资源耗尽。
-   审批凭据不能跨操作复用。
-   日志不泄露秘密。

## 19. 实施顺序

1.  盘点当前 Tauri IPC、Server API 和业务 DTO。
2.  建立 `aleph-protocol-types`，先覆盖真实存在的核心接口。
3.  建立 Frame Codec 与 JSON-RPC Envelope。
4.  建立 Server Transport。
5.  建立 Client SDK。
6.  让旧 Tauri 通过新 Client 连接 Server。
7.  实现 `system.hello`、health 和版本协商。
8.  实现 Session / Agent RPC。
9.  实现事件序号、Snapshot 和 Replay。
10. 实现幂等和恢复安全。
11. 新 NSH Desktop 复用相同 Client。
12. 在旧客户端退役前持续运行协议兼容测试。

## 20. 编码 Agent 完成定义

一次 Protocol v1 实施任务只有同时满足以下条件才算完成：

-   DTO 与协议 Schema 已提交。
-   Server 与 Client 实现已提交。
-   兼容性和错误路径有测试。
-   安全与资源限制有测试。
-   旧 Tauri 行为未发生未记录的破坏。
-   文档、示例和测试与实现一致。
-   `cargo fmt`、项目约定的 lint 和测试通过。
-   报告明确列出尚未实现的可选方法和
    Feature，不得将占位实现描述为已支持。
