# Guest Session Activity Logging

## 概述

为 Guest Session Monitoring 系统添加详细的活动日志记录功能，记录访客会话期间的所有活动，包括工具使用、请求详情、错误等。这是实现会话统计分析和历史记录的基础。

## 目标

1. **实时活动记录**：记录访客会话的所有活动
2. **结构化日志**：使用结构化格式存储日志，便于查询和分析
3. **性能优化**：异步日志写入，不影响主流程性能
4. **查询接口**：提供 RPC 方法查询会话活动日志

## 设计

### 1. 活动日志数据结构

```rust
/// Guest session activity log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestActivityLog {
    /// Unique log entry ID
    pub id: String,
    /// Session ID
    pub session_id: String,
    /// Guest ID
    pub guest_id: String,
    /// Activity type
    pub activity_type: ActivityType,
    /// Timestamp (Unix milliseconds)
    pub timestamp: i64,
    /// Activity details (JSON)
    pub details: serde_json::Value,
    /// Success/failure status
    pub status: ActivityStatus,
    /// Error message (if failed)
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActivityType {
    /// Tool execution
    ToolCall { tool_name: String },
    /// RPC request
    RpcRequest { method: String },
    /// Session event
    SessionEvent { event: String },
    /// Permission check
    PermissionCheck { resource: String },
    /// Error occurred
    Error { error_type: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActivityStatus {
    Success,
    Failed,
    Pending,
}
```

### 2. 日志存储

**选项 A: 内存存储（初期实现）**
- 使用 `DashMap<String, Vec<GuestActivityLog>>` 按 session_id 存储
- 设置最大日志条数限制（如 1000 条/会话）
- 会话结束后保留日志一段时间（如 1 小时）

**选项 B: SQLite 持久化（后期扩展）**
- 创建 `guest_activity_logs` 表
- 支持复杂查询和长期存储
- 定期清理过期日志

### 3. 日志记录点

在以下位置添加日志记录：

1. **工具调用**
   - 位置: `executor/mod.rs` 或 tool execution wrapper
   - 记录: 工具名称、参数、执行时间、结果

2. **RPC 请求**
   - 位置: Gateway request handler
   - 记录: 方法名、参数、响应状态

3. **权限检查**
   - 位置: Permission validation logic
   - 记录: 检查的资源、结果

4. **会话事件**
   - 位置: GuestSessionManager
   - 记录: 连接、断开、超时等事件

5. **错误**
   - 位置: Error handling code
   - 记录: 错误类型、消息、堆栈

### 4. RPC 接口

```rust
// 查询会话活动日志
guests.getActivityLogs(session_id, options?) -> ActivityLogsResult

// 选项
{
  limit?: number,           // 最大返回条数
  offset?: number,          // 偏移量
  activity_type?: string,   // 过滤活动类型
  status?: string,          // 过滤状态
  start_time?: number,      // 开始时间
  end_time?: number,        // 结束时间
}

// 响应
{
  logs: GuestActivityLog[],
  total: number,
  has_more: boolean
}
```

### 5. macOS 客户端集成

**RPC 类型**
```swift
// GatewayRPCTypes+Guests.swift
struct GWActivityLog: Codable, Sendable, Identifiable {
    let id: String
    let sessionId: String
    let guestId: String
    let activityType: String
    let timestamp: Int64
    let details: [String: Any]
    let status: String
    let error: String?
}

struct GWGetActivityLogsParams: Codable, Sendable {
    let sessionId: String
    let limit: Int?
    let offset: Int?
    let activityType: String?
    let status: String?
}

struct GWGetActivityLogsResult: Codable, Sendable {
    let logs: [GWActivityLog]
    let total: Int
    let hasMore: Bool
}
```

**UI 组件**
```swift
// GuestSessionActivityView.swift
- 活动时间线视图
- 按类型过滤
- 搜索功能
- 导出日志
```

## 实施步骤

### Phase 1: Backend Infrastructure (Core)

#### Step 1.1: 创建活动日志数据结构
**文件**: `core/src/gateway/security/activity_log.rs`
- 定义 `GuestActivityLog` 结构
- 定义 `ActivityType` 和 `ActivityStatus` 枚举
- 实现序列化/反序列化

#### Step 1.2: 创建活动日志管理器
**文件**: `core/src/gateway/security/activity_logger.rs`
- `GuestActivityLogger` 结构
- 内存存储实现（DashMap）
- 日志记录方法
- 日志查询方法
- 自动清理过期日志

#### Step 1.3: 集成到 GuestSessionManager
**文件**: `core/src/gateway/security/guest_session_manager.rs`
- 添加 `activity_logger` 字段
- 在关键操作点记录活动
- 会话结束时保留日志

### Phase 2: Gateway Integration

