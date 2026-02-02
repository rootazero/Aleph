# Halo-Only 消息流重构设计

> **设计日期**: 2026-02-02
> **状态**: 待实施
> **核心理念**: "Invisible First" — 完全取消对话窗口，所有交互通过 Halo 浮层完成

---

## 1. 背景与问题

### 1.1 当前 Aether vs OpenClaw 对比

| 对比维度 | OpenClaw | Aether (当前) | 差距 |
|----------|----------|---------------|------|
| **消息流架构** | 单一 WebSocket 路径，150ms 节流 | FFI + Gateway 双路径 | 2x 代码量 |
| **状态管理** | 3 种状态 (delta/final/error) | 16+ HaloState 变体 | 5x 复杂度 |
| **UI 分离** | 一个聊天窗口 | Halo + UnifiedConversation 双窗口 | 用户认知成本高 |
| **Agent 结果汇总** | `emitChatFinal()` 简洁输出 | 分散在多处 | 难以追踪 |

### 1.2 核心痛点

1. **单独窗口设计**: Multi-turn 对话在独立窗口，打断了 Halo 的 "invisible first" 体验
2. **状态爆炸**: HaloState 16+ 种状态变体，代码难以维护和扩展
3. **双路径复杂性**: FFI + Gateway 两套代码路径，功能重复且难以测试

---

## 2. 设计目标

1. **完全取消 UnifiedConversationWindow** — 所有交互通过 Halo 浮层完成
2. **状态简化为 6 种** — 从 16+ 减少到 6 种核心状态
3. **`//` 命令访问历史** — 轻量级历史访问方式
4. **类 OpenClaw 的 final 汇总** — 简洁的 `RunComplete` 事件模型
5. **代码削减 ~3500 行** — 删除冗余的对话窗口代码

---

## 3. 新状态架构

### 3.1 HaloState 简化 (6 种状态)

```swift
enum HaloState: Equatable {
    /// 完全隐藏
    case idle

    /// 脉冲圆圈，监听剪贴板/等待输入
    case listening

    /// 流式响应 (合并原来的 processingWithAI, typewriting, planProgress 等)
    case streaming(StreamingContext)

    /// 需要用户确认 (合并原来的 toolConfirmation, planConfirmation 等)
    case confirmation(ConfirmationContext)

    /// 最终结果 Toast，自动消失 (替代 success)
    case result(ResultContext)

    /// 错误状态，可重试/忽略
    case error(ErrorContext)
}
```

### 3.2 StreamingContext 详细设计

```swift
struct StreamingContext: Equatable {
    let runId: String
    var text: String                    // 累积的输出文本
    var toolCalls: [ToolCallInfo]       // 活跃的工具调用 (最多显示 3 个)
    var reasoning: String?              // 可选：显示 thinking (如果启用)
    var phase: StreamingPhase           // 当前阶段

    /// 最大显示的工具调用数
    static let maxToolCalls = 3
}

enum StreamingPhase: Equatable {
    case thinking       // AI 正在思考 (显示脉冲动画)
    case responding     // AI 正在输出 (显示流式文本)
    case toolExecuting  // 工具执行中 (显示工具卡片)
}

struct ToolCallInfo: Equatable, Identifiable {
    let id: String
    let name: String
    var status: ToolStatus
    var progressText: String?
}

enum ToolStatus: Equatable {
    case pending
    case running
    case completed
    case failed
}
```

**UI 尺寸映射：**

| Phase | 窗口尺寸 | 内容 |
|-------|---------|------|
| `thinking` | 80×60 | 脉冲圆圈 + "思考中..." |
| `responding` | 320×100 | 紧凑文本预览（最后 80 字符） |
| `toolExecuting` | 280×120 | 工具图标 + 名称 + 状态 |

### 3.3 ConfirmationContext 设计

```swift
struct ConfirmationContext: Equatable {
    let runId: String
    let type: ConfirmationType
    let title: String
    let description: String
    let options: [ConfirmationOption]
    var selectedOption: Int?
}

enum ConfirmationType: Equatable {
    case toolExecution      // 工具执行确认
    case planApproval       // 计划审批
    case fileConflict       // 文件冲突解决
    case userQuestion       // Agent 提问
}

struct ConfirmationOption: Equatable {
    let id: String
    let label: String
    let isDestructive: Bool
    let isDefault: Bool
}
```

### 3.4 ResultContext 设计 (参考 OpenClaw)

```swift
struct ResultContext: Equatable {
    let runId: String
    let summary: ResultSummary
    let timestamp: Date
    var autoDismissDelay: TimeInterval = 2.0
}

struct ResultSummary: Equatable {
    let status: ResultStatus
    let message: String?            // 简短消息
    let toolsExecuted: Int          // 执行的工具数
    let tokensUsed: Int?            // 可选 token 消耗
    let durationMs: Int             // 总耗时
    let finalResponse: String       // 完整响应（用于复制）
}

enum ResultStatus: Equatable {
    case success    // ✓ 绿色
    case partial    // ⚠ 黄色（部分完成）
    case error      // ✗ 红色
}
```

### 3.5 ErrorContext 设计

```swift
struct ErrorContext: Equatable {
    let runId: String?
    let type: ErrorType
    let message: String
    let suggestion: String?
    let canRetry: Bool
}

enum ErrorType: Equatable {
    case network
    case provider
    case toolFailure
    case timeout
    case unknown
}
```

---

## 4. `//` 历史命令设计

### 4.1 触发方式

- 用户输入 `//` + Enter
- 菜单栏图标点击 → "对话历史"
- 快捷键 `Cmd+Shift+H` (可选)

### 4.2 UI 呈现

```
┌─────────────────────────────────────┐
│  📋 对话历史                [×]     │
├─────────────────────────────────────┤
│  ▸ 今天                              │
│    • 代码重构讨论 (2小时前)          │
│    • Bug 修复 (4小时前)              │
│  ▸ 昨天                              │
│    • 架构设计 (昨天 14:30)           │
│  ▸ 本周                              │
│    • ...                             │
├─────────────────────────────────────┤
│  🔍 搜索历史...                      │
└─────────────────────────────────────┘
```

**窗口尺寸**: 380×420 (类似原 planConfirmation)

### 4.3 交互流程

1. 用户输入 `//` → Halo 展开为历史列表
2. 选择一个主题 → Halo 加载该主题上下文
3. ESC 或点击外部 → 关闭列表，恢复 idle
4. 搜索框支持模糊匹配

---

## 5. Gateway 事件简化