#### Step 2.1: 添加 RPC Handler
**文件**: `core/src/gateway/handlers/guests.rs`
- `handle_get_activity_logs` 函数
- 参数验证和解析
- 调用 ActivityLogger 查询
- 返回格式化结果

#### Step 2.2: 注册 RPC 方法
**文件**: `core/src/bin/aleph_gateway/commands/start.rs`
- 注册 `guests.getActivityLogs` handler
- 传递 activity_logger 依赖

### Phase 3: Logging Integration

#### Step 3.1: 工具调用日志
**文件**: `core/src/executor/mod.rs` 或相关文件
- 在工具执行前后记录日志
- 记录工具名称、参数、结果

#### Step 3.2: RPC 请求日志
**文件**: `core/src/gateway/server.rs`
- 在处理 guest 请求时记录日志
- 记录方法名、状态

#### Step 3.3: 权限检查日志
**文件**: Permission validation code
- 记录权限检查结果

### Phase 4: macOS Client

#### Step 4.1: RPC 类型定义
**文件**: `clients/macos/Aleph/Sources/Gateway/GatewayRPCTypes+Guests.swift`
- 添加活动日志相关类型

#### Step 4.2: RPC 方法
**文件**: `clients/macos/Aleph/Sources/Gateway/GatewayClient+Guests.swift`
- `guestsGetActivityLogs()` 方法

#### Step 4.3: UI 组件
**文件**: `clients/macos/Aleph/Sources/GuestSessionActivityView.swift`
- 活动日志列表视图
- 过滤和搜索功能

#### Step 4.4: 集成到会话详情
**文件**: `clients/macos/Aleph/Sources/GuestSessionsView.swift`
- 在 SessionCard 中添加"查看活动"按钮
- 显示活动日志 sheet

## 数据示例

### 工具调用日志
```json
{
  "id": "log-123",
  "session_id": "session-456",
  "guest_id": "guest-789",
  "activity_type": {
    "ToolCall": {
      "tool_name": "translate"
    }
  },
  "timestamp": 1770464000000,
  "details": {
    "input": "Hello",
    "output": "你好",
    "duration_ms": 150
  },
  "status": "Success",
  "error": null
}
```

### RPC 请求日志
```json
{
  "id": "log-124",
  "session_id": "session-456",
  "guest_id": "guest-789",
  "activity_type": {
    "RpcRequest": {
      "method": "agent.run"
    }
  },
  "timestamp": 1770464001000,
  "details": {
    "params": {"prompt": "..."},
    "response_size": 1024
  },
  "status": "Success",
  "error": null
}
```

### 权限检查日志
```json
{
  "id": "log-125",
  "session_id": "session-456",
  "guest_id": "guest-789",
  "activity_type": {
    "PermissionCheck": {
      "resource": "tool:summarize"
    }
  },
  "timestamp": 1770464002000,
  "details": {
    "allowed": false,
    "reason": "Tool not in allowed_tools list"
  },
  "status": "Failed",
  "error": "Permission denied"
}
```

## 性能考虑

1. **异步日志写入**
   - 使用 channel 异步写入日志
   - 避免阻塞主流程

2. **日志限制**
   - 每个会话最多 1000 条日志
   - 超过限制时删除最旧的日志

3. **内存管理**
   - 会话结束后 1 小时自动清理日志
   - 定期清理过期会话的日志

4. **查询优化**
   - 支持分页查询
   - 支持按类型和状态过滤

## 测试计划

1. **单元测试**
   - ActivityLogger 基本功能
   - 日志记录和查询
   - 自动清理

2. **集成测试**
   - 端到端日志记录
   - RPC 接口测试
   - 并发访问测试

3. **性能测试**
   - 高频日志写入性能
   - 查询性能
   - 内存使用

## 后续扩展

1. **SQLite 持久化**
   - 长期存储日志
   - 支持复杂查询

2. **日志导出**
   - 导出为 JSON/CSV
   - 日志归档

3. **实时日志流**
   - WebSocket 实时推送日志
   - 实时监控界面

4. **日志分析**
   - 统计分析
   - 异常检测
   - 使用模式分析

## 预估工作量

- **Phase 1**: 3-4 小时
- **Phase 2**: 1-2 小时
- **Phase 3**: 2-3 小时
- **Phase 4**: 3-4 小时
- **测试**: 1-2 小时

**总计**: 10-15 小时（约 2 个工作日）

## 成功标准

1. ✅ 活动日志正确记录所有关键操作
2. ✅ RPC 接口可以查询日志
3. ✅ macOS 客户端可以显示活动日志
4. ✅ 性能影响可接受（< 5% 延迟增加）
5. ✅ 内存使用合理（< 10MB per session）
6. ✅ 日志自动清理正常工作