### 5.1 新事件模型 (参考 OpenClaw)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    // === 生命周期 (3) ===
    RunAccepted {
        run_id: String,
        session_key: String,
        accepted_at: i64,
    },
    RunComplete {
        run_id: String,
        summary: RunSummary,
        total_duration_ms: u64,
    },
    RunError {
        run_id: String,
        error: String,
        error_code: Option<String>,
    },

    // === 流式内容 (2) ===
    /// 增量内容更新 (合并 ResponseChunk + Reasoning)
    Delta {
        run_id: String,
        seq: u64,
        content: String,
        is_thinking: bool,  // true = 思考内容, false = 响应内容
    },
    /// 最终完整内容
    Final {
        run_id: String,
        seq: u64,
        content: String,
    },

    // === 工具 (3) ===
    ToolStart {
        run_id: String,
        seq: u64,
        tool_id: String,
        tool_name: String,
        params: serde_json::Value,
    },
    ToolUpdate {
        run_id: String,
        seq: u64,
        tool_id: String,
        progress: String,
    },
    ToolEnd {
        run_id: String,
        seq: u64,
        tool_id: String,
        result: ToolResult,
        duration_ms: u64,
    },

    // === 交互 (1) ===
    AskUser {
        run_id: String,
        seq: u64,
        question: String,
        options: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub status: RunStatus,
    pub message: Option<String>,
    pub tools_executed: u32,
    pub tokens_used: Option<u32>,
    pub final_response: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunStatus {
    Success,
    Partial,
    Error,
}
```

### 5.2 节流机制 (参考 OpenClaw 的 150ms)

```rust
impl GatewayEventEmitter {
    /// Delta 事件节流间隔
    const DELTA_THROTTLE_MS: u64 = 150;

    pub async fn emit_delta(&self, content: &str, is_thinking: bool) {
        let now = Instant::now();
        let last = self.last_delta_at.load(Ordering::Relaxed);

        if now.duration_since(last).as_millis() < Self::DELTA_THROTTLE_MS as u128 {
            // 缓冲内容，不立即发送
            self.delta_buffer.lock().await.push_str(content);
            return;
        }

        // 发送缓冲内容 + 新内容
        let buffered = std::mem::take(&mut *self.delta_buffer.lock().await);
        let full_content = format!("{}{}", buffered, content);

        self.emit(StreamEvent::Delta {
            run_id: self.run_id.clone(),
            seq: self.next_seq(),
            content: full_content,
            is_thinking,
        }).await;

        self.last_delta_at.store(now, Ordering::Relaxed);
    }
}
```

---

## 6. 代码清理计划

### 6.1 待删除文件

```
platforms/macos/Aether/Sources/
├── UnifiedConversationWindow.swift     (删除)
├── UnifiedConversationView.swift       (删除)
├── UnifiedConversationViewModel.swift  (删除)
├── Components/
│   ├── ConversationAreaView.swift      (删除)
│   ├── MessageBubbleView.swift         (删除)
│   ├── ReasoningPartView.swift         (删除 - 合并到 Halo)
│   ├── PlanPartView.swift              (删除 - 合并到 Halo)
│   └── ToolCallPartView.swift          (删除 - 合并到 Halo)
├── MultiTurnCoordinator.swift          (删除)
└── Gateway/
    └── GatewayMultiTurnAdapter.swift   (删除)
```

### 6.2 待简化的 HaloState 变体

| 旧状态 | 处理方式 |
|--------|---------|
| `.processingWithAI` | → `.streaming(.thinking)` |
| `.processing(streamingText)` | → `.streaming(.responding)` |
| `.typewriting` | 删除 |
| `.planConfirmation` | → `.confirmation(.planApproval)` |
| `.planProgress` | → `.streaming` |
| `.taskGraphConfirmation` | → `.confirmation(.planApproval)` |
| `.taskGraphProgress` | → `.streaming` |
| `.agentPlan` | → `.confirmation(.planApproval)` |
| `.agentProgress` | → `.streaming` |
| `.agentConflict` | → `.confirmation(.fileConflict)` |
| `.toolConfirmation` | → `.confirmation(.toolExecution)` |

### 6.3 保留但重构

| 文件 | 重构内容 |
|------|---------|
| `HaloState.swift` | 简化为 6 状态 + 新数据结构 |
| `HaloView.swift` | 简化 switch 分支，添加历史列表视图 |
| `HaloWindow.swift` | 简化尺寸逻辑 |
| `EventHandler.swift` | 统一消息处理，移除 MultiTurn 分支 |
| `ConversationStore.swift` | 适配历史列表功能 |

### 6.4 预估代码变化

| 类别 | 行数变化 |
|------|---------|
| 删除 UnifiedConversation 系列 | -3000 行 |
| 简化 HaloState 相关 | -500 行 |
| 新增 StreamingContext 等 | +200 行 |
| 新增历史列表视图 | +300 行 |
| **净变化** | **约 -3000 行** |

---

## 7. 数据流设计

### 7.1 新的消息流

```
用户输入
    ↓
EventHandler.handleInput()
    ↓
Gateway chat.send RPC
    ↓
AgentRunManager.start_run()
    ↓
Agent Loop 执行
    │
    ├─ GatewayEventEmitter.emit_delta()  (150ms 节流)
    │   ↓
    │   StreamEvent::Delta → WebSocket → HaloState::streaming
    │
    ├─ GatewayEventEmitter.emit_tool_start()
    │   ↓
    │   StreamEvent::ToolStart → streaming.toolCalls.append()
    │
    └─ GatewayEventEmitter.emit_complete()
        ↓
        StreamEvent::RunComplete → HaloState::result
            ↓
        2 秒后自动消失 → HaloState::idle
```

### 7.2 确认流程

```
StreamEvent::AskUser
    ↓
HaloState::confirmation
    ↓
用户选择
    ↓
EventHandler.sendConfirmation()
    ↓
Agent 继续执行
```

### 7.3 历史访问流程

```
用户输入 "//"
    ↓
HaloState::historyList (新状态，可选)
或
Halo 展开为历史面板 (作为 streaming 的特殊模式)
    ↓
用户选择主题
    ↓
加载主题上下文到 session
    ↓
HaloState::listening
```

---

## 8. 迁移策略

### Phase 1: 状态简化 (不破坏现有功能)

1. 定义新的 6 种状态枚举
2. 创建状态映射层：旧状态 → 新状态
3. 更新 HaloView 支持新状态
4. 保留旧代码作为兼容层

### Phase 2: Gateway 事件简化

1. 定义新事件类型
2. 添加节流机制
3. 更新 EventHandler 处理新事件
4. 旧事件类型标记 deprecated

### Phase 3: 删除旧代码

1. 移除 UnifiedConversation 系列
2. 移除 MultiTurnCoordinator
3. 清理 deprecated 状态
4. 清理 deprecated 事件

### Phase 4: 历史列表功能

1. 实现 `//` 命令触发
2. 实现历史列表 UI
3. 实现搜索功能
4. 添加菜单栏入口

---

## 9. 测试计划

### 9.1 状态测试

- [ ] 所有 6 种状态的 UI 正确渲染
- [ ] 状态转换正确（idle → streaming → result → idle）
- [ ] 窗口尺寸随状态正确变化

### 9.2 流式测试

- [ ] Delta 事件正确累积文本
- [ ] 150ms 节流正常工作
- [ ] 工具调用正确显示（最多 3 个）
- [ ] Final 事件触发 result 状态

### 9.3 历史功能测试

- [ ] `//` 命令正确触发历史面板
- [ ] 历史按时间分组正确显示
- [ ] 搜索功能正常工作
- [ ] 选择主题正确加载上下文

---

## 10. 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| 删除代码导致功能丢失 | 高 | Phase 1 保留兼容层，逐步迁移 |
| 节流影响响应及时性 | 中 | 150ms 经 OpenClaw 验证，可接受 |
| 历史搜索性能 | 低 | SQLite FTS 已支持，性能可控 |
| 用户习惯改变 | 中 | 提供迁移提示，//命令易学 |

---

## 11. 成功指标

1. **代码量减少 3000+ 行**
2. **HaloState 从 16+ 减少到 6 种**
3. **消息流路径从 2 条减少到 1 条**
4. **用户反馈：更简洁的交互体验**
